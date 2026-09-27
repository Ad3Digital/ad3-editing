// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use crate::{Result, document::Node};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        let ffmpeg = env::var_os("AD3_FFMPEG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| "ffmpeg".into());
        let sibling = ffmpeg.with_file_name(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        let ffprobe = env::var_os("AD3_FFPROBE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if sibling.is_file() {
                    sibling
                } else {
                    "ffprobe".into()
                }
            });
        Self { ffmpeg, ffprobe }
    }
}

pub fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command
}

pub fn checked_output(command: &mut Command) -> Result<Output> {
    checked_output_cancellable(command, None, Duration::from_secs(120))
}

pub fn checked_output_cancellable(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    timeout: Duration,
) -> Result<Output> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Operation cancelled".into());
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Cannot start {}: {e}",
                command.get_program().to_string_lossy()
            )
        })?;
    let stdout = child.stdout.take().ok_or("Missing process stdout")?;
    let stderr = child.stderr.take().ok_or("Missing process stderr")?;
    let read = |stream: Box<dyn Read + Send>, limit: u64| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            stream
                .take(limit + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > limit {
                return Err("Process output exceeded its limit".to_string());
            }
            Ok(bytes)
        })
    };
    let out = read(Box::new(stdout), 512 * 1024 * 1024);
    let err = read(Box::new(stderr), 1024 * 1024);
    let started = Instant::now();
    let status = loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            break Err("Operation cancelled".to_owned());
        }
        if started.elapsed() >= timeout {
            break Err("Process timed out".to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => (),
            Err(error) => break Err(error.to_string()),
        }
        thread::sleep(Duration::from_millis(20));
    };
    if status.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let stdout = out.join().map_err(|_| "Process stdout reader failed")?;
    let stderr = err.join().map_err(|_| "Process stderr reader failed")?;
    let output = Output {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
    };
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} exited {}: {}",
            command.get_program().to_string_lossy(),
            output.status,
            error.chars().take(4000).collect::<String>()
        ));
    }
    Ok(output)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub duration: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub sample_rate: u32,
    pub channels: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub still: bool,
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .filter(|v| v.is_finite())
}

impl Tools {
    pub fn probe(&self, path: &Path) -> Result<Probe> {
        if !path.is_file() {
            return Err(format!("Media file not found: {}", path.display()));
        }
        let output = checked_output(
            command(&self.ffprobe)
                .args([
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-show_streams",
                    "-show_format",
                    "-of",
                    "json",
                ])
                .arg(path),
        )?;
        let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        let mut probe = Probe::default();
        let streams = metadata
            .get("streams")
            .and_then(Value::as_array)
            .ok_or("FFprobe returned no streams")?;
        for stream in streams {
            match stream["codec_type"].as_str() {
                Some("video")
                    if !probe.has_video
                        && stream["disposition"]["attached_pic"].as_u64() != Some(1) =>
                {
                    probe.has_video = true;
                    probe.width = stream["width"]
                        .as_u64()
                        .unwrap_or(0)
                        .try_into()
                        .map_err(|_| "Media width overflow")?;
                    probe.height = stream["height"]
                        .as_u64()
                        .unwrap_or(0)
                        .try_into()
                        .map_err(|_| "Media height overflow")?;
                    if let Some((a, b)) = stream["avg_frame_rate"]
                        .as_str()
                        .and_then(|s| s.split_once('/'))
                    {
                        let numerator = a.parse::<f64>().unwrap_or(0.0);
                        let denominator = b.parse::<f64>().unwrap_or(0.0);
                        if denominator > 0.0 {
                            probe.frame_rate = numerator / denominator;
                        }
                    }
                    probe.duration = stream.get("duration").and_then(number);
                }
                Some("audio") if !probe.has_audio => {
                    probe.has_audio = true;
                    probe.sample_rate =
                        stream.get("sample_rate").and_then(number).unwrap_or(0.0) as u32;
                    probe.channels = stream["channels"].as_u64().unwrap_or(0) as u32;
                    if !probe.has_video {
                        probe.duration = stream.get("duration").and_then(number);
                    }
                }
                _ => (),
            }
        }
        if probe.duration.is_none() {
            probe.duration = metadata["format"].get("duration").and_then(number);
        }
        let format = metadata["format"]["format_name"].as_str().unwrap_or("");
        probe.still = !probe.has_audio && (format.ends_with("_pipe") || format == "image2");
        if probe.still {
            probe.duration = None;
        }
        if probe.duration.is_some_and(|duration| duration <= 0.0) {
            return Err("Media has no positive duration".into());
        }
        Ok(probe)
    }

    pub fn rgba(&self, path: &Path, time: f64, width: u32, height: u32) -> Result<Vec<u8>> {
        if !time.is_finite()
            || time < 0.0
            || width == 0
            || height == 0
            || u64::from(width) * u64::from(height) > 4096 * 4096
        {
            return Err("Invalid frame request".into());
        }
        if !path.is_file() {
            return Err(format!("Media file not found: {}", path.display()));
        }
        let output = checked_output(
            command(&self.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-ss",
                    &time.to_string(),
                    "-i",
                ])
                .arg(path)
                .args([
                    "-map",
                    "0:v:0",
                    "-an",
                    "-sn",
                    "-frames:v",
                    "1",
                    "-vf",
                    &format!("scale={width}:{height}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "pipe:1",
                ]),
        )?;
        if output.stdout.len() != width as usize * height as usize * 4 {
            return Err("Decoder returned an incomplete frame".into());
        }
        Ok(output.stdout)
    }

    pub fn waveform(&self, path: &Path, output: &Path) -> Result<()> {
        if !path.is_file() {
            return Err("Media file not found".into());
        }
        checked_output(command(&self.ffmpeg).args(["-v", "error", "-nostdin", "-n", "-protocol_whitelist", "file,pipe", "-i"]).arg(path)
            .args(["-filter_complex", "[0:a:0]aformat=channel_layouts=mono,showwavespic=s=1600x240:colors=0x39d5bc[v]", "-map", "[v]", "-frames:v", "1"]).arg(output))?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MediaLibrary {
    pub sources: BTreeMap<String, PathBuf>,
    pub probes: BTreeMap<String, Probe>,
    pub durations: BTreeMap<String, f64>,
}

impl MediaLibrary {
    pub fn load(root: &Node, project: &Path, tools: &Tools) -> Result<Self> {
        let manifest = project.join("assets.yml");
        let manifest: Value = if manifest.is_file() {
            if fs::metadata(&manifest).map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
                return Err("Asset manifest is too large".into());
            }
            serde_yaml::from_slice(&fs::read(manifest).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Invalid assets.yml: {e}"))?
        } else {
            Value::Null
        };
        let mut library = Self {
            sources: BTreeMap::new(),
            probes: BTreeMap::new(),
            durations: BTreeMap::new(),
        };
        let mut cache = BTreeMap::new();
        for node in root.nodes() {
            if !matches!(
                node.tag.as_str(),
                "video" | "audio" | "image" | "videoPaint" | "imagePaint"
            ) {
                continue;
            }
            let src = node
                .string("src")
                .ok_or_else(|| format!("{}: media requires a string src", node.id))?;
            let source = manifest
                .get("assets")
                .and_then(Value::as_array)
                .and_then(|assets| {
                    assets.iter().find(|asset| {
                        asset["path"].as_str() == Some(src) || asset["id"].as_str() == Some(src)
                    })
                })
                .and_then(|asset| asset["source"].as_str())
                .unwrap_or(src);
            if source.contains("://") {
                return Err(format!(
                    "{}: remote media resolution is not ported yet",
                    node.id
                ));
            }
            let path = Path::new(source);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                project.join(path)
            };
            let path = std::path::absolute(path).map_err(|e| e.to_string())?;
            if !path.is_file() {
                return Err(format!("{}: source not found: {}", node.id, path.display()));
            }
            let probe = if let Some(probe) = cache.get(&path) {
                Probe::clone(probe)
            } else {
                let probe = tools.probe(&path)?;
                cache.insert(path.clone(), probe.clone());
                probe
            };
            if node.tag == "audio" && !probe.has_audio {
                return Err(format!("{}: source has no audio stream", node.id));
            }
            if matches!(
                node.tag.as_str(),
                "video" | "videoPaint" | "image" | "imagePaint"
            ) && !probe.has_video
            {
                return Err(format!("{}: source has no video/image stream", node.id));
            }
            if let Some(duration) = probe.duration {
                library.durations.insert(node.id.clone(), duration);
            }
            library.sources.insert(node.id.clone(), path);
            library.probes.insert(node.id.clone(), probe);
        }
        // A media paint supplies the timing of the geometry holding it.
        for node in root.nodes() {
            if node.is_timed()
                && !library.durations.contains_key(&node.id)
                && let Some(duration) = node
                    .children
                    .iter()
                    .filter(|c| c.tag == "videoPaint")
                    .find_map(|child| library.durations.get(&child.id))
                    .copied()
            {
                library.durations.insert(node.id.clone(), duration);
            }
        }
        Ok(library)
    }
}

pub fn is_mp4(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
}

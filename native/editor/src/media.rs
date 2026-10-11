// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use crate::{Result, document::Node, encoder::cancelled};
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
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
    checked_output_cancellable_limit(command, cancel, timeout, 512 * 1024 * 1024)
}

pub fn checked_output_cancellable_limit(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    timeout: Duration,
    stdout_limit: u64,
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
    let reader_failed = Arc::new(AtomicBool::new(false));
    let read = |stream: Box<dyn Read + Send>, limit: u64| {
        let reader_failed = reader_failed.clone();
        thread::spawn(move || {
            let result = (|| {
                let mut bytes = Vec::new();
                stream
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > limit {
                    return Err("Process output exceeded its limit".to_string());
                }
                Ok(bytes)
            })();
            if result.is_err() {
                reader_failed.store(true, Ordering::Relaxed);
            }
            result
        })
    };
    let out = read(Box::new(stdout), stdout_limit);
    let err = read(Box::new(stderr), 1024 * 1024);
    let started = Instant::now();
    let status = loop {
        if reader_failed.load(Ordering::Relaxed) {
            break Err("Process output read failed or exceeded its limit".into());
        }
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
        stdout: stdout?,
        stderr: stderr?,
        status: status?,
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

/// Consume codec output incrementally, while draining stderr and retaining
/// cancellation/timeout ownership in the calling thread.
pub(crate) fn checked_stream_cancellable<T: Send>(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    timeout: Duration,
    consume: impl FnOnce(std::process::ChildStdout) -> Result<T> + Send,
) -> Result<T> {
    cancelled(cancel)?;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("Missing codec stdout")?;
    let stderr = child.stderr.take().ok_or("Missing codec stderr")?;
    let failed = AtomicBool::new(false);
    thread::scope(|scope| {
        let output = scope.spawn(|| {
            let result = consume(stdout);
            if result.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            result
        });
        let errors = scope.spawn(|| {
            let mut bytes = Vec::new();
            let result = stderr
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())
                .and_then(|_| {
                    if bytes.len() > 1024 * 1024 {
                        Err("Codec stderr exceeded its limit".into())
                    } else {
                        Ok(bytes)
                    }
                });
            if result.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            result
        });
        let started = Instant::now();
        let status = loop {
            if failed.load(Ordering::Relaxed) {
                break Err("Codec stream reader failed".to_string());
            }
            if let Err(error) = cancelled(cancel) {
                break Err(error);
            }
            if started.elapsed() >= timeout {
                break Err("Codec stream timed out".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => (),
                Err(error) => break Err(error.to_string()),
            }
            thread::sleep(Duration::from_millis(10));
        };
        if status.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let output = output.join();
        let errors = errors.join();
        // Killing a codec can leave a partial PCM frame. Preserve the initiating
        // cancellation/timeout instead of reporting that secondary read error.
        if let Err(error) = &status
            && error != "Codec stream reader failed"
        {
            return Err(error.clone());
        }
        let output = output.map_err(|_| "Codec stdout consumer panicked")??;
        let errors = errors.map_err(|_| "Codec stderr reader panicked")??;
        let status = status?;
        if !status.success() {
            return Err(format!(
                "Codec exited {status}: {}",
                String::from_utf8_lossy(&errors)
                    .chars()
                    .take(4000)
                    .collect::<String>()
            ));
        }
        Ok(output)
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceVersion {
    pub(crate) path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
}
impl SourceVersion {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let info = fs::metadata(&path).map_err(|e| e.to_string())?;
        if !info.is_file() {
            return Err("Media source is not a file".into());
        }
        Ok(Self {
            path,
            len: info.len(),
            modified: info.modified().ok(),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
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
    pub rotation: f64,
    pub pixel_aspect: f64,
    pub start_time: f64,
    #[serde(skip)]
    pub version: Option<SourceVersion>,
}

impl Probe {
    pub fn display_dimensions(&self) -> Result<(u32, u32)> {
        if !self.rotation.is_finite() {
            return Err("Invalid video rotation metadata".into());
        }
        let turns = self.rotation / 90.0;
        if (turns - turns.round()).abs() > 1e-6 {
            return Err("Arbitrary video display rotation is not ported yet".into());
        }
        let aspect = if self.pixel_aspect == 0.0 {
            1.0
        } else {
            self.pixel_aspect
        };
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Invalid video pixel aspect ratio".into());
        }
        let width = crate::timing::round_frame(self.width as f64 * aspect);
        if width < 1.0 || width > u32::MAX as f64 {
            return Err("Video display width overflows".into());
        }
        let width = width as u32;
        Ok(if turns.rem_euclid(2.0).round() == 1.0 {
            (self.height, width)
        } else {
            (width, self.height)
        })
    }
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        .filter(|v| v.is_finite())
}
fn frame_rate(value: &Value) -> Option<f64> {
    let (a, b) = value.as_str()?.split_once('/')?;
    let value = a.parse::<f64>().ok()? / b.parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

// Matroska's DURATION tag and container duration are end timestamps; FFprobe's
// stream duration (and formats such as MPEG-TS) already reports elapsed time.
fn timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    let result = hours * 3600.0 + minutes * 60.0 + seconds;
    (parts.next().is_none() && result.is_finite() && result >= 0.0).then_some(result)
}

impl Tools {
    pub fn probe(&self, path: &Path) -> Result<Probe> {
        self.probe_cancellable(path, None)
    }
    pub fn probe_cancellable(&self, path: &Path, cancel: Option<&AtomicBool>) -> Result<Probe> {
        cancelled(cancel)?;
        if !path.is_file() {
            return Err(format!("Media file not found: {}", path.display()));
        }
        let version = SourceVersion::read(path)?;
        let output = checked_output_cancellable_limit(
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
                .arg(&version.path),
            cancel,
            Duration::from_secs(120),
            16 * 1024 * 1024,
        )?;
        cancelled(cancel)?;
        if SourceVersion::read(path)? != version {
            return Err("Media source changed during probing".into());
        }
        let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
        let format = metadata["format"]["format_name"].as_str().unwrap_or("");
        let matroska = format
            .split(',')
            .any(|name| matches!(name, "matroska" | "webm"));
        let mut probe = Probe {
            version: Some(version),
            ..Default::default()
        };
        let mut nominal_rate = None;
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
                    probe.frame_rate = frame_rate(&stream["avg_frame_rate"]).unwrap_or(0.0);
                    nominal_rate = frame_rate(&stream["r_frame_rate"]);
                    probe.duration = stream.get("duration").and_then(number);
                    probe.start_time = stream.get("start_time").and_then(number).unwrap_or(0.0);
                    if probe.duration.is_none() && matroska {
                        probe.duration = stream["tags"]["DURATION"]
                            .as_str()
                            .and_then(timestamp)
                            .map(|end| (end - probe.start_time.max(0.0)).max(0.0));
                    }
                    probe.rotation = stream
                        .get("side_data_list")
                        .and_then(Value::as_array)
                        .and_then(|items| {
                            items
                                .iter()
                                .find_map(|item| item.get("rotation").and_then(number))
                        })
                        .unwrap_or(0.0);
                    if let Some((a, b)) = stream["sample_aspect_ratio"]
                        .as_str()
                        .and_then(|s| s.split_once(':'))
                    {
                        let a = a.parse::<f64>().unwrap_or(0.0);
                        let b = b.parse::<f64>().unwrap_or(0.0);
                        if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 {
                            probe.pixel_aspect = a / b;
                        }
                    }
                }
                Some("audio") if !probe.has_audio => {
                    probe.has_audio = true;
                    probe.sample_rate =
                        stream.get("sample_rate").and_then(number).unwrap_or(0.0) as u32;
                    probe.channels = stream["channels"].as_u64().unwrap_or(0) as u32;
                    if !probe.has_video {
                        probe.duration = stream.get("duration").and_then(number);
                        probe.start_time = stream.get("start_time").and_then(number).unwrap_or(0.0);
                        if probe.duration.is_none() && matroska {
                            probe.duration = stream["tags"]["DURATION"]
                                .as_str()
                                .and_then(timestamp)
                                .map(|end| (end - probe.start_time.max(0.0)).max(0.0));
                        }
                    }
                }
                _ => (),
            }
        }
        if probe.duration.is_none() {
            probe.duration = metadata["format"].get("duration").and_then(number);
            if matroska {
                probe.duration = probe
                    .duration
                    .map(|end| (end - probe.start_time.max(0.0)).max(0.0));
            }
        }
        probe.still = !probe.has_audio && (format.ends_with("_pipe") || format == "image2");
        if probe.still {
            probe.duration = None;
        }
        if probe.duration.is_some_and(|duration| duration <= 0.0) {
            return Err("Media has no positive duration".into());
        }
        if probe.has_video && !probe.still && probe.frame_rate <= 0.0 {
            // The original asset rate is packet count divided by content time.
            // Only scan packets when ordinary average metadata is missing.
            let measured = if let Some(duration) = probe.duration {
                let output = checked_output_cancellable_limit(
                    command(&self.ffprobe)
                        .args([
                            "-v",
                            "error",
                            "-protocol_whitelist",
                            "file,pipe",
                            "-select_streams",
                            "V:0",
                            "-count_packets",
                            "-show_entries",
                            "stream=nb_read_packets",
                            "-of",
                            "json",
                        ])
                        .arg(&probe.version.as_ref().unwrap().path),
                    cancel,
                    Duration::from_secs(120),
                    16 * 1024 * 1024,
                )?;
                let packets: Value =
                    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
                number(&packets["streams"][0]["nb_read_packets"])
                    .map(|count| count / duration)
                    .filter(|rate| rate.is_finite() && *rate > 0.0)
            } else {
                None
            };
            probe.frame_rate = measured
                .or(nominal_rate)
                .ok_or("Video has no usable frame rate metadata")?;
        }
        cancelled(cancel)?;
        if Some(SourceVersion::read(path)?) != probe.version {
            return Err("Media source changed during probing".into());
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

#[derive(Default)]
pub struct ProbeCache(BTreeMap<(SourceVersion, bool), Probe>);

fn read_asset_manifest(project: &Path) -> Result<Value> {
    let manifest = project.join("assets.yml");
    let file = match fs::File::open(&manifest) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
        return Err("Asset manifest is too large".into());
    }
    parse_asset_manifest(file)
}

fn parse_asset_manifest(reader: impl Read) -> Result<Value> {
    let mut bytes = Vec::new();
    reader
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Asset manifest is too large".into());
    }
    serde_yaml::from_slice(&bytes).map_err(|e| format!("Invalid assets.yml: {e}"))
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    #[ignore = "requires installed FFmpeg"]
    fn streaming_codec_cancellation_timeout_and_reader_failure_release_workers() {
        let tools = Tools::default();
        let make_command = || {
            let mut process = command(&tools.ffmpeg);
            process.args([
                "-v",
                "error",
                "-nostdin",
                "-re",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=8000:cl=mono",
                "-f",
                "f32le",
                "pipe:1",
            ]);
            process
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let (ready, receiver) = std::sync::mpsc::sync_channel(1);
        let controller = thread::spawn(move || {
            let received = receiver.recv_timeout(Duration::from_secs(3)).is_ok();
            signal.store(true, Ordering::Relaxed);
            received
        });
        let started = Instant::now();
        let error = checked_stream_cancellable(
            &mut make_command(),
            Some(&cancel),
            Duration::from_secs(5),
            |mut stdout| -> Result<()> {
                let mut ready = Some(ready);
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stdout
                        .read(&mut buffer)
                        .map_err(|error| error.to_string())?;
                    if count == 0 {
                        return Err("partial PCM after interruption".into());
                    }
                    if let Some(ready) = ready.take() {
                        ready.send(()).map_err(|error| error.to_string())?;
                    }
                }
            },
        )
        .unwrap_err();
        assert!(
            controller.join().unwrap(),
            "No PCM arrived before cancellation"
        );
        assert!(error.to_lowercase().contains("cancelled"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let error = checked_stream_cancellable(
            &mut make_command(),
            None,
            Duration::from_millis(100),
            |mut stdout| -> Result<()> {
                std::io::copy(&mut stdout, &mut std::io::sink())
                    .map_err(|error| error.to_string())?;
                Err("partial PCM after interruption".into())
            },
        )
        .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let error = checked_stream_cancellable(
            &mut make_command(),
            None,
            Duration::from_secs(5),
            |_| -> Result<()> { Err("Injected consumer failure".into()) },
        )
        .unwrap_err();
        assert_eq!(error, "Injected consumer failure");
        let mut process = command(&tools.ffmpeg);
        process.arg("-version");
        let count =
            checked_stream_cancellable(&mut process, None, Duration::from_secs(5), |mut stdout| {
                std::io::copy(&mut stdout, &mut std::io::sink()).map_err(|error| error.to_string())
            })
            .unwrap();
        assert!(count > 0);
    }

    #[test]
    fn manifest_stream_limit_applies_without_trusting_a_prior_file_length() {
        // An unbounded stream models growth after the initial length check.
        assert_eq!(
            parse_asset_manifest(std::io::repeat(b' ')).unwrap_err(),
            "Asset manifest is too large"
        );
        let manifest = parse_asset_manifest(std::io::Cursor::new(
            b"assets:\n  - id: movie\n    path: videos/clip\n    source: local.mp4\n",
        ))
        .unwrap();
        assert_eq!(manifest["assets"][0]["source"], "local.mp4");
        assert!(
            parse_asset_manifest(std::io::Cursor::new(b"assets: [unclosed"))
                .unwrap_err()
                .contains("Invalid assets.yml")
        );
    }
}

fn resolve_asset_source(src: &str, project: &Path, manifest: &Value) -> Result<PathBuf> {
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
        return Err("remote media resolution is not ported yet".into());
    }
    let path = Path::new(source);
    let path = std::path::absolute(if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    })
    .map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("source not found: {}", path.display()));
    }
    Ok(path)
}

/// Resolve a local file or project-library path/id without importing it or
/// mounting unrelated project media. Shared with scene source binding.
pub fn resolve_source(src: &str, project: &Path) -> Result<PathBuf> {
    resolve_asset_source(src, project, &read_asset_manifest(project)?)
}

impl MediaLibrary {
    pub fn load(root: &Node, project: &Path, tools: &Tools) -> Result<Self> {
        Self::load_cached(root, project, tools, &mut ProbeCache::default())
    }

    pub fn load_cached(
        root: &Node,
        project: &Path,
        tools: &Tools,
        cache: &mut ProbeCache,
    ) -> Result<Self> {
        Self::load_cached_cancellable(root, project, tools, cache, None)
    }
    pub fn load_cached_cancellable(
        root: &Node,
        project: &Path,
        tools: &Tools,
        cache: &mut ProbeCache,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self> {
        cancelled(cancel)?;
        let manifest = read_asset_manifest(project)?;
        let mut library = Self {
            sources: BTreeMap::new(),
            probes: BTreeMap::new(),
            durations: BTreeMap::new(),
        };
        for node in root.nodes() {
            cancelled(cancel)?;
            if !matches!(
                node.tag.as_str(),
                "video" | "audio" | "image" | "videoPaint" | "imagePaint"
            ) {
                continue;
            }
            let src = node
                .string("src")
                .ok_or_else(|| format!("{}: media requires a string src", node.id))?;
            let path = resolve_asset_source(src, project, &manifest)
                .map_err(|error| format!("{}: {error}", node.id))?;
            let version = SourceVersion::read(&path)?;
            // Source binding follows the asset kind, independently of JSX tag.
            // Every visual source gets the same Rust image probe first.
            let image = node.tag != "audio";
            let cache_key = (version.clone(), image);
            let probe = if let Some(probe) = cache.0.get(&cache_key) {
                Probe::clone(probe)
            } else {
                let probe = if image {
                    match crate::texture::probe_image(&path, cancel)? {
                        Some(probe) => probe,
                        None => tools.probe_cancellable(&path, cancel)?,
                    }
                } else {
                    tools.probe_cancellable(&path, cancel)?
                };
                if probe.version.as_ref() != Some(&version) {
                    return Err("Media source changed before probing".into());
                }
                cache
                    .0
                    .retain(|(existing, kind), _| existing.path != version.path || *kind != image);
                cache.0.insert(cache_key, probe.clone());
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
                    .filter(|c| crate::paints::is_media(&c.tag))
                    .filter(|c| library.probes.get(&c.id).is_some_and(|p| !p.still))
                    .find_map(|child| library.durations.get(&child.id))
                    .copied()
            {
                library.durations.insert(node.id.clone(), duration);
            }
        }
        Ok(library)
    }

    /// Playback forwards every visible video fill but uses only the last one's
    /// audio. Even a silent video fill replaces an intrinsic audio source.
    pub(crate) fn audio_source<'a>(&self, holder: &'a Node) -> Option<&'a Node> {
        if !holder.is_timed() {
            return None;
        }
        holder
            .children
            .iter()
            .rev()
            .find(|n| {
                crate::paints::is_media(&n.tag)
                    && !n.flag("hidden")
                    && self.probes.get(&n.id).is_some_and(|p| !p.still)
            })
            .or_else(|| {
                (holder.tag == "audio"
                    || self
                        .probes
                        .get(&holder.id)
                        .is_some_and(|p| p.has_video && !p.still))
                .then_some(holder)
            })
    }
}

pub fn is_mp4(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
}

pub fn export_extension(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("mp4") {
        Some("mp4")
    } else if extension.eq_ignore_ascii_case("mov") {
        Some("mov")
    } else if extension.eq_ignore_ascii_case("webm") {
        Some("webm")
    } else if extension.eq_ignore_ascii_case("ogg") {
        Some("ogg")
    } else {
        None
    }
}

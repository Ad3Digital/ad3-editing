// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Rust owns timeline evaluation, filter construction, source clocks and mixing.
//! FFmpeg is the external codec tool, also used by the existing desktop build.
use crate::{
    Result,
    document::Node,
    media::{MediaLibrary, Tools, checked_output, checked_output_cancellable, command, is_mp4},
    timing::Timeline,
};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct RenderPlan {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub fps: f64,
    pub graph: String,
    pub inputs: Vec<OsString>,
    pub has_audio: bool,
    text_files: Vec<String>,
}

fn dimension(n: f64) -> Result<u32> {
    if !n.is_finite() || n < 1.0 || n > 8192.0 || n.fract() != 0.0 {
        return Err("Render dimensions must be integers from 1 to 8192".into());
    }
    Ok(n as u32)
}

fn color(value: &serde_json::Value) -> Result<String> {
    let packed = match value {
        serde_json::Value::String(text) => crate::color::parse(text),
        serde_json::Value::Number(number) => number.as_f64().and_then(crate::color::number),
        _ => None,
    };
    // The JSX reconciler stores packed RGB. Alpha embedded in authored colors
    // is discarded there; opacity is an independent composition property.
    packed
        .map(|rgb| format!("0x{rgb:06X}"))
        .ok_or_else(|| format!("Invalid color: {value}"))
}

fn supported(node: &Node) -> Result<()> {
    let common = [
        "id",
        "name",
        "start",
        "end",
        "sourceIn",
        "sourceOut",
        "playbackRate",
        "hidden",
        "muted",
        "volume",
        "x",
        "y",
        "offsetX",
        "offsetY",
        "width",
        "height",
        "selected",
        "expanded",
        "clipHeight",
        "clipColor",
        "keepAspectRatio",
    ];
    let specific: &[&str] = match node.tag.as_str() {
        "scene" => &["active", "workarea", "fill"],
        "group" => &["opacity"],
        "sequence" => &[],
        "video" | "image" => &["src", "objectFit", "opacity"],
        "audio" => &["src"],
        "rect" => &["fill", "opacity"],
        "text" => &["color", "opacity", "fontSize", "fontFamily"],
        tag => {
            return Err(format!(
                "{}: native rendering of <{tag}> is not ported yet",
                node.id
            ));
        }
    };
    for name in node.props.keys() {
        if !common.contains(&name.as_str()) && !specific.contains(&name.as_str()) {
            return Err(format!(
                "{}: render property {name:?} is not ported yet; refusing an incomplete render",
                node.id
            ));
        }
    }
    if node.tag == "text" && (node.props.contains_key("width") || node.props.contains_key("height"))
    {
        return Err(format!(
            "{}: native text wrapping is not ported yet",
            node.id
        ));
    }
    Ok(())
}

impl RenderPlan {
    /// Scale after composition so authored coordinates remain in scene pixels.
    pub fn resize_output(&mut self, height: u32) -> Result<()> {
        let width = dimension(
            (f64::from(self.width) * f64::from(height) / f64::from(self.height) / 2.0).round()
                * 2.0,
        )?;
        dimension(f64::from(height))?;
        let intermediate = format!("unscaled{}", self.graph.len());
        self.graph = self.graph.replace("[vout]", &format!("[{intermediate}]"));
        self.graph += &format!(";[{intermediate}]scale={width}:{height}[vout]");
        self.width = width;
        self.height = height;
        Ok(())
    }
    pub fn build(
        root: &Node,
        timeline: &Timeline,
        library: &MediaLibrary,
        scene_id: &str,
        fps: f64,
        window: Option<[f64; 2]>,
    ) -> Result<Self> {
        if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
            return Err("FPS must be in (0, 240]".into());
        }
        let scene = root
            .find(scene_id)
            .filter(|n| n.tag == "scene")
            .ok_or("No active scene")?;
        supported(scene)?;
        let width = dimension(scene.number("width", 1920.0)?)?;
        let height = dimension(scene.number("height", 1080.0)?)?;
        let window = window.unwrap_or(
            *timeline
                .workareas
                .get(scene_id)
                .ok_or("Scene has no workarea")?,
        );
        if window.iter().any(|t| !t.is_finite()) || window[0] < 0.0 || window[1] <= window[0] {
            return Err("Invalid render window".into());
        }
        let duration = window[1] - window[0];
        if duration > 24.0 * 3600.0 {
            return Err("Native render workarea exceeds the current 24-hour job limit".into());
        }
        let background = scene
            .props
            .get("fill")
            .map(color)
            .transpose()?
            .unwrap_or_else(|| "0x000000".into());
        let mut plan = Self {
            width,
            height,
            duration,
            fps,
            graph: format!(
                "color=c={background}:s={width}x{height}:r={fps}:d={duration},format=rgba[base0]"
            ),
            inputs: Vec::new(),
            has_audio: false,
            text_files: Vec::new(),
        };
        let mut visual = 0;
        let mut input = 0;
        let mut audio = Vec::new();
        plan.children(
            scene,
            timeline,
            library,
            window,
            [0.0, 0.0],
            1.0,
            &mut visual,
            &mut input,
            &mut audio,
        )?;
        plan.graph += &format!(";[base{visual}]null[vout]");
        if !audio.is_empty() {
            plan.graph += &format!(
                ";{}amix=inputs={}:duration=longest:normalize=0,apad,atrim=duration={duration}[aout]",
                audio.join(""),
                audio.len()
            );
            plan.has_audio = true;
        }
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    fn children(
        &mut self,
        parent: &Node,
        timeline: &Timeline,
        library: &MediaLibrary,
        window: [f64; 2],
        offset: [f64; 2],
        parent_opacity: f64,
        visual: &mut usize,
        input: &mut usize,
        audio: &mut Vec<String>,
    ) -> Result<()> {
        for node in &parent.children {
            // Validate all authored features even when outside this frame, so a
            // frame capture is never mistaken for proof of full export support.
            supported(node)?;
            let span = timeline.get(&node.id)?;
            if span.hidden {
                continue;
            }
            let x = offset[0] + node.number("x", 0.0)? + node.number("offsetX", 0.0)?;
            let y = offset[1] + node.number("y", 0.0)? + node.number("offsetY", 0.0)?;
            // The original Canvas renderNode inherits globalAlpha per draw;
            // groups do not flatten their children into an isolated layer.
            let requested_opacity = parent_opacity * node.number("opacity", 1.0)?;
            // Canvas ignores an invalid globalAlpha assignment and retains the
            // previous alpha; this also matters for overshooting keyframes.
            // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-globalalpha
            let opacity = if (0.0..=1.0).contains(&requested_opacity) {
                requested_opacity
            } else {
                parent_opacity
            };
            if node.is_container() {
                self.children(
                    node,
                    timeline,
                    library,
                    window,
                    [x, y],
                    opacity,
                    visual,
                    input,
                    audio,
                )?;
                continue;
            }
            if !node.children.is_empty() {
                return Err(format!(
                    "{}: native paints, effects and animation are not ported yet",
                    node.id
                ));
            }
            let start = span.start.max(window[0]);
            let end = span.end.min(window[1]);
            if end <= start {
                continue;
            }
            let delay = start - window[0];
            let duration = end - start;
            let source_start = span.source_at(start);
            if source_start < 0.0 && matches!(node.tag.as_str(), "video" | "audio") {
                return Err(format!(
                    "{}: negative media preroll is not ported yet",
                    node.id
                ));
            }
            let source_duration = duration * span.playback_rate;
            let index = *input;
            let media = matches!(node.tag.as_str(), "video" | "audio" | "image");
            if media {
                let path = library
                    .sources
                    .get(&node.id)
                    .ok_or("Unresolved media source")?;
                self.inputs
                    .extend(["-protocol_whitelist".into(), "file,pipe".into()]);
                if node.tag == "image" {
                    self.inputs.extend([
                        "-loop".into(),
                        "1".into(),
                        "-framerate".into(),
                        self.fps.to_string().into(),
                    ]);
                } else {
                    self.inputs
                        .extend(["-ss".into(), source_start.to_string().into()]);
                }
                self.inputs.extend(["-i".into(), path.as_os_str().into()]);
                *input += 1;
                let probe = library.probes.get(&node.id).ok_or("Unprobed source")?;
                if probe.has_audio && !span.muted && node.tag != "image" {
                    let label = format!("audio{}", audio.len());
                    let tempo = tempo_filters(span.playback_rate);
                    let gain = 10f64.powf(span.volume_db / 20.0);
                    if !gain.is_finite() {
                        return Err("Audio gain overflows".into());
                    }
                    let delay_samples = (delay * 48000.0).round() as u64;
                    self.graph += &format!(
                        ";[{index}:a:0]atrim=duration={source_duration},asetpts=PTS-STARTPTS,{tempo},aresample=48000,volume={gain},adelay={delay_samples}S:all=1[{label}]"
                    );
                    audio.push(format!("[{label}]"));
                }
            }
            if node.tag == "audio" {
                continue;
            }
            // Text has no default glyph paint. This is independent from scene
            // background/rect fill in the authoring contract.
            if node.tag == "text" && !node.props.contains_key("color") {
                continue;
            }
            let previous = *visual;
            *visual += 1;
            let current = *visual;
            if node.tag == "text" {
                let text_id = self.text_files.len();
                self.text_files.push(node.text.clone());
                let font = node.string("fontFamily").unwrap_or("Inter");
                if !font
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || " _-".contains(ch))
                {
                    return Err("Unsupported font family spelling".into());
                }
                let size = node.number("fontSize", 16.0)?;
                if size <= 0.0 || size > 8192.0 {
                    return Err("Invalid font size".into());
                }
                let fill = color(node.props.get("color").ok_or("Missing text color")?)?;
                self.graph += &format!(
                    ";[base{previous}]drawtext=textfile=text-{text_id}.txt:expansion=none:font='{font}':fontsize={size}:fontcolor={fill}:alpha={opacity}:x={x}:y={y}:enable='gte(t,{delay})*lt(t,{})'[base{current}]",
                    delay + duration
                );
                continue;
            }
            let default_size = if node.tag == "rect" {
                [100.0, 100.0]
            } else {
                [1920.0, 1080.0]
            };
            let width = dimension(node.number("width", default_size[0])?)?;
            let height = dimension(node.number("height", default_size[1])?)?;
            let layer = format!("layer{current}");
            if node.tag == "rect" {
                let fill = color(node.props.get("fill").ok_or_else(|| {
                    format!(
                        "{}: unpainted rect requires transparent composition support",
                        node.id
                    )
                })?)?;
                self.graph += &format!(
                    ";color=c={fill}:s={width}x{height}:r={}:d={duration},format=rgba,colorchannelmixer=aa={opacity},setpts=PTS+{delay}/TB[{layer}]",
                    self.fps
                );
            } else {
                let fit = match node.string("objectFit").unwrap_or("cover") {
                    "fill" => format!("scale={width}:{height}"),
                    "contain" => format!(
                        "scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black@0"
                    ),
                    "cover" => format!(
                        "scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height}"
                    ),
                    value => return Err(format!("Invalid objectFit: {value}")),
                };
                let clip_duration = if node.tag == "image" {
                    duration
                } else {
                    source_duration
                };
                let rate = if node.tag == "image" {
                    1.0
                } else {
                    span.playback_rate
                };
                self.graph += &format!(
                    ";[{index}:v:0]trim=duration={clip_duration},setpts=(PTS-STARTPTS)/{rate}+{delay}/TB,format=rgba,{fit},colorchannelmixer=aa={opacity}[{layer}]"
                );
            }
            self.graph += &format!(
                ";[base{previous}][{layer}]overlay=x={x}:y={y}:eof_action=pass:repeatlast=0:format=auto:enable='gte(t,{delay})*lt(t,{})'[base{current}]",
                delay + duration
            );
        }
        Ok(())
    }

    fn prepare(&self, project: &Path) -> Result<JobDirectory> {
        let mut directory = JobDirectory::new(project)?;
        directory.write("graph.txt", &self.graph)?;
        for (index, text) in self.text_files.iter().enumerate() {
            directory.write(&format!("text-{index}.txt"), text)?;
        }
        Ok(directory)
    }

    fn command(&self, tools: &Tools, directory: &JobDirectory) -> std::process::Command {
        let mut command = command(&tools.ffmpeg);
        command
            .current_dir(&directory.path)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-filter_complex_threads",
                "1",
            ])
            .args(&self.inputs)
            .args(["-/filter_complex", "graph.txt"]);
        command
    }

    pub fn export(&self, tools: &Tools, project: &Path, output: &Path) -> Result<()> {
        self.export_cancellable(tools, project, output, None)
    }

    pub fn export_cancellable(
        &self,
        tools: &Tools,
        project: &Path,
        output: &Path,
        cancel: Option<&AtomicBool>,
    ) -> Result<()> {
        if !is_mp4(output) {
            return Err("Native export currently supports .mp4; other formats are pending".into());
        }
        if output.exists() {
            return Err("Export destination already exists".into());
        }
        let output = std::path::absolute(output).map_err(|e| e.to_string())?;
        if !output.parent().is_some_and(Path::is_dir) {
            return Err("Export destination directory does not exist".into());
        }
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err("H.264 export requires even dimensions".into());
        }
        let mut directory = self.prepare(project)?;
        let temp = directory.path.join("render.mp4");
        directory.files.push(temp.clone());
        let mut command = self.command(tools, &directory);
        command.args(["-map", "[vout]"]);
        if self.has_audio {
            command.args([
                "-map", "[aout]", "-c:a", "aac", "-ar", "48000", "-b:a", "192k",
            ]);
        }
        command
            .args([
                "-n",
                "-t",
                &self.duration.to_string(),
                "-r",
                &self.fps.to_string(),
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(&temp);
        let result =
            checked_output_cancellable(&mut command, cancel, Duration::from_secs(24 * 3600));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
            return result.map(|_| ());
        }
        // create_new + copy permits another volume while refusing an existing
        // file. Source media is never eligible for overwrite.
        let mut destination = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| e.to_string())?;
        let copied = (|| {
            let mut source = fs::File::open(&temp)?;
            let mut buffer = vec![0; 1024 * 1024];
            loop {
                if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Operation cancelled",
                    ));
                }
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                destination.write_all(&buffer[..count])?;
            }
            destination.sync_all()
        })();
        drop(destination);
        let _ = fs::remove_file(&temp);
        if let Err(error) = copied {
            let _ = fs::remove_file(output);
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn frame(&self, tools: &Tools, project: &Path) -> Result<Vec<u8>> {
        self.frame_cancellable(tools, project, None)
    }

    pub fn frame_cancellable(
        &self,
        tools: &Tools,
        project: &Path,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>> {
        let directory = self.prepare(project)?;
        let mut command = self.command(tools, &directory);
        command.args([
            "-map",
            "[vout]",
            "-frames:v",
            "1",
            "-an",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ]);
        if self.has_audio {
            command.args([
                "-map",
                "[aout]",
                "-t",
                &self.duration.to_string(),
                "-f",
                "null",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ]);
        }
        let output = checked_output_cancellable(&mut command, cancel, Duration::from_secs(120))?;
        if output.stdout.len() != self.width as usize * self.height as usize * 4 {
            return Err("Incomplete composed frame".into());
        }
        Ok(output.stdout)
    }

    pub fn capture(&self, tools: &Tools, project: &Path, output: &Path) -> Result<()> {
        if output.extension().and_then(|e| e.to_str()) != Some("png") {
            return Err("Capture destination must be .png".into());
        }
        let output = std::path::absolute(output).map_err(|e| e.to_string())?;
        let directory = self.prepare(project)?;
        let mut command = self.command(tools, &directory);
        command
            .args(["-n", "-map", "[vout]", "-frames:v", "1", "-an"])
            .arg(output);
        if self.has_audio {
            command.args([
                "-map",
                "[aout]",
                "-t",
                &self.duration.to_string(),
                "-f",
                "null",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ]);
        }
        checked_output(&mut command)?;
        Ok(())
    }
}

pub(crate) fn tempo_filters(mut rate: f64) -> String {
    let mut filters = Vec::new();
    while rate > 2.0 {
        filters.push("atempo=2".into());
        rate /= 2.0;
    }
    while rate < 0.5 {
        filters.push("atempo=0.5".into());
        rate *= 2.0;
    }
    filters.push(format!("atempo={rate}"));
    filters.join(",")
}

pub(crate) struct JobDirectory {
    pub path: PathBuf,
    files: Vec<PathBuf>,
}
impl JobDirectory {
    pub fn new(project: &Path) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let parent = std::path::absolute(project)
            .map_err(|e| e.to_string())?
            .join(".diffusion")
            .join("native-render");
        fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let path = parent.join(format!("{}-{stamp}", std::process::id()));
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            files: Vec::new(),
        })
    }
    pub fn file(&mut self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        self.files.push(path.clone());
        path
    }
    fn write(&mut self, name: &str, text: &str) -> Result<()> {
        let path = self.path.join(name);
        self.files.push(path.clone());
        fs::write(path, text).map_err(|e| e.to_string())
    }
}
impl Drop for JobDirectory {
    fn drop(&mut self) {
        for file in &self.files {
            if let Err(error) = remove_owned_file(file) {
                eprintln!(
                    "Could not remove native render temporary file {}: {error}",
                    file.display()
                );
            }
        }
        if let Err(error) = fs::remove_dir(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not remove native render job directory {}: {error}",
                self.path.display()
            );
        }
    }
}

pub(crate) fn remove_owned_file(path: &Path) -> std::io::Result<()> {
    for attempt in 0..3 {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    unreachable!()
}

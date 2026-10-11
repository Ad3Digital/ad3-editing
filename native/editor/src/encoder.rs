// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Bounded frame pipe. Every exit path reaps the encoder and its writer.
use crate::{
    Result,
    media::{Tools, command},
    render::JobDirectory,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) fn cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err("Operation cancelled".into())
    } else {
        Ok(())
    }
}

pub(crate) struct Encoder {
    child: Child,
    sender: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<std::io::Result<()>>>,
    log: PathBuf,
}
impl Encoder {
    pub fn start(
        tools: &Tools,
        width: u32,
        height: u32,
        fps: f64,
        output: &Path,
        directory: &mut JobDirectory,
        bitrate: Option<u32>,
        codec: Option<&str>,
    ) -> Result<Self> {
        let log = directory.file("encoder.log");
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log)
            .map_err(|e| e.to_string())?;
        let mut encoder_command = command(&tools.ffmpeg);
        encoder_command.args([
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            &fps.to_string(),
            "-i",
            "pipe:0",
            "-an",
        ]);
        configure_video(
            &mut encoder_command,
            crate::media::export_extension(output).ok_or("Unsupported export container")?,
            bitrate,
            codec,
        );
        let mut child = encoder_command
            .arg(output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().expect("Requested piped encoder stdin");
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(1);
        let writer = thread::spawn(move || {
            while let Ok(frame) = receiver.recv() {
                stdin.write_all(&frame)?;
            }
            Ok(())
        });
        Ok(Self {
            child,
            sender: Some(sender),
            writer: Some(writer),
            log,
        })
    }
    pub fn send(&mut self, mut frame: Vec<u8>, cancel: Option<&AtomicBool>) -> Result<()> {
        let started = Instant::now();
        loop {
            cancelled(cancel)?;
            if started.elapsed() > Duration::from_secs(120) {
                return Err("Encoder frame pipe timed out".into());
            }
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("Encoder exited early ({status}): {}", self.error()));
            }
            match self
                .sender
                .as_ref()
                .ok_or("Encoder is closed")?
                .try_send(frame)
            {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(value)) => frame = value,
                Err(TrySendError::Disconnected(_)) => {
                    return Err(format!("Encoder frame writer stopped: {}", self.error()));
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn error(&self) -> String {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .chars()
            .take(4000)
            .collect()
    }
    pub fn finish(&mut self, cancel: Option<&AtomicBool>) -> Result<()> {
        self.sender.take();
        let started = Instant::now();
        let status = loop {
            cancelled(cancel)?;
            if started.elapsed() > Duration::from_secs(120) {
                return Err("Encoder finalization timed out".into());
            }
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            thread::sleep(Duration::from_millis(10));
        };
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| "Encoder writer panicked")?
                .map_err(|e| e.to_string())?;
        }
        if !status.success() {
            return Err(format!("Encoder failed ({status}): {}", self.error()));
        }
        Ok(())
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        self.sender.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

pub(crate) fn publish(source: &Path, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        use std::io::Read;
        let mut source = fs::File::open(source).map_err(|e| e.to_string())?;
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            cancelled(cancel)?;
            let count = source.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            destination
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        destination.sync_all().map_err(|e| e.to_string())
    })();
    drop(destination);
    if let Err(original) = &result
        && let Err(error) = crate::render::remove_owned_file(output)
    {
        return Err(format!(
            "{original}; could not remove incomplete output {}: {error}",
            output.display()
        ));
    }
    result
}

pub(crate) fn configure_video(
    command: &mut std::process::Command,
    extension: &str,
    bitrate: Option<u32>,
    codec: Option<&str>,
) {
    if codec == Some("av1") {
        command.args([
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "6",
            "-row-mt",
            "1",
            "-threads",
            "2",
            "-pix_fmt",
            "yuv420p",
        ]);
        if bitrate.is_none() {
            command.args(["-crf", "30", "-b:v", "0"]);
        }
    } else if extension == "webm" {
        command.args([
            "-c:v",
            if codec == Some("vp8") {
                "libvpx"
            } else {
                "libvpx-vp9"
            },
            "-deadline",
            "good",
            "-cpu-used",
            "4",
            "-pix_fmt",
            "yuv420p",
        ]);
        if bitrate.is_none() {
            command.args(["-crf", "30", "-b:v", "0"]);
        }
    } else {
        let hevc = codec == Some("hevc");
        command.args([
            "-c:v",
            if hevc { "libx265" } else { "libx264" },
            "-preset",
            "fast",
            "-pix_fmt",
            "yuv420p",
        ]);
        if hevc {
            command.args([
                "-tag:v",
                "hvc1",
                "-x265-params",
                "log-level=error:pools=none:frame-threads=1:asm=0",
            ]);
            if extension == "mov" {
                // This Windows FFmpeg build corrupts its heap when libx265
                // encodes directly into MOV. Encode MP4, then remux losslessly.
                command.args(["-f", "mp4"]);
            }
        }
        if bitrate.is_none() {
            command.args(["-crf", if hevc { "28" } else { "18" }]);
        }
    }
    if let Some(bitrate) = bitrate {
        // Use rate control instead of the default constant-quality mode.
        command.args(["-b:v", &bitrate.to_string()]);
    }
}

pub(crate) fn audio_encoder(extension: &str, codec: Option<&str>) -> &'static str {
    if codec == Some("vorbis") {
        return "libvorbis";
    }
    if codec == Some("flac") {
        return "flac";
    }
    if codec == Some("mp3") {
        return "libmp3lame";
    }
    if codec == Some("opus") || codec.is_none() && matches!(extension, "webm" | "ogg") {
        "libopus"
    } else {
        "aac"
    }
}

pub(crate) fn configure_lossless_audio(command: &mut std::process::Command, codec: Option<&str>) {
    if codec == Some("flac") {
        // Preserve the mix to FLAC's 32-bit integer precision; MP4's FLAC
        // muxing is gated as experimental by the installed FFmpeg build.
        command.args(["-sample_fmt", "s32", "-strict", "-2", "-b:a", "0"]);
    }
}

pub(crate) fn publish_video(
    tools: &Tools,
    source: &Path,
    output: &Path,
    directory: &mut JobDirectory,
    cancel: Option<&AtomicBool>,
    codec: Option<&str>,
) -> Result<()> {
    if codec == Some("hevc") && crate::media::export_extension(output) == Some("mov") {
        let remuxed = directory.file("hevc-remux.mov");
        crate::media::checked_output_cancellable(
            command(&tools.ffmpeg)
                .args(["-v", "error", "-nostdin", "-n", "-i"])
                .arg(source)
                .args([
                    "-c",
                    "copy",
                    "-tag:v",
                    "hvc1",
                    "-movflags",
                    "+faststart",
                    "-f",
                    "mov",
                ])
                .arg(&remuxed),
            cancel,
            Duration::from_secs(120),
        )?;
        publish(&remuxed, output, cancel)
    } else {
        publish(source, output, cancel)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn av1_configures_bounded_encoder_and_rate_control() {
        for bitrate in [None, Some(500_000)] {
            let mut command = std::process::Command::new("ffmpeg");
            super::configure_video(&mut command, "webm", bitrate, Some("av1"));
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert!(args.windows(2).any(|pair| pair == ["-c:v", "libaom-av1"]));
            assert!(args.windows(2).any(|pair| pair == ["-threads", "2"]));
            assert_eq!(args.contains(&"-crf"), bitrate.is_none());
            assert!(!args.contains(&"-preset"));
        }
    }
    #[test]
    fn hevc_configures_encoder_tag_and_bounded_threads() {
        let mut command = std::process::Command::new("ffmpeg");
        super::configure_video(&mut command, "mov", Some(500_000), Some("hevc"));
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "libx265"]));
        assert!(args.windows(2).any(|pair| pair == ["-tag:v", "hvc1"]));
        assert!(args.contains(&"log-level=error:pools=none:frame-threads=1:asm=0"));
        assert!(!args.contains(&"-crf"));
    }
    #[test]
    fn saved_bitrate_replaces_constant_quality_for_both_codecs() {
        for extension in ["mp4", "mov", "webm"] {
            let mut command = std::process::Command::new("ffmpeg");
            super::configure_video(&mut command, extension, Some(500_000), None);
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert!(args.windows(2).any(|pair| pair == ["-b:v", "500000"]));
            assert!(!args.contains(&"-crf"));
            let mut defaults = std::process::Command::new("ffmpeg");
            super::configure_video(&mut defaults, extension, None, None);
            assert!(defaults.get_args().any(|arg| arg == "-crf"));
        }
    }
}

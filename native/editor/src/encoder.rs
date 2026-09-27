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
    ) -> Result<Self> {
        let log = directory.file("encoder.log");
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log)
            .map_err(|e| e.to_string())?;
        let mut child = command(&tools.ffmpeg)
            .args([
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
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
            ])
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
        && let Err(error) = crate::render::remove_owned_file(output) {
            return Err(format!(
                "{original}; could not remove incomplete output {}: {error}",
                output.display()
            ));
    }
    result
}

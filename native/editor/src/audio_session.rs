// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Owned PCM producer with one queued block and codec backpressure.
use crate::{
    Result,
    audio_stream::PcmBlock,
    media::{SourceVersion, Tools},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, sync_channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub struct AudioSession {
    tools: Tools,
    origin: f64,
    duration: Option<f64>,
    rate: f64,
    created: Instant,
    base_frame: u64,
    version: SourceVersion,
    next_frame: u64,
    cancel: Arc<AtomicBool>,
    receiver: Option<Receiver<Result<PcmBlock>>>,
    worker: Option<JoinHandle<()>>,
}

impl AudioSession {
    pub fn start(
        tools: Tools,
        source: PathBuf,
        start: f64,
        duration: Option<f64>,
        rate: f64,
    ) -> Result<Self> {
        crate::audio_stream::validate_window(start, duration, rate)?;
        let version = SourceVersion::read(&source)?;
        let retained_tools = tools.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("ad3-audio-session".into())
            .spawn(move || {
                let result = crate::audio_stream::decode(
                    &tools,
                    &source,
                    start,
                    duration,
                    rate,
                    Some(&worker_cancel),
                    |block| {
                        sender
                            .send(Ok(block))
                            .map_err(|_| "Audio receiver closed".to_string())
                    },
                );
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            tools: retained_tools,
            origin: start,
            duration,
            rate,
            created: Instant::now(),
            base_frame: 0,
            version,
            next_frame: 0,
            cancel,
            receiver: Some(receiver),
            worker: Some(worker),
        })
    }

    pub fn next(&mut self, cancel: Option<&AtomicBool>) -> Result<Option<PcmBlock>> {
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                self.stop();
                return Err("Audio session cancelled".into());
            }
            if self.receiver.is_some()
                && self.next_frame > 0
                && self.created.elapsed() >= Duration::from_secs(90)
            {
                let elapsed =
                    self.next_frame as f64 / crate::audio_stream::SAMPLE_RATE as f64 * self.rate;
                let remaining = self.duration.map(|duration| duration - elapsed);
                if remaining.is_some_and(|duration| duration <= 0.0) {
                    self.stop();
                    return Ok(None);
                }
                if SourceVersion::read(&self.version.path)? != self.version {
                    self.stop();
                    return Err("Audio source changed before renewal".into());
                }
                let mut replacement = Self::start(
                    self.tools.clone(),
                    self.version.path.clone(),
                    self.origin + elapsed,
                    remaining,
                    self.rate,
                )?;
                if replacement.version != self.version {
                    self.stop();
                    return Err("Audio source changed during renewal".into());
                }
                replacement.origin = self.origin;
                replacement.duration = self.duration;
                replacement.base_frame = self.next_frame;
                replacement.next_frame = self.next_frame;
                self.stop();
                *self = replacement;
            }
            let Some(receiver) = &self.receiver else {
                return Ok(None);
            };
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(mut block)) => {
                    block.first_frame = block
                        .first_frame
                        .checked_add(self.base_frame)
                        .ok_or("Audio renewal clock overflow")?;
                    let current = match SourceVersion::read(&self.version.path) {
                        Ok(version) => version,
                        Err(error) => {
                            self.stop();
                            return Err(error);
                        }
                    };
                    if current != self.version || block.first_frame != self.next_frame {
                        self.stop();
                        return Err("Audio session source or sample clock changed".into());
                    }
                    self.next_frame += block.samples.len() as u64 / 2;
                    return Ok(Some(block));
                }
                Ok(Err(error)) => {
                    self.stop();
                    return Err(error);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    return Ok(None);
                }
                Err(RecvTimeoutError::Timeout) => continue,
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn expire_for_test(&mut self) {
        self.created = Instant::now() - Duration::from_secs(91);
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for AudioSession {
    fn drop(&mut self) {
        self.stop();
    }
}

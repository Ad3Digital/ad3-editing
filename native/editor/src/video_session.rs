// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! A forward video session with codec backpressure and explicit worker ownership.
use crate::{
    Result,
    media::{Probe, SourceVersion, Tools},
    video_clock::VideoClock,
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

pub struct VideoSession {
    version: SourceVersion,
    next_index: usize,
    memory_bytes: usize,
    created: Instant,
    cancel: Arc<AtomicBool>,
    receiver: Option<Receiver<Result<(usize, tiny_skia::Pixmap)>>>,
    worker: Option<JoinHandle<()>>,
}

impl VideoSession {
    pub fn start(
        tools: Tools,
        source: PathBuf,
        probe: Probe,
        clock: Arc<VideoClock>,
        first: usize,
    ) -> Result<Self> {
        let created = Instant::now();
        let version = SourceVersion::read(&source)?;
        if clock.version.as_ref() != Some(&version) || probe.version.as_ref() != Some(&version) {
            return Err("Video session requires current probe and clock metadata".into());
        }
        if first >= clock.timestamps.len() {
            return Err("Video session starts past its last frame".into());
        }
        let (width, height) = probe.display_dimensions()?;
        let pixels = u64::from(width) * u64::from(height);
        if pixels == 0 || pixels > 16_000_000 {
            return Err("Video session dimensions exceed the raster limit".into());
        }
        let memory_bytes =
            pixels as usize * 16 + (clock.timestamps.len() - first) * std::mem::size_of::<usize>();
        let (sender, receiver) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::Builder::new()
            .name("ad3-video-session".into())
            .spawn(move || {
                let indices = (first..clock.timestamps.len()).collect::<Vec<_>>();
                let result = crate::video_decode::selected_frames_with_clock(
                    &tools,
                    &source,
                    &probe,
                    &indices,
                    [width, height],
                    false,
                    &clock,
                    Some(&worker_cancel),
                    |index, pixels| {
                        sender
                            .send(Ok((index, pixels)))
                            .map_err(|_| "Video session receiver closed".to_string())
                    },
                );
                if let Err(error) = result {
                    let _ = sender.send(Err(error));
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            version,
            next_index: first,
            memory_bytes,
            created,
            cancel,
            receiver: Some(receiver),
            worker: Some(worker),
        })
    }

    pub fn matches(&self, version: &SourceVersion, index: usize) -> bool {
        self.matches_at(version, index, Instant::now())
    }
    pub(crate) fn matches_at(&self, version: &SourceVersion, index: usize, now: Instant) -> bool {
        self.version == *version
            && self.next_index == index
            && self.receiver.is_some()
            && now.saturating_duration_since(self.created) < Duration::from_secs(90)
    }
    pub(crate) fn version(&self) -> &SourceVersion {
        &self.version
    }
    pub(crate) fn memory_bytes(&self) -> usize {
        self.memory_bytes
    }
    pub(crate) fn can_advance(&self, version: &SourceVersion, index: usize) -> bool {
        self.version == *version
            && self.receiver.is_some()
            && self.created.elapsed() < Duration::from_secs(90)
            && index
                .checked_sub(self.next_index)
                .is_some_and(|gap| gap <= 32)
    }

    /// Skip a small forward gap while keeping the same codec and bounded queue.
    pub fn frame_at(
        &mut self,
        index: usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<Option<(usize, tiny_skia::Pixmap)>> {
        if !index
            .checked_sub(self.next_index)
            .is_some_and(|gap| gap <= 32)
        {
            return Err("Video session request needs a new seek".into());
        }
        loop {
            match self.next(cancel)? {
                Some((at, pixels)) if at == index => return Ok(Some((at, pixels))),
                Some((at, _)) if at < index => continue,
                Some(_) => return Err("Video session skipped the requested frame".into()),
                None => return Ok(None),
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn expire_for_test(&mut self) {
        self.created = Instant::now() - Duration::from_secs(91);
    }
    #[cfg(test)]
    pub(crate) fn worker_id(&self) -> Option<std::thread::ThreadId> {
        self.worker.as_ref().map(|worker| worker.thread().id())
    }

    pub fn next(
        &mut self,
        cancel: Option<&AtomicBool>,
    ) -> Result<Option<(usize, tiny_skia::Pixmap)>> {
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                self.stop();
                return Err("Video session cancelled".into());
            }
            let Some(receiver) = &self.receiver else {
                return Ok(None);
            };
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok((index, pixels))) => {
                    let current = match SourceVersion::read(&self.version.path) {
                        Ok(version) => version,
                        Err(error) => {
                            self.stop();
                            return Err(error);
                        }
                    };
                    if index != self.next_index || current != self.version {
                        self.stop();
                        return Err("Video session source or frame order changed".into());
                    }
                    self.next_index += 1;
                    return Ok(Some((index, pixels)));
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

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // Disconnect a backpressured send before joining the codec controller.
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for VideoSession {
    fn drop(&mut self) {
        self.stop();
    }
}

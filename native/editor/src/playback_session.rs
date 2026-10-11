// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Owned Windows playback worker. Device calls never block the UI thread.
use crate::{Result, playback_audio::PlaybackMixer, windows_audio::WindowsOutput};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
pub type PlaybackControls = Vec<(std::path::PathBuf, String, serde_json::Value)>;

pub struct PlaybackSession {
    cancel: Arc<AtomicBool>,
    played: Arc<AtomicU64>,
    result: Receiver<Result<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
    completion: Option<Result<()>>,
    controls: Arc<Mutex<Option<PlaybackControls>>>,
    last_controls: PlaybackControls,
    speed: f64,
}
impl PlaybackSession {
    /// `frames` is the exact remaining workarea length at the 48 kHz clock.
    pub fn start(mixer: PlaybackMixer, frames: u64) -> Result<Self> {
        Self::start_with(move |_| Ok((mixer, None)), frames)
    }
    pub fn start_project(
        document: crate::source::SourceDocument,
        timeline: crate::timing::Timeline,
        project: std::path::PathBuf,
        tools: crate::media::Tools,
        playhead: f64,
    ) -> Result<Self> {
        Self::start_project_with_controls(document, timeline, project, tools, playhead, Vec::new())
    }
    pub fn start_project_with_controls(
        document: crate::source::SourceDocument,
        timeline: crate::timing::Timeline,
        project: std::path::PathBuf,
        tools: crate::media::Tools,
        playhead: f64,
        controls: PlaybackControls,
    ) -> Result<Self> {
        Self::start_project_at_speed(document, timeline, project, tools, playhead, controls, 1.0)
    }
    pub fn start_project_at_speed(
        document: crate::source::SourceDocument,
        timeline: crate::timing::Timeline,
        project: std::path::PathBuf,
        tools: crate::media::Tools,
        playhead: f64,
        controls: PlaybackControls,
        speed: f64,
    ) -> Result<Self> {
        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
            return Err("Invalid forward playback speed".into());
        }
        let workarea = timeline
            .workareas
            .get(&timeline.active)
            .ok_or("Missing playback workarea")?;
        let frames =
            crate::playback_audio::ceil_sample_frame((workarea[1] - playhead) * 48000.0 / speed);
        if !playhead.is_finite()
            || !frames.is_finite()
            || frames <= 0.0
            || frames > (1u64 << 53) as f64
        {
            return Err("Invalid project playback range".into());
        }
        let initial_controls = controls.clone();
        let mut session = Self::start_with(
            move |cancel| {
                let mut renderer = crate::project::ProjectRenderer::new_cancellable(
                    &document,
                    &project,
                    tools.clone(),
                    false,
                    30.0,
                    Some(cancel),
                )?;
                for (file, name, value) in initial_controls {
                    renderer.set_inspect(&file, &name, value)?;
                }
                renderer.sample(playhead, true)?;
                let mixer = PlaybackMixer::from_timeline_at_speed(
                    tools,
                    &renderer.root,
                    &renderer.timeline,
                    &renderer.library,
                    &timeline.active,
                    playhead,
                    speed,
                )?;
                Ok((mixer, Some((renderer, playhead, speed))))
            },
            frames as u64,
        )?;
        session.last_controls = controls;
        session.speed = speed;
        Ok(session)
    }
    /// Latest-value mailbox: UI updates cannot accumulate behind audio work.
    pub fn set_controls(&mut self, controls: PlaybackControls) -> Result<()> {
        if controls != self.last_controls {
            *self
                .controls
                .lock()
                .map_err(|_| "Playback controls lock poisoned")? = Some(controls.clone());
            self.last_controls = controls;
        }
        Ok(())
    }
    fn start_with(
        build: impl FnOnce(
            Arc<AtomicBool>,
        ) -> Result<(
            PlaybackMixer,
            Option<(crate::project::ProjectRenderer, f64, f64)>,
        )> + Send
        + 'static,
        frames: u64,
    ) -> Result<Self> {
        if frames == 0 || frames > 1u64 << 53 {
            return Err("Invalid playback session duration".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let played = Arc::new(AtomicU64::new(0));
        let controls = Arc::new(Mutex::new(None::<PlaybackControls>));
        let worker_controls = controls.clone();
        let (sender, result) = mpsc::channel();
        let worker_cancel = cancel.clone();
        let worker_played = played.clone();
        let worker = std::thread::Builder::new()
            .name("native-audio-playback".into())
            .spawn(move || {
                let outcome = (|| {
                    let (mut mixer, mut evaluation) = build(worker_cancel.clone())?;
                    crate::encoder::cancelled(Some(&worker_cancel))?;
                    let mut device = WindowsOutput::open()?;
                    let mut position = 0;
                    while position < frames || device.queued_blocks() > 0 {
                        if worker_cancel.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        if position == frames || device.queued_blocks() == 3 {
                            device.wait_one(Some(&worker_cancel))?;
                            worker_played.store(device.played_frames(), Ordering::Release);
                            continue;
                        }
                        let count = (frames - position)
                            .min(crate::audio_stream::BLOCK_FRAMES as u64)
                            as usize;
                        if let Some((renderer, start, speed)) = &mut evaluation {
                            if let Some(controls) = worker_controls
                                .lock()
                                .map_err(|_| "Playback controls lock poisoned")?
                                .take()
                            {
                                for (file, name, value) in controls {
                                    renderer.set_inspect(&file, &name, value)?;
                                }
                            }
                            renderer.sample(*start + position as f64 / 48000.0 * *speed, true)?;
                            mixer.reconcile_timeline_at_speed(
                                &renderer.root,
                                &renderer.timeline,
                                &renderer.library,
                                &renderer.timeline.active,
                                *start,
                                *speed,
                            )?;
                        }
                        let packet = mixer.mix(count, Some(&worker_cancel))?;
                        device.enqueue(&packet, Some(&worker_cancel))?;
                        position += count as u64;
                    }
                    Ok(())
                })();
                // User cancellation is normal termination, not an asynchronous error.
                let outcome = if worker_cancel.load(Ordering::Relaxed) {
                    Ok(())
                } else {
                    outcome
                };
                let _ = sender.send(outcome);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            cancel,
            played,
            result,
            worker: Some(worker),
            completion: None,
            controls,
            last_controls: Vec::new(),
            speed: 1.0,
        })
    }
    pub fn played_frames(&self) -> u64 {
        self.played.load(Ordering::Acquire)
    }
    pub fn speed(&self) -> f64 {
        self.speed
    }
    /// Nonblocking completion/error check. Completion remains observable.
    pub fn poll(&mut self) -> Option<Result<()>> {
        if self.completion.is_none() {
            self.completion = match self.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Audio playback worker disconnected".into()))
                }
            };
        }
        if self.completion.is_some()
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.is_finished())
            && let Some(worker) = self.worker.take()
        {
            if worker.join().is_err() {
                self.completion = Some(Err("Audio playback worker panicked".into()));
            }
        }
        if self.worker.is_some() {
            None
        } else {
            self.completion.clone()
        }
    }
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn stop(&mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.poll();
    }
}
impl Drop for PlaybackSession {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asynchronous_stop_returns_before_initialization_finishes() {
        let (release, barrier) = mpsc::channel();
        let (entered, started) = mpsc::channel();
        let mut session = PlaybackSession::start_with(
            move |_| {
                entered.send(()).unwrap();
                barrier.recv().unwrap();
                Ok((
                    PlaybackMixer::new(crate::media::Tools::default(), Vec::new())?,
                    None,
                ))
            },
            48000,
        )
        .unwrap();
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        session.request_stop();
        assert!(session.poll().is_none());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = session.poll() {
                result.unwrap();
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(session.played_frames(), 0);
    }
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn project_evaluation_runs_on_worker_and_completes_workarea() {
        let project = std::env::current_dir().unwrap();
        let document = crate::source::SourceDocument::parse(project.join("playback-test.tsx"),
            r#"export default <stage><scene id="s" active workarea={[0,0.1]}><rect id="r" duration={0.1}/></scene></stage>"#.into()).unwrap();
        let timeline =
            crate::timing::Timeline::resolve(&document.root, &Default::default()).unwrap();
        for speed in [1.0, 2.0, 5.0] {
            let mut session = PlaybackSession::start_project_at_speed(
                document.clone(),
                timeline.clone(),
                project.clone(),
                crate::media::Tools::default(),
                0.0,
                Vec::new(),
                speed,
            )
            .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some(result) = session.poll() {
                    result.unwrap();
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(session.played_frames(), (4800.0 / speed) as u64);
            assert_eq!(session.speed(), speed);
        }
    }
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn workarea_tail_completion_and_early_stop() {
        let mixer = || PlaybackMixer::new(crate::media::Tools::default(), Vec::new()).unwrap();
        assert!(PlaybackSession::start(mixer(), 0).is_err());
        let mut session = PlaybackSession::start(mixer(), 2055).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(result) = session.poll() {
                result.unwrap();
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(session.played_frames(), 2055);
        assert_eq!(session.poll(), Some(Ok(())));
        let mut session = PlaybackSession::start(mixer(), 480000).unwrap();
        session.stop();
        assert!(session.played_frames() < 480000);
        assert_eq!(session.poll(), Some(Ok(())));
    }
}

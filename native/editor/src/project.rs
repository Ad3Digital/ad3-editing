// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Shared live preview/offline entrypoint. All consumers sample the same native
//! lifecycle; an export creates a fresh owner tree and advances it per frame.
use crate::{
    Result,
    audio::{Mixer, SAMPLE_RATE},
    document::Node,
    encoder::{Encoder, cancelled, publish},
    evaluator::LiveProject,
    media::{MediaLibrary, ProbeCache, Tools, checked_output_cancellable, command, is_mp4},
    render::{JobDirectory, RenderPlan},
    source::SourceDocument,
    timing::Timeline,
};
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub struct ProjectRenderer {
    live: Option<LiveProject>,
    cache: ProbeCache,
    pub project: PathBuf,
    pub root: Node,
    pub library: MediaLibrary,
    pub timeline: Timeline,
    pub tools: Tools,
    frame_rate: f64,
}
#[derive(Debug, Serialize)]
pub struct ExportReport {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub fps: f64,
    pub frames: u64,
    pub warnings: Vec<String>,
    pub workarea_duration: f64,
    pub video_duration: f64,
}

impl ProjectRenderer {
    pub fn new(
        document: &SourceDocument,
        project: &Path,
        tools: Tools,
        offline: bool,
    ) -> Result<Self> {
        Self::new_cancellable(document, project, tools, offline, None)
    }
    fn new_cancellable(
        document: &SourceDocument,
        project: &Path,
        tools: Tools,
        offline: bool,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Self> {
        let live = if document.module_sources.is_empty() {
            None
        } else {
            Some(LiveProject::mount_frozen_cancellable(
                &document.path,
                &document.source,
                offline,
                Some(document.module_sources.clone()),
                cancel,
            )?)
        };
        let mut renderer = Self {
            live,
            cache: ProbeCache::default(),
            project: project.into(),
            root: document.root.clone(),
            library: MediaLibrary {
                sources: Default::default(),
                probes: Default::default(),
                durations: Default::default(),
            },
            timeline: Timeline {
                spans: Default::default(),
                workareas: Default::default(),
                active: String::new(),
            },
            tools,
            frame_rate: 30.0,
        };
        renderer.sample(0.0, false)?;
        Ok(renderer)
    }
    pub fn sample(&mut self, time: f64, playing: bool) -> Result<()> {
        if let Some(live) = &mut self.live {
            self.root = live.sample_at_rate(time, playing, self.frame_rate)?;
        }
        self.library =
            MediaLibrary::load_cached(&self.root, &self.project, &self.tools, &mut self.cache)?;
        self.timeline = Timeline::resolve(&self.root, &self.library.durations)?;
        Ok(())
    }
    pub fn frame_plan(
        &mut self,
        scene: &str,
        time: f64,
        fps: f64,
        playing: bool,
    ) -> Result<RenderPlan> {
        self.frame_rate = fps;
        self.sample(time, playing)?;
        self.current_frame_plan(scene, time, fps)
    }
    fn current_frame_plan(&self, scene: &str, time: f64, fps: f64) -> Result<RenderPlan> {
        // A preview/capture only needs pixels. Sound is rendered by the mixer.
        let mut silent = self.timeline.clone();
        for span in silent.spans.values_mut() {
            span.muted = true;
        }
        RenderPlan::build(
            &self.root,
            &silent,
            &self.library,
            scene,
            fps,
            Some([time, time + 1.0 / fps]),
        )
    }
    pub fn set_inspect(&mut self, file: &Path, name: &str, value: serde_json::Value) -> Result<()> {
        self.live
            .as_mut()
            .ok_or("This project has no live controls")?
            .set_inspect(file, name, value, false)
    }
    pub fn warnings(&self) -> Vec<String> {
        self.live
            .as_ref()
            .map(|live| live.warnings.clone())
            .unwrap_or_default()
    }

    pub fn export(
        document: &SourceDocument,
        project: &Path,
        tools: Tools,
        output: &Path,
        fps: f64,
        cancel_token: Option<Arc<AtomicBool>>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<ExportReport> {
        let cancel = cancel_token.as_deref();
        cancelled(cancel)?;
        if !is_mp4(output) {
            return Err("Native export currently supports .mp4".into());
        }
        let output = std::path::absolute(output).map_err(|e| e.to_string())?;
        if output.exists() {
            return Err("Export destination already exists".into());
        }
        if !output.parent().is_some_and(Path::is_dir) {
            return Err("Export destination directory does not exist".into());
        }
        let mut renderer =
            Self::new_cancellable(document, project, tools, true, cancel_token.clone())?;
        let scene = renderer.timeline.active.clone();
        let area = *renderer
            .timeline
            .workareas
            .get(&scene)
            .ok_or("No active scene workarea")?;
        renderer.frame_rate = fps;
        renderer.sample(area[0], true)?;
        let plan = RenderPlan::build(
            &renderer.root,
            &renderer.timeline,
            &renderer.library,
            &scene,
            fps,
            Some(area),
        )?;
        // The original encoder writes only complete output frames.
        let frames = (plan.duration * fps + 1e-9).floor() as u64;
        if frames == 0 {
            return Err("Workarea is shorter than one complete frame at the selected FPS".into());
        }
        let video_duration = frames as f64 / fps;
        let mut report = ExportReport {
            width: plan.width,
            height: plan.height,
            duration: plan.duration,
            fps,
            frames,
            warnings: Vec::new(),
            workarea_duration: plan.duration,
            video_duration,
        };
        if !renderer.live.as_ref().is_some_and(LiveProject::uses_ticker)
            && (video_duration - plan.duration).abs() < 1e-9
        {
            plan.export_cancellable(&renderer.tools, project, &output, cancel)?;
            progress(frames, frames);
        } else {
            if !plan.width.is_multiple_of(2) || !plan.height.is_multiple_of(2) {
                return Err("H.264 export requires even dimensions".into());
            }
            let mut directory = JobDirectory::new(project)?;
            let video = directory.file("video.mp4");
            let audio = directory.file("audio.f32");
            let muxed = directory.file("output.mp4");
            let mut audio_file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&audio)
                .map_err(|e| e.to_string())?;
            let mut mixer = Mixer::default();
            let mut encoder = Encoder::start(
                &renderer.tools,
                plan.width,
                plan.height,
                fps,
                &video,
                &mut directory,
            )?;
            let samples = (plan.duration * SAMPLE_RATE).ceil() as u64;
            let mut first_sample = 0;
            for frame in 0..frames {
                cancelled(cancel)?;
                let time = area[0] + frame as f64 / fps;
                let frame_plan = if frame == 0 {
                    renderer.current_frame_plan(&scene, time, fps)?
                } else {
                    renderer.frame_plan(&scene, time, fps, true)?
                };
                if frame_plan.width != plan.width || frame_plan.height != plan.height {
                    return Err("Scene dimensions changed during export".into());
                }
                let bytes = frame_plan.frame_cancellable(&renderer.tools, project, cancel)?;
                encoder.send(bytes, cancel)?;
                let last_sample = if frame + 1 == frames {
                    samples
                } else {
                    (((frame + 1) as f64 * SAMPLE_RATE / fps).round() as u64).min(samples)
                };
                while first_sample < last_sample {
                    let end = (first_sample + 48_000).min(last_sample);
                    let bytes = mixer.mix(
                        &renderer.root,
                        &renderer.timeline,
                        &renderer.library,
                        &scene,
                        area[0],
                        first_sample..end,
                        &mut directory,
                        &renderer.tools,
                        cancel,
                    )?;
                    audio_file.write_all(&bytes).map_err(|e| e.to_string())?;
                    first_sample = end;
                }
                progress(frame + 1, frames);
            }
            encoder.finish(cancel)?;
            drop(encoder);
            audio_file.sync_all().map_err(|e| e.to_string())?;
            drop(audio_file);
            // Close decoded caches before their temporary files are removed on Windows.
            let has_audio = mixer.has_audio;
            report.duration = if has_audio {
                samples as f64 / SAMPLE_RATE
            } else {
                video_duration
            };
            drop(mixer);
            if has_audio {
                let mut mux = command(&renderer.tools.ffmpeg);
                mux.args(["-v", "error", "-nostdin", "-n", "-i"])
                    .arg(&video)
                    .args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"])
                    .arg(&audio)
                    .args([
                        "-map",
                        "0:v:0",
                        "-map",
                        "1:a:0",
                        "-c:v",
                        "copy",
                        "-c:a",
                        "aac",
                        "-b:a",
                        "192k",
                        "-t",
                        &plan.duration.to_string(),
                        "-movflags",
                        "+faststart",
                    ])
                    .arg(&muxed);
                checked_output_cancellable(&mut mux, cancel, Duration::from_secs(24 * 3600))?;
                publish(&muxed, &output, cancel)?;
            } else {
                publish(&video, &output, cancel)?;
            }
        }
        report.warnings = renderer.warnings();
        Ok(report)
    }
}

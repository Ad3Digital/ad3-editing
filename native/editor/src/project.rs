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
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

enum SceneBackend {
    Native(crate::compositor::Compositor),
    Legacy,
    Blocked(String),
}
struct SceneState {
    // The capture engine allocates its canvas before per-frame motion.
    canvas: [f64; 2],
    backend: Option<SceneBackend>,
}
pub struct ProjectRenderer {
    live: Option<LiveProject>,
    authored_root: Node,
    current_authored: Node,
    motion: crate::motion::Motion,
    has_motion: bool,
    scenes: BTreeMap<String, SceneState>,
    textures: Arc<crate::texture::TextureStore>,
    fonts: crate::fonts::FontBook,
    cancel: Option<Arc<AtomicBool>>,
    offline: bool,
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
        Self::new_cancellable(document, project, tools, offline, 30.0, None)
    }
    fn new_cancellable(
        document: &SourceDocument,
        project: &Path,
        tools: Tools,
        offline: bool,
        fps: f64,
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
                cancel.clone(),
            )?)
        };
        let mut renderer = Self {
            live,
            authored_root: document.root.clone(),
            current_authored: document.root.clone(),
            motion: Default::default(),
            has_motion: false,
            scenes: BTreeMap::new(),
            textures: Arc::new(crate::texture::TextureStore::new(tools.clone())),
            fonts: Default::default(),
            cancel,
            offline,
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
            frame_rate: fps,
        };
        renderer.sample(0.0, false)?;
        Ok(renderer)
    }
    pub fn sample(&mut self, time: f64, playing: bool) -> Result<()> {
        cancelled(self.cancel.as_deref())?;
        let authored = if let Some(live) = &mut self.live {
            live.sample_at_rate(time, playing, self.frame_rate)?
        } else {
            self.authored_root.clone()
        };
        self.library = MediaLibrary::load_cached_cancellable(
            &authored,
            &self.project,
            &self.tools,
            &mut self.cache,
            self.cancel.as_deref(),
        )?;
        let timeline =
            Timeline::resolve_at_rate(&authored, &self.library.durations, self.frame_rate)?;
        self.scenes
            .retain(|id, _| timeline.workareas.contains_key(id));
        for id in timeline.workareas.keys() {
            if !self.scenes.contains_key(id) {
                let scene = authored.find(id).ok_or("Scene missing from document")?;
                let canvas = [
                    crate::timing::round_frame(scene.number("width", 1920.0)?),
                    crate::timing::round_frame(scene.number("height", 1080.0)?),
                ];
                self.scenes.insert(
                    id.clone(),
                    SceneState {
                        canvas,
                        backend: None,
                    },
                );
            }
        }
        self.has_motion = crate::motion::has_motion(&authored);
        self.current_authored = authored.clone();
        self.root = if self.has_motion {
            self.motion
                .sample(&authored, &timeline, time, self.frame_rate)?
        } else {
            authored
        };
        self.timeline = if self.has_motion {
            Timeline::resolve_at_rate(&self.root, &self.library.durations, self.frame_rate)?
        } else {
            timeline
        };
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
    fn current_frame_plan(&mut self, scene: &str, time: f64, fps: f64) -> Result<RenderPlan> {
        let node = self
            .root
            .find(scene)
            .filter(|node| node.tag == "scene")
            .ok_or("No active scene")?;
        let supported = crate::compositor::Compositor::supports(node);
        let state = self
            .scenes
            .get_mut(scene)
            .ok_or("Scene has no capture state")?;
        let backend = state.backend.get_or_insert_with(|| {
            if supported {
                SceneBackend::Native(Default::default())
            } else {
                SceneBackend::Legacy
            }
        });
        if matches!(backend, SceneBackend::Native(_)) && !supported {
            *backend=SceneBackend::Blocked("Scene added unsupported nodes after native vector rendering began; switching compositors during a render requires reopening the project".into());
        }
        if let SceneBackend::Blocked(error) = backend {
            return Err(error.clone());
        }
        if let SceneBackend::Native(compositor) = backend {
            let no_text_motion = Default::default();
            let mut texts = crate::text::prepare(
                node,
                self.current_authored
                    .find(scene)
                    .ok_or("Missing authored scene")?,
                if self.has_motion {
                    &self.motion.text_sizes
                } else {
                    &no_text_motion
                },
            );
            let result = (|| {
                let background = if self.offline {
                    None
                } else {
                    Some(
                        self.root
                            .props
                            .get("background")
                            .map(crate::compositor::packed_color)
                            .transpose()?
                            .unwrap_or(0),
                    )
                };
                compositor.frame_on_canvas(
                    node,
                    self.current_authored
                        .find(scene)
                        .ok_or("Missing authored scene")?,
                    &self.timeline,
                    time,
                    fps,
                    self.has_motion,
                    background,
                    Some(state.canvas),
                    Some(crate::compositor::MediaContext {
                        library: &self.library,
                        store: &self.textures,
                    }),
                    &mut texts,
                    &mut self.fonts,
                    self.cancel.as_deref(),
                )
            })();
            let frame = match result {
                Ok(frame) => frame,
                Err(error) => {
                    if error.transient {
                        return Err(error.message);
                    }
                    // Planning errors describe input/feature incompatibility.
                    // Pixel allocation, cancellation and file/codec errors are
                    // handled later, outside this state transition.
                    let error = format!(
                        "{}; scene rendering requires reopening the project",
                        error.message
                    );
                    *backend = SceneBackend::Blocked(error.clone());
                    return Err(error);
                }
            };
            return Ok(RenderPlan::native_frame(frame, fps));
        }
        // A preview/capture only needs pixels. Sound is rendered by the mixer.
        let mut silent = self.timeline.clone();
        for span in silent.spans.values_mut() {
            span.muted = true;
        }
        let plan = RenderPlan::build(
            &self.root,
            &silent,
            &self.library,
            scene,
            fps,
            Some([time, time + 1.0 / fps]),
        )?;
        Ok(plan)
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
            Self::new_cancellable(document, project, tools, true, fps, cancel_token.clone())?;
        let scene = renderer.timeline.active.clone();
        let area = *renderer
            .timeline
            .workareas
            .get(&scene)
            .ok_or("No active scene workarea")?;
        renderer.frame_rate = fps;
        renderer.sample(area[0], true)?;
        // The capture world already quantized both endpoints to output frames.
        // Subtract their integer frame indices: subtracting seconds first can
        // lose a whole frame through cancellation at large scene times.
        let frames = complete_frames(area, fps)?;
        if frames == 0 {
            return Err("Workarea is shorter than one complete frame at the selected FPS".into());
        }
        let duration = area[1] - area[0];
        if duration > 24.0 * 3600.0 {
            return Err("Native render workarea exceeds the current 24-hour job limit".into());
        }
        let plan = renderer.current_frame_plan(&scene, area[0], fps)?;
        let (width, height) = (plan.width, plan.height);
        let video_duration = frames as f64 / fps;
        let mut report = ExportReport {
            width: plan.width,
            height: plan.height,
            duration,
            fps,
            frames,
            warnings: Vec::new(),
            workarea_duration: duration,
            video_duration,
        };
        if !plan.is_native()
            && !renderer.has_motion
            && !renderer.live.as_ref().is_some_and(LiveProject::uses_ticker)
            && (video_duration - duration).abs() < 1e-9
        {
            let plan = RenderPlan::build(
                &renderer.root,
                &renderer.timeline,
                &renderer.library,
                &scene,
                fps,
                Some(area),
            )?;
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
            let samples = (frames as f64 * SAMPLE_RATE / fps).ceil() as u64;
            let mut first_sample = 0;
            let mut first_plan = Some(plan);
            for frame in 0..frames {
                cancelled(cancel)?;
                let time = area[0] + frame as f64 / fps;
                let frame_plan = if frame == 0 {
                    first_plan.take().expect("First frame plan")
                } else {
                    renderer.frame_plan(&scene, time, fps, true)?
                };
                if frame_plan.width != width || frame_plan.height != height {
                    return Err("Scene dimensions changed during export".into());
                }
                let bytes =
                    frame_plan.export_frame_cancellable(&renderer.tools, project, cancel)?;
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
                        &video_duration.to_string(),
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

fn complete_frames(area: [f64; 2], fps: f64) -> Result<u64> {
    // Leave a wide precision margin for seconds <-> frames round trips. At
    // 2^40 frames their f64 error stays well below a half-frame; larger absolute
    // positions are outside the current native export range. This is separate
    // from the 24-hour duration limit and checked before an encoder is started.
    const MAX_ABSOLUTE_FRAME: f64 = (1_u64 << 40) as f64;
    let frames = area.map(|time| crate::timing::round_frame(time * fps));
    if frames
        .iter()
        .any(|frame| !frame.is_finite() || !(0.0..=MAX_ABSOLUTE_FRAME).contains(frame))
    {
        return Err(
            "Export frame positions exceed the supported absolute timeline range (2^40 frames)"
                .into(),
        );
    }
    Ok((frames[1] - frames[0]) as u64)
}

#[cfg(test)]
mod tests {
    use super::complete_frames;
    #[test]
    fn invalid_media_clock_fails_during_pixels_and_recovers_without_remount() {
        use super::*;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let folder =
            std::env::temp_dir().join(format!("ad3-media-clock-{}-{stamp}", std::process::id()));
        fs::create_dir(&folder).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(folder.join("pixel.png"))
            .unwrap();
        let doc=SourceDocument::parse(folder.join("index.tsx"),r#"export default <stage><scene id="s" active width={2} height={2} workarea={[0,1]}><image id="v" src="pixel.png" width={2} height={2}/></scene></stage>"#.into()).unwrap();
        let tools = Tools {
            ffmpeg: "missing-clock-ffmpeg".into(),
            ffprobe: "missing-clock-ffprobe".into(),
        };
        let mut renderer = ProjectRenderer::new(&doc, &folder, tools.clone(), true).unwrap();
        // Simulate unusable video metadata after the library phase. The source
        // version remains valid, so the pixel request exercises clock validation.
        renderer.root.children[0].children[0].tag = "video".into();
        renderer.library.probes.get_mut("v").unwrap().still = false;
        let invalid = renderer.current_frame_plan("s", 0.0, 30.0).unwrap();
        assert!(
            invalid
                .frame(&tools, &folder)
                .err()
                .unwrap()
                .contains("Invalid video source clock")
        );
        let recovered = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        assert!(recovered.is_native());
        assert_eq!(
            &recovered.frame(&tools, &folder).unwrap()[..4],
            &[255, 0, 0, 255]
        );
        fs::remove_file(folder.join("pixel.png")).unwrap();
        fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn quantized_workareas_count_frames_without_cancellation_at_late_scene_times() {
        for (fps, start, count) in [
            (120.0, 9_960_004.0, 7.0),
            (119.88, 9_950_042.0, 1.0),
            (239.99, 19_919_170.0, 23.0),
            (237.0, 19_671_000.0, 7.0),
        ] {
            let area = [start / fps, (start + count) / fps];
            assert_eq!(complete_frames(area, fps).unwrap(), count as u64);
            assert_ne!(
                ((area[1] - area[0]) * fps + 1e-9).floor() as u64,
                count as u64
            );
        }
    }
    #[test]
    fn export_rejects_absolute_positions_beyond_precise_frame_clock_range() {
        let boundary = (1_u64 << 40) as f64;
        assert_eq!(
            complete_frames([(boundary - 1.0) / 120.0, boundary / 120.0], 120.0).unwrap(),
            1
        );
        assert!(
            complete_frames([(boundary + 100.0) / 30.0, (boundary + 130.0) / 30.0], 30.0).is_err()
        );
        let start = 2_f64.powi(54) / 30.0;
        assert!(complete_frames([start, start + 1.0], 30.0).is_err());
    }
}

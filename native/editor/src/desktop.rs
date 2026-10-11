// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use ad3_editor::{
    Result,
    inspect::Control,
    media::{MediaLibrary, Tools},
    project::ProjectRenderer,
    source::{History, SourceClipboard, SourceDocument},
    timing::Timeline,
    transport::Transport,
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
fn retire_audio(
    current: &mut Option<(u64, u64, f64, ad3_editor::playback_session::PlaybackSession)>,
    retired: &mut Vec<ad3_editor::playback_session::PlaybackSession>,
) {
    if let Some((_, _, _, session)) = current.take() {
        session.request_stop();
        retired.push(session);
    }
}

const BLEND_MODES: &[(&str, &str)] = &[
    ("multiply", "Multiplicar"),
    ("screen", "Tela"),
    ("overlay", "Sobrepor"),
    ("darken", "Escurecer"),
    ("lighten", "Clarear"),
    ("colorDodge", "Subexposição de cor"),
    ("colorBurn", "Superexposição de cor"),
    ("hardLight", "Luz forte"),
    ("softLight", "Luz suave"),
    ("difference", "Diferença"),
    ("exclusion", "Exclusão"),
    ("hue", "Matiz"),
    ("saturation", "Saturação"),
    ("color", "Cor"),
    ("luminosity", "Luminosidade"),
];

const EFFECT_OPTIONS: &[(&str, &str, f64)] = &[
    ("blur", "px", 8.0),
    ("brightness", "amount", 0.8),
    ("contrast", "amount", 0.8),
    ("grayscale", "amount", 0.5),
    ("hueRotate", "deg", 100.0),
    ("invert", "amount", 0.5),
    ("saturate", "amount", 0.8),
    ("sepia", "amount", 0.5),
];

const ANIMATION_OPTIONS: &[(&str, &str)] = &[
    ("fade", "Esmaecer"),
    ("slideLeft", "Deslizar à esquerda"),
    ("slideRight", "Deslizar à direita"),
    ("slideUp", "Deslizar para cima"),
    ("slideDown", "Deslizar para baixo"),
    ("grow", "Crescer"),
    ("shrink", "Encolher"),
    ("spin", "Girar"),
    ("twist", "Torcer"),
    ("blur", "Desfoque"),
    ("appearWord", "Revelar palavras"),
    ("appearChar", "Revelar caracteres"),
    ("scramble", "Embaralhar caracteres"),
    ("gain", "Volume"),
];

fn validate_media_paint_source(path: &Path, video: bool) -> Result<()> {
    let probe = match ad3_editor::texture::probe_image(path, None)? {
        Some(probe) => probe,
        None => Tools::default().probe(path)?,
    };
    if !probe.has_video || probe.still == video {
        return Err(if video {
            "Selecione um arquivo de vídeo."
        } else {
            "Selecione um arquivo de imagem."
        }
        .into());
    }
    probe.display_dimensions()?;
    Ok(())
}

fn animation_label(kind: &str) -> &str {
    ANIMATION_OPTIONS
        .iter()
        .find(|(name, _)| *name == kind)
        .map_or(kind, |(_, label)| *label)
}

fn animation_available(
    node: &ad3_editor::document::Node,
    kind: &str,
    current: Option<&str>,
) -> bool {
    let text_preset = |kind| matches!(kind, "appearWord" | "appearChar" | "scramble");
    if text_preset(kind) {
        return matches!(node.tag.as_str(), "text" | "captions")
            || current.is_some_and(text_preset);
    }
    if kind == "gain" {
        return matches!(node.tag.as_str(), "audio" | "video")
            || node.children.iter().any(|child| child.tag == "videoPaint")
            || current == Some("gain");
    }
    true
}

fn effect_label(kind: &str) -> &str {
    match kind {
        "blur" => "Desfoque",
        "brightness" => "Brilho",
        "contrast" => "Contraste",
        "saturate" => "Saturação",
        "grayscale" => "Escala de cinza",
        "invert" => "Inverter cores",
        "sepia" => "Sépia",
        "hueRotate" => "Rotação de matiz",
        _ => kind,
    }
}

fn inherited_text_property(
    root: &ad3_editor::document::Node,
    id: &str,
    property: &str,
    default: &serde_json::Value,
) -> serde_json::Value {
    fn find(
        node: &ad3_editor::document::Node,
        id: &str,
        property: &str,
        inherited: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let value = if matches!(node.tag.as_str(), "text" | "textRange") {
            node.props
                .get(property)
                .filter(|value| !value.is_null())
                .unwrap_or(inherited)
        } else {
            inherited
        };
        if node.id == id {
            return Some(value.clone());
        }
        node.children
            .iter()
            .find_map(|child| find(child, id, property, value))
    }
    find(root, id, property, default).unwrap_or_else(|| default.clone())
}

fn media_job<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
        .unwrap_or_else(|_| Err("Falha interna ao processar mídia. Tente novamente.".into()))
}

#[derive(Clone)]
struct Snapshot {
    document: SourceDocument,
    timeline: Timeline,
    project: PathBuf,
    controls: Vec<Control>,
}
impl Snapshot {
    fn load(path: &Path, tools: &Tools) -> Result<Self> {
        let document = SourceDocument::open(path)?;
        let project = if path.is_dir() {
            std::path::absolute(path).map_err(|e| e.to_string())?
        } else {
            document
                .path
                .parent()
                .ok_or("Missing project directory")?
                .into()
        };
        Self::from_document(document, project, tools)
    }
    fn from_document(document: SourceDocument, project: PathBuf, tools: &Tools) -> Result<Self> {
        let library = MediaLibrary::load(&document.root, &project, tools)?;
        let offsets = ad3_editor::audio_sync::AudioSyncCache::default().offsets(
            &document.root,
            &library,
            tools,
            None,
        )?;
        let timeline = if document.root.tag == "stage"
            && !document
                .root
                .children
                .iter()
                .any(|node| node.tag == "scene")
        {
            Timeline {
                active: String::new(),
                spans: BTreeMap::new(),
                workareas: BTreeMap::new(),
            }
        } else {
            Timeline::resolve_with_media_sync(&document.root, &library, 30.0, &offsets)?
        };
        let controls = document.controls()?;
        Ok(Self {
            document,
            timeline,
            project,
            controls,
        })
    }
}
struct FrameRequest {
    snapshot: Arc<Snapshot>,
    generation: u64,
    time: f64,
    playing: bool,
    full_resolution: bool,
    controls: Vec<(PathBuf, String, serde_json::Value)>,
    runtime_skews: BTreeMap<String, [f64; 2]>,
}
struct FrameReply {
    generation: u64,
    time: f64,
    result: Result<(
        egui::ColorImage,
        BTreeMap<String, ad3_editor::compositor::EditGeometry>,
    )>,
}
enum Event {
    Opened(Box<Result<Snapshot>>),
    Exported(Result<PathBuf>),
    ExportProgress(u64, u64),
    Captured(Result<PathBuf>),
}

#[cfg(debug_assertions)]
struct Smoke {
    output: PathBuf,
    started: Instant,
    requested: bool,
}

const COLORS: [(&str, &str); 8] = [
    ("Amarelo", "#F59E0B"),
    ("Azul", "#3B82F6"),
    ("Verde", "#10B981"),
    ("Rosa", "#EC4899"),
    ("Roxo", "#A855F7"),
    ("Laranja", "#F97316"),
    ("Ciano", "#06B6D4"),
    ("Vermelho", "#EF4444"),
];

#[derive(Clone)]
struct ShadowDraft {
    rgb: [u8; 3],
    opacity: f64,
    blur: f64,
    offset: [f64; 2],
}

#[derive(Clone)]
struct StrokeDraft {
    rgb: [u8; 3],
    width: f64,
    opacity: f64,
    miter_limit: f64,
}

#[derive(Clone)]
struct PaintDraft {
    rgb: [u8; 3],
    opacity: f64,
}

struct Editor {
    project_path: String,
    export_path: String,
    export_fps: f64,
    export_resolution: u32,
    export_video_bitrate: u32,
    export_audio: bool,
    export_video: bool,
    export_video_codec: String,
    export_audio_codec: String,
    export_audio_channels: u32,
    export_audio_sample_rate: u32,
    export_audio_bitrate: u32,
    snapshot: Option<Arc<Snapshot>>,
    marker_config: Option<ad3_editor::project_config::ProjectConfig>,
    marker_color: String,
    marker_drag: Option<(usize, f64)>,
    keyframe_drag: Option<(String, f64)>,
    keyframe_drag_delta: f64,
    selected_keyframes: Vec<String>,
    marker_hold: Option<(String, f64)>,
    marker_picker: bool,
    history: Option<History>,
    transport: Option<Transport>,
    selected: String,
    selection: Vec<String>,
    gap_selection: Option<(String, [f64; 2])>,
    marquee: Option<(Pos2, Vec<String>)>,
    property: String,
    value: String,
    easing_drafts: BTreeMap<String, String>,
    keyframe_time_drafts: BTreeMap<String, f64>,
    controls_draft: Vec<Control>,
    status: String,
    texture: Option<egui::TextureHandle>,
    frame_geometry: Option<(f64, BTreeMap<String, ad3_editor::compositor::EditGeometry>)>,
    runtime_skews: BTreeMap<String, [f64; 2]>,
    skew_undo: Vec<BTreeMap<String, [f64; 2]>>,
    skew_redo: Vec<BTreeMap<String, [f64; 2]>>,
    pending_ungroup: Option<(Vec<String>, bool)>,
    pending_wrap_scene: Option<Vec<String>>,
    clipboard: Option<SourceClipboard>,
    frames: mpsc::Receiver<FrameReply>,
    request: mpsc::SyncSender<FrameRequest>,
    events: mpsc::Receiver<Event>,
    event_sender: mpsc::Sender<Event>,
    busy: bool,
    loading: bool,
    exporting: bool,
    export_cancel: Arc<AtomicBool>,
    export_job: Option<thread::JoinHandle<()>>,
    export_started: Option<Instant>,
    export_progress: (u64, u64),
    capture_job: Option<thread::JoinHandle<()>>,
    capturing: bool,
    workarea_draft: [f64; 2],
    scene_name_draft: String,
    text_draft: Option<(String, String)>,
    clip_name_draft: Option<(String, String)>,
    media_source_draft: Option<(String, String)>,
    volume_draft: Option<(String, f64, f64)>,
    opacity_draft: Option<(String, f64, f64)>,
    font_families: Option<Vec<String>>,
    font_book: ad3_editor::fonts::FontBook,
    font_weights: BTreeMap<String, Vec<u16>>,
    font_search: String,
    animation_out: bool,
    animation_duration: f64,
    animation_delay: f64,
    animation_drafts: BTreeMap<String, (bool, f64, f64)>,
    effect_drafts: BTreeMap<String, (f64, f64)>,
    shadow_drafts: BTreeMap<String, (f64, ShadowDraft)>,
    stroke_drafts: BTreeMap<String, (f64, StrokeDraft)>,
    paint_drafts: BTreeMap<String, (f64, PaintDraft)>,
    stop_drafts: BTreeMap<String, (f64, PaintDraft, f64)>,
    gradient_geometry_drafts: BTreeMap<String, (f64, [f64; 5])>,
    media_paint_source_drafts: BTreeMap<String, String>,
    media_path: String,
    capture_cancel: Arc<AtomicBool>,
    dirty_preview: bool,
    generation: u64,
    last_tick: Instant,
    #[cfg(windows)]
    audio_playback: Option<(u64, u64, f64, ad3_editor::playback_session::PlaybackSession)>,
    #[cfg(windows)]
    audio_retired: Vec<ad3_editor::playback_session::PlaybackSession>,
    zoom: f32,
    preview_actual_size: bool,
    drag_start: Option<f64>,
    #[cfg(test)]
    hit_regions: Vec<(String, Rect)>,
    #[cfg(test)]
    marker_hits: Vec<Rect>,
    #[cfg(test)]
    keyframe_hits: Vec<(String, Rect)>,
    #[cfg(debug_assertions)]
    smoke: Option<Smoke>,
}

impl Editor {
    fn new(context: &eframe::CreationContext<'_>, path: String) -> Self {
        Self::create(&context.egui_ctx, path)
    }

    fn create(context: &egui::Context, path: String) -> Self {
        context.set_visuals(egui::Visuals::dark());
        let (tx, rx) = mpsc::sync_channel::<FrameRequest>(1);
        let (result_tx, result_rx) = mpsc::channel();
        let repaint = context.clone();
        thread::spawn(move || {
            let tools = Tools::default();
            let mut mounted: Option<(Arc<Snapshot>, ProjectRenderer)> = None;
            while let Ok(request) = rx.recv() {
                let snapshot = &request.snapshot;
                let result = media_job(|| {
                    if !mounted
                        .as_ref()
                        .is_some_and(|(current, _)| Arc::ptr_eq(current, snapshot))
                    {
                        let renderer = ProjectRenderer::new(
                            &snapshot.document,
                            &snapshot.project,
                            tools.clone(),
                            false,
                        )?;
                        mounted = Some((snapshot.clone(), renderer));
                    }
                    let renderer = &mut mounted.as_mut().unwrap().1;
                    renderer.set_runtime_skews(request.runtime_skews)?;
                    for (file, name, value) in request.controls {
                        renderer.set_inspect(&file, &name, value)?;
                    }
                    let mut plan = renderer.frame_plan(
                        &snapshot.timeline.active,
                        request.time,
                        30.0,
                        request.playing,
                    )?;
                    if !request.full_resolution {
                        plan.resize_output(360)?;
                    }
                    let bytes = plan.frame(&tools, &snapshot.project)?;
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [plan.width as usize, plan.height as usize],
                        &bytes,
                    );
                    Ok((
                        image,
                        renderer
                            .edit_geometries(&snapshot.timeline.active)
                            .cloned()
                            .unwrap_or_default(),
                    ))
                });
                if result.is_err() {
                    // Rebuild the renderer after failure rather than reusing
                    // partially sampled state on the next preview request.
                    mounted = None;
                }
                if result_tx
                    .send(FrameReply {
                        generation: request.generation,
                        time: request.time,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                repaint.request_repaint();
            }
        });
        let (event_sender, events) = mpsc::channel();
        let mut editor = Self {
            project_path: path,
            export_path: String::new(),
            export_fps: 30.0,
            export_resolution: 1080,
            export_video_bitrate: 10_000_000,
            export_audio: true,
            export_video: true,
            export_video_codec: String::new(),
            export_audio_codec: String::new(),
            export_audio_channels: 2,
            export_audio_sample_rate: 48_000,
            export_audio_bitrate: 128_000,
            snapshot: None,
            marker_config: None,
            marker_color: "#EC4899".into(),
            marker_drag: None,
            keyframe_drag: None,
            keyframe_drag_delta: 0.0,
            selected_keyframes: Vec::new(),
            marker_hold: None,
            marker_picker: false,
            history: None,
            transport: None,
            selected: String::new(),
            selection: Vec::new(),
            gap_selection: None,
            marquee: None,
            property: "volume".into(),
            value: "0".into(),
            easing_drafts: BTreeMap::new(),
            keyframe_time_drafts: BTreeMap::new(),
            controls_draft: Vec::new(),
            status: "Abra a pasta de um projeto AD3.".into(),
            texture: None,
            frame_geometry: None,
            runtime_skews: BTreeMap::new(),
            skew_undo: Vec::new(),
            skew_redo: Vec::new(),
            pending_ungroup: None,
            pending_wrap_scene: None,
            clipboard: None,
            frames: result_rx,
            request: tx,
            events,
            event_sender,
            busy: false,
            loading: false,
            exporting: false,
            export_cancel: Arc::new(AtomicBool::new(false)),
            export_job: None,
            export_started: None,
            export_progress: (0, 0),
            capture_job: None,
            capturing: false,
            workarea_draft: [0.0, 5.0],
            scene_name_draft: String::new(),
            text_draft: None,
            clip_name_draft: None,
            media_source_draft: None,
            volume_draft: None,
            opacity_draft: None,
            font_families: None,
            font_book: ad3_editor::fonts::FontBook::default(),
            font_weights: BTreeMap::new(),
            font_search: String::new(),
            animation_out: false,
            animation_duration: 1.0,
            animation_delay: 0.0,
            animation_drafts: BTreeMap::new(),
            effect_drafts: BTreeMap::new(),
            shadow_drafts: BTreeMap::new(),
            stroke_drafts: BTreeMap::new(),
            paint_drafts: BTreeMap::new(),
            stop_drafts: BTreeMap::new(),
            gradient_geometry_drafts: BTreeMap::new(),
            media_paint_source_drafts: BTreeMap::new(),
            media_path: String::new(),
            capture_cancel: Arc::new(AtomicBool::new(false)),
            dirty_preview: false,
            generation: 0,
            last_tick: Instant::now(),
            #[cfg(windows)]
            audio_playback: None,
            #[cfg(windows)]
            audio_retired: Vec::new(),
            zoom: 80.0,
            preview_actual_size: false,
            drag_start: None,
            #[cfg(test)]
            hit_regions: Vec::new(),
            #[cfg(test)]
            marker_hits: Vec::new(),
            #[cfg(test)]
            keyframe_hits: Vec::new(),
            #[cfg(debug_assertions)]
            smoke: std::env::var_os("AD3_NATIVE_SMOKE_SCREENSHOT").map(|path| Smoke {
                output: path.into(),
                started: Instant::now(),
                requested: false,
            }),
        };
        if !editor.project_path.is_empty() {
            editor.open(context.clone());
        }
        editor
    }

    fn open(&mut self, context: egui::Context) {
        if self.loading {
            return;
        }
        let path = PathBuf::from(&self.project_path);
        let tx = self.event_sender.clone();
        self.loading = true;
        self.status = "Abrindo projeto e lendo mídias…".into();
        thread::spawn(move || {
            let _ = tx.send(Event::Opened(Box::new(Snapshot::load(
                &path,
                &Tools::default(),
            ))));
            context.request_repaint();
        });
    }

    fn invalidate(&mut self) {
        self.pending_ungroup = None;
        self.pending_wrap_scene = None;
        self.frame_geometry = None;
        self.generation += 1;
        self.dirty_preview = true;
    }

    fn apply_export_preferences(&mut self, snapshot: &Snapshot) {
        let settings = self.marker_config.as_ref().map_or_else(
            || serde_json::json!({}),
            |config| config.export_settings(&snapshot.timeline.active),
        );
        self.export_fps = settings
            .get("video")
            .and_then(|video| video.get("fps"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(30.0);
        self.export_audio = settings
            .get("audio")
            .and_then(|audio| audio.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        self.export_video = settings
            .get("video")
            .and_then(|video| video.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        self.export_resolution = settings
            .get("video")
            .and_then(|video| video.get("resolution"))
            .and_then(serde_json::Value::as_f64)
            .filter(|height| (1.0..=8192.0).contains(height) && height.fract() == 0.0)
            .map(|height| height as u32)
            .unwrap_or(1080);
        self.export_video_bitrate = settings
            .get("video")
            .and_then(|video| video.get("bitrate"))
            .and_then(serde_json::Value::as_f64)
            .filter(|bitrate| (1.0..=u32::MAX as f64).contains(bitrate))
            .map(|bitrate| bitrate.round() as u32)
            .unwrap_or(10_000_000);
        self.export_video_codec = settings
            .get("video")
            .and_then(|video| video.get("codec"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .into();
        self.export_audio_codec = settings
            .get("audio")
            .and_then(|audio| audio.get("codec"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .into();
        let sample_rate = settings
            .get("audio")
            .and_then(|audio| audio.get("bitrate"))
            .and_then(serde_json::Value::as_f64);
        self.export_audio_bitrate = sample_rate
            .filter(|bitrate| (1.0..=u32::MAX as f64).contains(bitrate))
            .map(|bitrate| bitrate.round() as u32)
            .unwrap_or(128_000);
        let sample_rate = settings
            .get("audio")
            .and_then(|audio| audio.get("sampleRate"))
            .and_then(serde_json::Value::as_f64);
        self.export_audio_sample_rate = sample_rate
            .filter(|rate| rate.fract() == 0.0 && (7350.0..=192000.0).contains(rate))
            .map(|rate| rate as u32)
            .unwrap_or(48_000);
        let channels = settings
            .get("audio")
            .and_then(|audio| audio.get("numberOfChannels"))
            .and_then(serde_json::Value::as_f64);
        self.export_audio_channels = match channels {
            Some(1.0) => 1,
            _ => 2,
        };
        let extension = settings
            .get("format")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("mp4");
        self.export_path = snapshot
            .project
            .join(format!("export.{extension}"))
            .to_string_lossy()
            .into_owned();
    }

    fn replace_snapshot(&mut self, snapshot: Snapshot) {
        self.easing_drafts.clear();
        self.keyframe_time_drafts.clear();
        self.marker_drag = None;
        self.keyframe_drag = None;
        self.selected_keyframes.clear();
        self.marker_hold = None;
        self.marker_picker = false;
        self.marker_config =
            ad3_editor::project_config::ProjectConfig::load(&snapshot.project).ok();
        self.marquee = None;
        self.gap_selection = None;
        let same_scene = self.snapshot.as_ref().is_some_and(|old| {
            old.timeline.active == snapshot.timeline.active && old.project == snapshot.project
        });
        if !same_scene {
            self.apply_export_preferences(&snapshot);
        }
        let previous = self.transport.take();
        self.transport = snapshot
            .timeline
            .workareas
            .get(&snapshot.timeline.active)
            .and_then(|area| Transport::new(*area).ok());
        if self.transport.is_none() {
            self.texture = None;
            self.frame_geometry = None;
        }
        if let Some(transport) = &self.transport {
            self.workarea_draft = transport.workarea;
        }
        if same_scene && let (Some(previous), Some(current)) = (previous, &mut self.transport) {
            let _ = current.seek(
                previous
                    .time
                    .clamp(current.workarea[0], current.workarea[1]),
            );
            current.looping = previous.looping;
        }
        self.controls_draft = snapshot.controls.clone();
        self.text_draft = None;
        self.animation_drafts.clear();
        self.effect_drafts.clear();
        self.shadow_drafts.clear();
        self.stroke_drafts.clear();
        self.paint_drafts.clear();
        self.stop_drafts.clear();
        self.gradient_geometry_drafts.clear();
        self.media_paint_source_drafts.clear();
        self.clip_name_draft = None;
        self.media_source_draft = None;
        self.volume_draft = None;
        self.opacity_draft = None;
        self.scene_name_draft = snapshot
            .document
            .root
            .find(&snapshot.timeline.active)
            .and_then(|node| node.string("name"))
            .unwrap_or("")
            .into();
        self.selection.retain(|id| {
            snapshot
                .timeline
                .spans
                .get(id)
                .is_some_and(|span| span.scene_id == snapshot.timeline.active)
        });
        self.selected = self.selection.last().cloned().unwrap_or_default();
        self.snapshot = Some(Arc::new(snapshot));
    }

    fn commit(&mut self, next: Result<SourceDocument>) -> bool {
        self.commit_with_skews(next, self.runtime_skews.clone())
    }

    fn commit_with_skews(
        &mut self,
        next: Result<SourceDocument>,
        mut skews: BTreeMap<String, [f64; 2]>,
    ) -> bool {
        let result = (|| {
            let next = next?;
            if skews.values().flatten().any(|value| !value.is_finite()) {
                return Err("Runtime skew must be finite".into());
            }
            skews.retain(|id, _| next.root.find(id).is_some());
            let previous = self.snapshot.as_ref().ok_or("No project")?;
            if next.source == previous.document.source
                && next.path == previous.document.path
                && skews == self.runtime_skews
            {
                return Ok(false);
            }
            let snapshot =
                Snapshot::from_document(next.clone(), previous.project.clone(), &Tools::default())?;
            self.history.as_mut().ok_or("No history")?.commit(next)?;
            self.skew_undo.push(self.runtime_skews.clone());
            self.skew_redo.clear();
            self.replace_snapshot(snapshot);
            self.runtime_skews = skews;
            Ok(true)
        })();
        match result {
            Ok(changed) => {
                if changed {
                    self.status = "Salvo no projeto.".into();
                    self.invalidate();
                } else {
                    self.status = "Sem alterações no projeto.".into();
                }
                true
            }
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn history(&mut self, redo: bool) {
        let result = (|| {
            let history = self.history.as_mut().ok_or("No project")?;
            let snapshot = self.snapshot.as_ref().ok_or("No project")?;
            let next = history.restore(redo, |document| {
                Snapshot::from_document(
                    document.clone(),
                    snapshot.project.clone(),
                    &Tools::default(),
                )
            })?;
            let (from, to) = if redo {
                (&mut self.skew_redo, &mut self.skew_undo)
            } else {
                (&mut self.skew_undo, &mut self.skew_redo)
            };
            to.push(self.runtime_skews.clone());
            self.runtime_skews = from.pop().unwrap_or_default();
            self.replace_snapshot(next);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.status = if redo {
                    "Edição refeita."
                } else {
                    "Edição desfeita."
                }
                .into();
                self.invalidate();
            }
            Err(error) => self.status = error,
        }
    }

    fn split(&mut self) {
        if let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) {
            match snapshot.document.split_at_playhead(
                &snapshot.timeline,
                &snapshot.timeline.active,
                &self.selection,
                transport.time,
            ) {
                Ok((next, units)) if !units.is_empty() => {
                    if self.commit(Ok(next)) {
                        self.selected = units[0].clone();
                        self.selection = units;
                    }
                }
                Ok(_) => {}
                Err(error) => self.status = error,
            }
        }
    }

    fn ungroup_selection(&mut self) {
        self.dissolve_selection(false);
    }

    fn copy_selection(&mut self) -> bool {
        let Some(snapshot) = &self.snapshot else {
            return false;
        };
        match snapshot.document.copy_selection(&self.selection) {
            Ok(Some(clipboard)) => {
                self.clipboard = Some(clipboard);
                true
            }
            Ok(None) => true,
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn paste_selection(&mut self) {
        let (Some(snapshot), Some(clipboard)) = (self.snapshot.clone(), self.clipboard.clone())
        else {
            return;
        };
        let document = &snapshot.document;
        let (parent, anchor) =
            if let Some(selected) = self.selection.first().and_then(|id| document.root.find(id)) {
                if selected.is_container() {
                    (selected.id.clone(), None)
                } else {
                    let Some(parent) =
                        document.root.nodes().into_iter().find(|parent| {
                            parent.children.iter().any(|child| child.id == selected.id)
                        })
                    else {
                        return;
                    };
                    let index = parent
                        .children
                        .iter()
                        .position(|child| child.id == selected.id)
                        .unwrap();
                    (
                        parent.id.clone(),
                        parent.children.get(index + 1).map(|child| child.id.clone()),
                    )
                }
            } else {
                (snapshot.timeline.active.clone(), None)
            };
        match document.paste_selection(&clipboard, &parent, anchor.as_deref()) {
            Ok((document, copies)) => {
                if !copies.is_empty() && self.commit(Ok(document)) {
                    self.selected = copies[0].clone();
                    self.selection = copies;
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn wrap_scene_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let (Some(snapshot), Some(transport)) = (self.snapshot.clone(), &mut self.transport) else {
            return;
        };
        transport.playing = false;
        let at = transport.time;
        let Some((_, geometry)) = self
            .frame_geometry
            .clone()
            .filter(|(time, _)| (*time - at).abs() < 1e-9)
        else {
            self.invalidate();
            self.pending_wrap_scene = Some(self.selection.clone());
            self.status = "Preparando o frame para criar a cena…".into();
            return;
        };
        match snapshot
            .document
            .wrap_scene(&snapshot.timeline, &self.selection, at, &geometry)
        {
            Ok((document, Some(id))) => {
                if self.commit(Ok(document)) {
                    self.selected = id.clone();
                    self.selection = vec![id];
                }
            }
            Ok(_) => {}
            Err(error) => self.status = error,
        }
    }

    fn dissolve_selection(&mut self, sequences_only: bool) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        if !self.selection.iter().any(|id| {
            snapshot.document.root.find(id).is_some_and(|node| {
                if sequences_only {
                    node.tag == "sequence"
                } else {
                    matches!(node.tag.as_str(), "group" | "sequence" | "scene")
                }
            })
        }) {
            return;
        }
        let Some(transport) = &mut self.transport else {
            return;
        };
        transport.playing = false;
        let at = transport.time;
        let Some((_, mut geometry)) = self
            .frame_geometry
            .clone()
            .filter(|(time, _)| (*time - at).abs() < 1e-9)
        else {
            self.invalidate();
            self.pending_ungroup = Some((self.selection.clone(), sequences_only));
            self.status = "Preparando o frame para desagrupar…".into();
            return;
        };
        let result = (|| {
            let mut current = (*snapshot).clone();
            let mut released = Vec::new();
            let mut skews = self.runtime_skews.clone();
            for id in &self.selection {
                if !current.document.root.find(id).is_some_and(|node| {
                    if sequences_only {
                        node.tag == "sequence"
                    } else {
                        matches!(node.tag.as_str(), "group" | "sequence" | "scene")
                    }
                }) {
                    continue;
                }
                let (document, children, baked) =
                    current
                        .document
                        .dissolve_container(&current.timeline, id, at, &geometry)?;
                for (id, values) in baked {
                    if let Some(item) = geometry.get_mut(&id) {
                        item.transform = values;
                        item.local = values.local_matrix();
                    }
                    if values.skew == [0.0, 0.0] {
                        skews.remove(&id);
                    } else {
                        skews.insert(id, values.skew);
                    }
                }
                released.extend(children);
                current =
                    Snapshot::from_document(document, snapshot.project.clone(), &Tools::default())?;
            }
            released.retain(|id| current.document.root.find(id).is_some());
            Ok::<_, String>((current.document, released, skews))
        })();
        match result {
            Ok((document, released, skews)) => {
                if self.commit_with_skews(Ok(document), skews) {
                    self.selected = released.first().cloned().unwrap_or_default();
                    self.selection = released;
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn delete_selection(&mut self) {
        if !self.selected_keyframes.is_empty()
            && let Some(snapshot) = &self.snapshot
        {
            let next = self
                .selected_keyframes
                .iter()
                .try_fold(snapshot.document.clone(), |document, id| {
                    document.remove_keyframe(id)
                });
            self.commit(next);
            return;
        }
        if self.gap_selection.is_some() {
            self.close_gap();
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut ids = self.selection.clone();
        ids.sort_by_key(|id| {
            std::cmp::Reverse(
                snapshot
                    .document
                    .root
                    .find(id)
                    .map_or(0, |node| node.location.element.len()),
            )
        });
        let next = ids
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                if document.root.find(id).is_some() {
                    document.remove(id)
                } else {
                    Ok(document)
                }
            });
        if self.commit(next) {
            self.selected.clear();
            self.selection.clear();
        }
    }

    fn select_forward(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(scene) = snapshot.document.root.find(&snapshot.timeline.active) else {
            return;
        };
        fn collect<'a>(
            parent: &'a ad3_editor::document::Node,
            units: &mut Vec<&'a ad3_editor::document::Node>,
        ) {
            for child in &parent.children {
                if child.tag == "sequence" {
                    collect(child, units);
                } else {
                    units.push(child);
                }
            }
        }
        let mut units = Vec::new();
        collect(scene, &mut units);
        let anchor = units
            .iter()
            .filter(|node| self.selection.contains(&node.id))
            .filter_map(|node| snapshot.timeline.spans.get(&node.id).map(|span| span.start))
            .reduce(f64::min)
            .unwrap_or_else(|| {
                self.transport
                    .as_ref()
                    .map_or(0.0, |transport| transport.time)
            });
        self.selection = units
            .into_iter()
            .filter(|node| {
                snapshot
                    .timeline
                    .spans
                    .get(&node.id)
                    .is_some_and(|span| span.start >= anchor)
            })
            .map(|node| node.id.clone())
            .collect();
        self.selected = self.selection.last().cloned().unwrap_or_default();
        self.gap_selection = None;
        self.marquee = None;
    }

    fn select_children(&mut self) {
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        fn collect(
            parent: &ad3_editor::document::Node,
            timeline: &Timeline,
            time: f64,
            result: &mut Vec<String>,
        ) {
            for child in &parent.children {
                if !timeline
                    .spans
                    .get(&child.id)
                    .is_some_and(|span| span.visible(time))
                {
                    continue;
                }
                if child.tag == "sequence" {
                    collect(child, timeline, time, result);
                } else if !result.contains(&child.id) {
                    result.push(child.id.clone());
                }
            }
        }
        let mut children = Vec::new();
        for id in &self.selection {
            if let Some(parent) = snapshot.document.root.find(id) {
                collect(parent, &snapshot.timeline, transport.time, &mut children);
            }
        }
        if !children.is_empty() {
            self.selection = children;
            self.selected = self.selection.last().cloned().unwrap_or_default();
            self.gap_selection = None;
            self.marquee = None;
        }
    }

    fn select_parents(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let nodes = snapshot.document.root.nodes();
        let mut parents = Vec::new();
        for id in &self.selection {
            let mut current = id.as_str();
            while let Some(parent) = nodes
                .iter()
                .find(|node| node.children.iter().any(|child| child.id == current))
            {
                if parent.tag == "stage" {
                    break;
                }
                if parent.tag == "sequence" {
                    current = &parent.id;
                    continue;
                }
                if !parents.contains(&parent.id) {
                    parents.push(parent.id.clone());
                }
                break;
            }
        }
        if !parents.is_empty() {
            self.selection = parents;
            self.selected = self.selection.last().cloned().unwrap_or_default();
            self.gap_selection = None;
            self.marquee = None;
        }
    }

    #[cfg(windows)]
    fn tick_audio(&mut self) {
        self.audio_retired
            .retain_mut(|session| session.poll().is_none());
        let Some(transport) = &mut self.transport else {
            retire_audio(&mut self.audio_playback, &mut self.audio_retired);
            return;
        };
        if !transport.playing || transport.speed <= 0.0 {
            retire_audio(&mut self.audio_playback, &mut self.audio_retired);
            return;
        }
        if self
            .audio_playback
            .as_ref()
            .is_some_and(|(generation, document, _, _)| {
                *generation != transport.generation || *document != self.generation
            })
        {
            retire_audio(&mut self.audio_playback, &mut self.audio_retired);
        }
        if self.audio_playback.is_none() {
            if self.audio_retired.len() >= 4 {
                return;
            }
            let Some(snapshot) = &self.snapshot else {
                return;
            };
            match ad3_editor::playback_session::PlaybackSession::start_project_at_speed(
                snapshot.document.clone(),
                snapshot.timeline.clone(),
                snapshot.project.clone(),
                Tools::default(),
                transport.time,
                self.controls_draft
                    .iter()
                    .map(|control| {
                        (
                            control.declaration.file.clone(),
                            control.declaration.name.clone(),
                            control.value.clone(),
                        )
                    })
                    .collect(),
                transport.speed,
            ) {
                Ok(session) => {
                    self.audio_playback = Some((
                        transport.generation,
                        self.generation,
                        transport.time,
                        session,
                    ))
                }
                Err(error) => {
                    self.status = error;
                    transport.playing = false;
                    return;
                }
            }
        }
        let (_, _, start, session) = self.audio_playback.as_mut().unwrap();
        if let Err(error) = session.set_controls(
            self.controls_draft
                .iter()
                .map(|control| {
                    (
                        control.declaration.file.clone(),
                        control.declaration.name.clone(),
                        control.value.clone(),
                    )
                })
                .collect(),
        ) {
            self.status = error;
            transport.playing = false;
            retire_audio(&mut self.audio_playback, &mut self.audio_retired);
            return;
        }
        transport.time = (*start + session.played_frames() as f64 / 48000.0 * session.speed())
            .min(transport.workarea[1]);
        if let Some(result) = session.poll() {
            self.audio_playback.take();
            match result {
                Err(error) => {
                    self.status = error;
                    transport.playing = false;
                }
                Ok(()) if transport.looping => {
                    transport.time = transport.workarea[0];
                    transport.generation += 1;
                }
                Ok(()) => {
                    transport.time = transport.workarea[1];
                    transport.playing = false;
                }
            }
        }
    }
    fn playback_shortcuts(&mut self, context: &egui::Context) {
        if context.egui_wants_keyboard_input() {
            return;
        }
        let restack = context.input_mut(|input| {
            let modifiers = input.modifiers;
            if modifiers.command || modifiers.ctrl {
                return None;
            }
            if input.consume_key(modifiers, egui::Key::CloseBracket) {
                Some(true)
            } else if input.consume_key(modifiers, egui::Key::OpenBracket) {
                Some(false)
            } else {
                None
            }
        });
        if let Some(front) = restack
            && let Some(snapshot) = &self.snapshot
        {
            let next = snapshot.document.restack(&self.selection, front);
            self.commit(next);
        }
        let keys = context.input(|i| {
            let plain =
                !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.shift;
            (
                i.key_pressed(egui::Key::Space),
                plain && i.key_pressed(egui::Key::J),
                plain && i.key_pressed(egui::Key::K),
                plain && i.key_pressed(egui::Key::L),
                plain && i.key_pressed(egui::Key::W),
                plain && i.key_pressed(egui::Key::Q),
                plain && i.key_pressed(egui::Key::E),
            )
        });
        if let Some(transport) = &mut self.transport {
            let generation = transport.generation;
            if keys.0 {
                transport.toggle();
            }
            if keys.1 {
                transport.shuttle(true);
            }
            if keys.2 {
                transport.k();
            }
            if keys.3 {
                transport.shuttle(false);
            }
            if generation != transport.generation {
                self.invalidate();
            }
        }
        if keys.4 {
            self.split();
        }
        if keys.5 {
            self.ripple_trim(true);
        }
        if keys.6 {
            self.ripple_trim(false);
        }
    }

    fn replace_markers(
        &mut self,
        expected: &[ad3_editor::project_config::Marker],
        next: &[ad3_editor::project_config::Marker],
    ) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = (|| {
            let mut config = ad3_editor::project_config::ProjectConfig::load(&snapshot.project)?;
            let scene = &snapshot.timeline.active;
            if config.markers(scene) != expected {
                return Err("Markers changed externally; reload before editing".to_string());
            }
            config.set_markers(scene, next)?;
            config.save()?;
            Ok(config)
        })();
        match result {
            Ok(config) => {
                self.marker_config = Some(config);
                self.status = "Marcadores salvos.".into();
            }
            Err(error) => self.status = error,
        }
    }

    fn edit_markers(&mut self, clear: bool) {
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        let result = (|| {
            let mut config = ad3_editor::project_config::ProjectConfig::load(&snapshot.project)?;
            if clear {
                config.set_markers(&snapshot.timeline.active, &[])?;
            } else {
                config.toggle_marker(
                    &snapshot.timeline.active,
                    transport.time,
                    &self.marker_color,
                )?;
            }
            config.save()?;
            Ok(config)
        })();
        match result {
            Ok(config) => {
                self.marker_config = Some(config);
                self.status = "Marcadores salvos.".into();
            }
            Err(error) => self.status = error,
        }
    }

    fn marker_gesture(
        &mut self,
        now: f64,
        pressed: bool,
        released: bool,
        down: bool,
        allowed: bool,
    ) {
        let scene = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.timeline.active.clone())
            .unwrap_or_default();
        if !allowed
            || scene.is_empty()
            || self
                .marker_hold
                .as_ref()
                .is_some_and(|(held_scene, _)| *held_scene != scene)
        {
            self.marker_hold = None;
            return;
        }
        if pressed {
            self.marker_hold = Some((scene, now));
        }
        if let Some((_, started)) = &self.marker_hold {
            if now - started >= 2.0 && (down || released) {
                self.marker_picker = true;
                self.marker_hold = None;
            } else if released {
                self.marker_hold = None;
                self.edit_markers(false);
            } else if !down {
                self.marker_hold = None;
            }
        }
    }

    fn edit_shortcuts(&mut self, context: &egui::Context) {
        if context.egui_wants_keyboard_input() {
            self.marker_hold = None;
            return;
        }
        let previous_preview_mode = self.preview_actual_size;
        let (copy, cut, paste) = context.input_mut(|input| {
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0) {
                self.preview_actual_size = true;
            }
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1) {
                self.preview_actual_size = false;
            }
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
            {
                self.zoom = (self.zoom * 1.25).clamp(10.0, 400.0);
            }
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) {
                self.zoom = (self.zoom / 1.25).clamp(10.0, 400.0);
            }
            let mut copy = input.consume_key(egui::Modifiers::COMMAND, egui::Key::C);
            let mut cut = input.consume_key(egui::Modifiers::COMMAND, egui::Key::X);
            let mut paste = input.consume_key(egui::Modifiers::COMMAND, egui::Key::V);
            input.events.retain(|event| match event {
                egui::Event::Copy => {
                    copy = true;
                    false
                }
                egui::Event::Cut => {
                    cut = true;
                    false
                }
                egui::Event::Paste(_) => {
                    paste = true;
                    false
                }
                _ => true,
            });
            (copy, cut, paste)
        });
        if self.preview_actual_size != previous_preview_mode {
            self.invalidate();
        }
        if (copy || cut) && self.copy_selection() && cut {
            self.delete_selection();
        }
        if paste {
            self.paste_selection();
        }
        if context.input_mut(|input| {
            let modifiers = input.modifiers;
            input.consume_key(modifiers, egui::Key::Escape)
        }) {
            self.marker_drag = None;
            self.keyframe_drag = None;
            self.selected_keyframes.clear();
            self.marker_hold = None;
            self.marker_picker = false;
            self.marquee = None;
            self.gap_selection = None;
            self.selection.clear();
            self.selected.clear();
        }
        if context.input_mut(|input| {
            let modifiers = input.modifiers;
            (modifiers.command || modifiers.ctrl) && input.consume_key(modifiers, egui::Key::B)
        }) {
            self.split();
        }
        let nudge = context.input_mut(|input| {
            let modifiers = input.modifiers;
            let step = if modifiers.shift { 10.0 } else { 1.0 };
            [
                (egui::Key::ArrowLeft, [-step, 0.0]),
                (egui::Key::ArrowRight, [step, 0.0]),
                (egui::Key::ArrowUp, [0.0, -step]),
                (egui::Key::ArrowDown, [0.0, step]),
            ]
            .into_iter()
            .find_map(|(key, delta)| input.consume_key(modifiers, key).then_some(delta))
        });
        if let Some(delta) = nudge
            && !self.selection.is_empty()
            && let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport)
        {
            self.commit(snapshot.document.nudge(
                &snapshot.timeline,
                &self.selection,
                transport.time,
                delta,
            ));
        }
        if context.input_mut(|input| {
            input.consume_key(
                egui::Modifiers {
                    command: true,
                    alt: true,
                    shift: true,
                    ..Default::default()
                },
                egui::Key::Enter,
            )
        }) {
            self.dissolve_selection(true);
        }
        if context.input_mut(|input| {
            input.consume_key(
                egui::Modifiers {
                    command: true,
                    alt: true,
                    ..Default::default()
                },
                egui::Key::Enter,
            )
        }) && let Some(snapshot) = &self.snapshot
        {
            match snapshot
                .document
                .wrap_sequence(&snapshot.timeline, &self.selection)
            {
                Ok((document, Some(id))) => {
                    if self.commit(Ok(document)) {
                        self.selected = id.clone();
                        self.selection = vec![id];
                    }
                }
                Ok(_) => {}
                Err(error) => self.status = error,
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        {
            self.wrap_scene_selection();
        }
        let toggle_hidden = context.input_mut(|input| {
            input.consume_key(
                egui::Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                },
                egui::Key::H,
            )
        });
        if context.input_mut(|input| {
            input.consume_key(
                egui::Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                },
                egui::Key::G,
            )
        }) {
            self.ungroup_selection();
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::G))
            && let Some(snapshot) = &self.snapshot
        {
            match snapshot.document.group(&self.selection) {
                Ok((next, Some(group))) => {
                    if self.commit(Ok(next)) {
                        self.selected = group.clone();
                        self.selection = vec![group];
                    }
                }
                Ok(_) => {}
                Err(error) => self.status = error,
            }
        }
        if context.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::D))
            && let Some(snapshot) = &self.snapshot
        {
            match snapshot.document.duplicate(&self.selection) {
                Ok((next, copies)) if !copies.is_empty() => {
                    let mut skews = self.runtime_skews.clone();
                    for copy_id in &copies {
                        let Some(copy) = next.root.find(copy_id) else {
                            continue;
                        };
                        // Duplicate IDs append a numeric suffix; find the longest
                        // authored root prefix so IDs containing dashes stay valid.
                        let original = self
                            .selection
                            .iter()
                            .filter(|id| {
                                copy_id.strip_prefix(id.as_str()).is_some_and(|suffix| {
                                    suffix.starts_with('-') && suffix[1..].parse::<u32>().is_ok()
                                })
                            })
                            .max_by_key(|id| id.len())
                            .and_then(|id| snapshot.document.root.find(id));
                        if let Some(original) = original {
                            for (source, target) in original.nodes().iter().zip(copy.nodes()) {
                                if let Some(skew) = self.runtime_skews.get(&source.id) {
                                    skews.insert(target.id.clone(), *skew);
                                }
                            }
                        }
                    }
                    if self.commit_with_skews(Ok(next), skews) {
                        self.selected = copies[0].clone();
                        self.selection = copies;
                    }
                }
                Ok(_) => {}
                Err(error) => self.status = error,
            }
        }
        if toggle_hidden && let Some(snapshot) = &self.snapshot {
            let hide = self.selection.iter().any(|id| {
                snapshot
                    .timeline
                    .spans
                    .get(id)
                    .is_some_and(|span| !span.hidden)
            });
            self.set_selection_flag("hidden", hide);
        }
        let (now, pressed, released, down, allowed) = context.input(|input| {
            (
                input.time,
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::Quote,
                            pressed: true,
                            repeat: false,
                            ..
                        }
                    )
                }),
                input.key_released(egui::Key::Quote),
                input.key_down(egui::Key::Quote),
                !input.modifiers.command && !input.modifiers.ctrl,
            )
        });
        self.marker_gesture(now, pressed, released, down, allowed);
        if self.marker_hold.is_some() {
            context.request_repaint_after(Duration::from_millis(50));
        }
        let marker_action = context.input_mut(|input| {
            let modifiers = input.modifiers;
            (!modifiers.command && !modifiers.ctrl && input.consume_key(modifiers, egui::Key::M))
                .then_some(modifiers.alt)
        });
        if let Some(clear) = marker_action {
            self.edit_markers(clear);
        }
        let (step, back) = context.input_mut(|input| {
            let modifiers = input.modifiers;
            let allowed = !modifiers.command && !modifiers.ctrl;
            (
                allowed && input.consume_key(modifiers, egui::Key::D),
                allowed && input.consume_key(modifiers, egui::Key::S),
            )
        });
        if (step || back)
            && let Some(transport) = &mut self.transport
        {
            let delta = if step { 1.0 / 30.0 } else { -1.0 };
            let _ = transport.seek(transport.time + delta);
            self.invalidate();
        }
        let (redo, undo, all, delete, forward, children, parents) = context.input_mut(|input| {
            let modifiers = input.modifiers;
            let allowed = !modifiers.command && !modifiers.ctrl;
            let redo = input.consume_key(
                egui::Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                },
                egui::Key::Z,
            ) || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
            (
                redo,
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                input.consume_key(modifiers, egui::Key::Delete)
                    || input.consume_key(modifiers, egui::Key::Backspace),
                allowed && !modifiers.alt && input.consume_key(modifiers, egui::Key::A),
                allowed && input.consume_key(modifiers, egui::Key::Enter),
                allowed && input.consume_key(modifiers, egui::Key::Backslash),
            )
        });
        if all || forward || children || parents {
            self.selected_keyframes.clear();
        }
        if redo {
            self.history(true);
        } else if undo {
            self.history(false);
        } else if all {
            if let Some(snapshot) = &self.snapshot {
                let selection: Vec<String> = snapshot
                    .document
                    .root
                    .children
                    .iter()
                    .filter(|node| {
                        snapshot.timeline.spans.get(&node.id).is_some_and(|span| {
                            self.transport
                                .as_ref()
                                .is_some_and(|transport| span.visible(transport.time))
                        })
                    })
                    .map(|node| node.id.clone())
                    .collect();
                if selection.is_empty() {
                    return;
                }
                self.selection = selection;
                self.selected = self.selection.last().cloned().unwrap_or_default();
                self.gap_selection = None;
                self.marquee = None;
            }
        } else if delete {
            self.delete_selection();
        } else if forward {
            self.select_forward();
        } else if children {
            self.select_children();
        } else if parents {
            self.select_parents();
        }
    }

    fn ripple_trim(&mut self, left: bool) {
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        let selected = self.selection.clone();
        let result = snapshot.document.ripple_trim(
            &snapshot.timeline,
            &snapshot.timeline.active,
            &selected,
            transport.time,
            left,
        );
        match result {
            Ok((next, units, time)) if !units.is_empty() => {
                if self.commit(Ok(next)) {
                    self.selected = units[0].clone();
                    self.selection = units;
                    if let Some(transport) = &mut self.transport {
                        let _ = transport.seek(time);
                    }
                    self.invalidate();
                }
            }
            Ok(_) => {}
            Err(error) => self.status = error,
        }
    }

    fn close_gap(&mut self) {
        let Some((sequence, gap)) = self.gap_selection.take() else {
            return;
        };
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if !self.selection.is_empty() {
            return;
        }
        self.commit(snapshot.document.close_gap(
            &snapshot.timeline,
            &snapshot.timeline.active,
            &sequence,
            gap,
        ));
    }

    fn insert_dropped_files(&mut self, files: &[PathBuf]) {
        if files.is_empty() {
            return;
        }
        if self.exporting || self.capturing {
            self.status = "Aguarde o trabalho de mídia atual terminar.".into();
            return;
        }
        let result = (|| {
            let snapshot = self
                .snapshot
                .as_ref()
                .ok_or("Abra um projeto antes de inserir mídia.")?;
            let mut document = snapshot.document.clone();
            let mut imported_sizes = Vec::new();
            let (scene, time) = if let Some(transport) = &self.transport {
                (
                    snapshot.timeline.active.clone(),
                    transport
                        .time
                        .min((transport.workarea[1] - 1.0 / 30.0).max(transport.workarea[0])),
                )
            } else if !document
                .root
                .children
                .iter()
                .any(|node| node.tag == "scene")
            {
                let mut dimensions = (1920, 1080);
                let mut duration: f64 = 1.0 / 30.0;
                for file in files {
                    let probe = match ad3_editor::texture::probe_image(file, None)? {
                        Some(probe) => probe,
                        None => Tools::default().probe(file)?,
                    };
                    if probe.has_video {
                        dimensions = probe.display_dimensions()?;
                        imported_sizes.push(Some(dimensions));
                    } else {
                        imported_sizes.push(None);
                    }
                    duration = duration.max(
                        probe
                            .duration
                            .filter(|duration| duration.is_finite() && *duration > 0.0)
                            .unwrap_or(5.0),
                    );
                }
                let (width, height) = dimensions;
                let (next, id) = document.create_scene(width, height, duration)?;
                document = next;
                (id, 0.0)
            } else {
                return Err("Escolha uma cena antes de inserir mídia.".into());
            };
            let mut ids = Vec::new();
            for (index, file) in files.iter().enumerate() {
                if !file.is_file() {
                    return Err("Arquivo de mídia local não encontrado.".into());
                }
                let extension = file
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let kind = match extension.as_str() {
                    "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" => "image",
                    "wav" | "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" => "audio",
                    "mp4" | "mov" | "webm" | "mkv" | "avi" => "video",
                    _ => return Err(format!("Formato de mídia não reconhecido: {extension}")),
                };
                let dimensions = if kind == "audio" {
                    (500, 150)
                } else if let Some(Some(dimensions)) = imported_sizes.get(index) {
                    *dimensions
                } else {
                    let probe = match ad3_editor::texture::probe_image(file, None)? {
                        Some(probe) => probe,
                        None => Tools::default().probe(file)?,
                    };
                    probe.display_dimensions()?
                };
                let (next, id) = document.insert_media_centered(
                    &scene,
                    kind,
                    file.to_str().ok_or("Invalid media path")?,
                    time,
                    dimensions,
                )?;
                document = next;
                ids.push(id);
            }
            Ok((document, ids))
        })();
        match result {
            Ok((document, ids)) => {
                if self.commit(Ok(document)) {
                    self.selected = ids.last().cloned().unwrap_or_default();
                    self.selection = ids;
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn insert_selected_mask(&mut self) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let mut size = [
            holder.number("width", 0.0).unwrap_or(0.0),
            holder.number("height", 0.0).unwrap_or(0.0),
        ];
        if let Some((at, geometry)) = &self.frame_geometry {
            if self
                .transport
                .as_ref()
                .is_some_and(|transport| transport.time == *at)
            {
                if let Some(frame) = geometry.get(&holder.id) {
                    size = frame.frame.size;
                }
            }
        }
        if size.iter().any(|value| *value <= 0.0) {
            size = [500.0; 2];
        }
        match snapshot.document.insert_mask(&self.selected, size) {
            Ok((document, id)) => {
                if self.commit(Ok(document)) {
                    self.selected = id.clone();
                    self.selection = vec![id];
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn duplicate_selected_mask(&mut self, id: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(_) = holder
            .children
            .iter()
            .find(|child| child.id == id && child.tag == "rect" && child.flag("mask"))
        else {
            return;
        };
        self.commit(
            snapshot
                .document
                .duplicate(&[id.to_string()])
                .map(|(document, _)| document),
        );
    }

    fn remove_selected_mask(&mut self, id: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.id == id && child.tag == "rect" && child.flag("mask"))
        {
            return;
        }
        self.commit(snapshot.document.remove(id));
    }

    fn insert_selection_media_paint(&mut self, video: bool) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let path = Path::new(&self.media_path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            snapshot.project.join(path)
        };
        if !resolved.is_file() {
            self.status = "Arquivo de mídia local não encontrado.".into();
            return;
        }
        if let Err(error) = validate_media_paint_source(&resolved, video) {
            self.status = error;
            return;
        }
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_media_paint(id, video, &self.media_path)
            });
        self.commit(next);
    }

    fn insert_media(&mut self, kind: &str) {
        if self.exporting || self.capturing {
            return;
        }
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        let path = Path::new(&self.media_path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            snapshot.project.join(path)
        };
        if !resolved.is_file() {
            self.status = "Arquivo de mídia local não encontrado.".into();
            return;
        }
        let time = transport
            .time
            .min((transport.workarea[1] - 1.0 / 30.0).max(transport.workarea[0]));
        let result = (|| {
            let dimensions = if kind == "audio" {
                (500, 150)
            } else {
                let probe = match ad3_editor::texture::probe_image(&resolved, None)? {
                    Some(probe) => probe,
                    None => Tools::default().probe(&resolved)?,
                };
                probe.display_dimensions()?
            };
            snapshot.document.insert_media_centered(
                &snapshot.timeline.active,
                kind,
                &self.media_path,
                time,
                dimensions,
            )
        })();
        match result {
            Ok((next, id)) => {
                if self.commit(Ok(next)) {
                    self.selected = id.clone();
                    self.selection = vec![id];
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn insert_visual(&mut self, kind: &str) {
        if self.exporting || self.capturing {
            return;
        }
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        let time = transport
            .time
            .min((transport.workarea[1] - 1.0 / 30.0).max(transport.workarea[0]));
        match snapshot
            .document
            .insert_visual(&snapshot.timeline.active, kind, time)
        {
            Ok((next, id)) => {
                if self.commit(Ok(next)) {
                    self.selected = id.clone();
                    self.selection = vec![id];
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn resize_active_scene(&mut self, width: u32, height: u32) {
        if self.exporting || self.capturing {
            return;
        }
        if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) {
            self.status = "Dimensões da cena devem estar entre 1 e 8192 pixels.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let id = &snapshot.timeline.active;
        if id.is_empty() {
            return;
        }
        let next = snapshot
            .document
            .set(id, "width", Some(serde_json::json!(width)))
            .and_then(|document| document.set(id, "height", Some(serde_json::json!(height))));
        self.commit(next);
    }

    fn remove_selected_intrinsic_media(&mut self) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = (|| {
            let mut document = snapshot.document.clone();
            let node = document
                .root
                .find(&self.selected)
                .ok_or("Selected media not found")?;
            if !matches!(node.tag.as_str(), "image" | "video") {
                return Err("Selecione uma imagem ou vídeo.".into());
            }
            let missing_width = !node.props.contains_key("width");
            let missing_height = !node.props.contains_key("height");
            if missing_width || missing_height {
                let library =
                    MediaLibrary::load(&document.root, &snapshot.project, &Tools::default())?;
                let probe = library
                    .probes
                    .get(&self.selected)
                    .ok_or("Media dimensions unavailable")?;
                let (width, height) = probe.display_dimensions()?;
                if missing_width {
                    document =
                        document.set(&self.selected, "width", Some(serde_json::json!(width)))?;
                }
                if missing_height {
                    document =
                        document.set(&self.selected, "height", Some(serde_json::json!(height)))?;
                }
            }
            document.remove_intrinsic_media(&snapshot.timeline, &self.selected)
        })();
        self.commit(next);
    }

    fn replace_selected_media(&mut self, source: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(node) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !matches!(
            node.tag.as_str(),
            "image" | "video" | "audio" | "imagePaint" | "videoPaint"
        ) {
            self.status = "Selecione um clipe de mídia.".into();
            return;
        }
        let path = Path::new(source);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            snapshot.project.join(path)
        };
        if source.trim().is_empty() || !resolved.is_file() {
            self.status = "Arquivo de mídia local não encontrado.".into();
            return;
        }
        if node.string("src") == Some(source) {
            return;
        }
        let next = snapshot
            .document
            .set(&node.id, "src", Some(serde_json::json!(source)));
        self.commit(next);
    }

    fn set_media_fit(&mut self, fit: &str) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if !matches!(fit, "cover" | "contain" | "fill") {
            self.status = "Modo de encaixe inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                let node = document.root.find(id).ok_or("Selected media not found")?;
                if !matches!(
                    node.tag.as_str(),
                    "image" | "video" | "imagePaint" | "videoPaint"
                ) {
                    return Err(
                        "Selecione somente imagens ou vídeos para alterar o encaixe.".into(),
                    );
                }
                document.set(id, "objectFit", Some(serde_json::json!(fit)))
            });
        self.commit(result);
    }

    fn text_property_at_playhead(
        &self,
        snapshot: &Snapshot,
        id: &str,
        property: &str,
        default: &serde_json::Value,
    ) -> serde_json::Value {
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let sampled = ad3_editor::motion::Motion::default().sample(
            &snapshot.document.root,
            &snapshot.timeline,
            time,
            30.0,
        );
        let root = sampled.as_ref().unwrap_or(&snapshot.document.root);
        inherited_text_property(root, id, property, default)
    }

    #[cfg(test)]
    fn refresh_shadow_draft(
        &mut self,
        snapshot: &Snapshot,
        holder: &ad3_editor::document::Node,
        shadow: &ad3_editor::document::Node,
    ) {
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let sampled = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, time, 30.0)
            .ok();
        self.refresh_shadow_draft_sampled(holder, shadow, sampled.as_ref());
    }

    fn refresh_shadow_draft_sampled(
        &mut self,
        holder: &ad3_editor::document::Node,
        shadow: &ad3_editor::document::Node,
        sampled: Option<&ad3_editor::document::Node>,
    ) {
        let authored = matches!(holder.tag.as_str(), "text" | "textRange");
        let time = if authored {
            0.0
        } else {
            self.transport
                .as_ref()
                .map_or(0.0, |transport| transport.time)
        };
        if self
            .shadow_drafts
            .get(&shadow.id)
            .is_some_and(|(at, _)| *at == time)
        {
            return;
        }
        let values = sampled
            .filter(|_| !authored)
            .and_then(|root| root.find(&shadow.id))
            .unwrap_or(shadow);
        let color = values
            .string("color")
            .and_then(ad3_editor::color::parse)
            .or_else(|| {
                values
                    .props
                    .get("color")
                    .and_then(serde_json::Value::as_f64)
                    .and_then(ad3_editor::color::number)
            })
            .unwrap_or(0);
        let draft = ShadowDraft {
            rgb: [(color >> 16) as u8, (color >> 8) as u8, color as u8],
            opacity: values.number("opacity", 1.0).unwrap_or(1.0),
            blur: values.number("blur", 0.0).unwrap_or(0.0),
            offset: [
                values.number("offsetX", 0.0).unwrap_or(0.0),
                values.number("offsetY", 0.0).unwrap_or(0.0),
            ],
        };
        self.shadow_drafts.insert(shadow.id.clone(), (time, draft));
    }

    fn edit_selected_shadow(&mut self, id: &str, draft: Option<ShadowDraft>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if draft.as_ref().is_some_and(|draft| {
            !draft.opacity.is_finite()
                || !(0.0..=1.0).contains(&draft.opacity)
                || !draft.blur.is_finite()
                || !(0.0..=512.0).contains(&draft.blur)
                || draft
                    .offset
                    .iter()
                    .any(|value| !value.is_finite() || value.abs() > 1e6)
        }) {
            self.status = "Parâmetros da sombra inválidos.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "shadow" && child.id == id)
        {
            return;
        }
        let next = if let Some(draft) = draft {
            let color = format!(
                "#{:02x}{:02x}{:02x}",
                draft.rgb[0], draft.rgb[1], draft.rgb[2]
            );
            let at = self
                .transport
                .as_ref()
                .map_or(0.0, |transport| transport.time);
            [
                ("color", serde_json::json!(color)),
                ("opacity", serde_json::json!(draft.opacity)),
                ("blur", serde_json::json!(draft.blur)),
                ("offsetX", serde_json::json!(draft.offset[0])),
                ("offsetY", serde_json::json!(draft.offset[1])),
            ]
            .into_iter()
            .try_fold(snapshot.document.clone(), |document, (property, value)| {
                document.set_at_playhead(&snapshot.timeline, id, property, value, at)
            })
        } else {
            snapshot.document.remove(id)
        };
        self.commit(next);
    }

    fn insert_selection_gradient(&mut self, radial: bool) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_gradient_paint(id, radial)
            });
        self.commit(next);
    }

    fn insert_selection_paint(&mut self) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.commit(
            self.selection
                .iter()
                .try_fold(snapshot.document.clone(), |document, id| {
                    document.insert_solid_paint(id)
                }),
        );
    }

    fn set_selected_paint_blend(&mut self, id: &str, mode: Option<&str>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if mode.is_some_and(|mode| !BLEND_MODES.iter().any(|(key, _)| *key == mode)) {
            self.status = "Modo de mesclagem inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(paint) = holder.children.iter().find(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "solidPaint"
                        | "linearGradientPaint"
                        | "radialGradientPaint"
                        | "imagePaint"
                        | "videoPaint"
                        | "stroke"
                )
        }) else {
            return;
        };
        if paint.string("blendMode") == mode {
            return;
        }
        self.commit(snapshot.document.set(
            id,
            "blendMode",
            mode.map(|mode| serde_json::json!(mode)),
        ));
    }

    fn set_selected_media_paint_opacity(&mut self, id: &str, opacity: f64) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            self.status = "Opacidade do preenchimento inválida.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id && matches!(child.tag.as_str(), "imagePaint" | "videoPaint")
        }) {
            return;
        }
        self.commit(
            snapshot.document.set_at_playhead(
                &snapshot.timeline,
                id,
                "opacity",
                serde_json::json!(opacity),
                self.transport
                    .as_ref()
                    .map_or(0.0, |transport| transport.time),
            ),
        );
    }

    fn replace_selected_media_paint(&mut self, id: &str, source: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(paint) = holder.children.iter().find(|child| {
            child.id == id && matches!(child.tag.as_str(), "imagePaint" | "videoPaint")
        }) else {
            return;
        };
        if paint.string("src") == Some(source) {
            return;
        }
        let path = Path::new(source);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            snapshot.project.join(path)
        };
        if source.trim().is_empty() || !resolved.is_file() {
            self.status = "Arquivo de mídia local não encontrado.".into();
            return;
        }
        if let Err(error) = validate_media_paint_source(&resolved, paint.tag == "videoPaint") {
            self.status = error;
            return;
        }
        self.commit(
            snapshot
                .document
                .set(id, "src", Some(serde_json::json!(source))),
        );
    }

    fn set_selected_media_paint_fit(&mut self, id: &str, fit: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if !matches!(fit, "cover" | "contain" | "fill") {
            self.status = "Modo de encaixe inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(paint) = holder.children.iter().find(|child| {
            child.id == id && matches!(child.tag.as_str(), "imagePaint" | "videoPaint")
        }) else {
            return;
        };
        if paint.string("objectFit").unwrap_or("cover") == fit {
            return;
        }
        self.commit(
            snapshot
                .document
                .set(id, "objectFit", Some(serde_json::json!(fit))),
        );
    }

    fn remove_selected_media_paint(&mut self, id: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id && matches!(child.tag.as_str(), "imagePaint" | "videoPaint")
        }) {
            return;
        }
        self.commit(snapshot.document.remove(id));
    }

    fn duplicate_selected_style(&mut self, id: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "solidPaint"
                        | "linearGradientPaint"
                        | "radialGradientPaint"
                        | "imagePaint"
                        | "videoPaint"
                        | "stroke"
                        | "shadow"
                        | "effect"
                        | "animation"
                )
        }) {
            return;
        }
        self.commit(
            snapshot
                .document
                .duplicate(&[id.to_string()])
                .map(|(document, _)| document),
        );
    }

    fn edit_selected_stroke_shape(&mut self, id: &str, property: &str, value: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let valid = match property {
            "join" => matches!(value, "miter" | "round" | "bevel"),
            "cap" => matches!(value, "butt" | "round" | "square"),
            _ => false,
        };
        if !valid {
            self.status = "Formato do contorno inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.id == id && child.tag == "stroke")
        {
            return;
        }
        self.commit(
            snapshot
                .document
                .set(id, property, Some(serde_json::json!(value))),
        );
    }

    fn change_selected_outline_stack(&mut self, id: &str, enabled: Option<bool>, later: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(style) = holder
            .children
            .iter()
            .find(|child| child.id == id && matches!(child.tag.as_str(), "stroke" | "shadow"))
        else {
            return;
        };
        let next = if let Some(enabled) = enabled {
            if style.flag("hidden") == !enabled {
                return;
            }
            snapshot
                .document
                .set(id, "hidden", Some(serde_json::json!(!enabled)))
        } else {
            snapshot.document.move_outline_style(id, later)
        };
        if next
            .as_ref()
            .is_ok_and(|document| document.source == snapshot.document.source)
        {
            return;
        }
        self.commit(next);
    }

    fn set_selected_paint_enabled(&mut self, id: &str, enabled: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(paint) = holder.children.iter().find(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "solidPaint"
                        | "linearGradientPaint"
                        | "radialGradientPaint"
                        | "imagePaint"
                        | "videoPaint"
                )
        }) else {
            return;
        };
        if paint.flag("hidden") == !enabled {
            return;
        }
        self.commit(
            snapshot
                .document
                .set(id, "hidden", Some(serde_json::json!(!enabled))),
        );
    }

    fn move_selected_paint(&mut self, id: &str, later: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "solidPaint"
                        | "linearGradientPaint"
                        | "radialGradientPaint"
                        | "imagePaint"
                        | "videoPaint"
                )
        }) {
            return;
        }
        let next = snapshot.document.move_paint(id, later);
        if next
            .as_ref()
            .is_ok_and(|document| document.source == snapshot.document.source)
        {
            return;
        }
        self.commit(next);
    }

    fn edit_selected_gradient_geometry(&mut self, id: &str, values: [f64; 5]) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if values
            .iter()
            .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
        {
            self.status = "Geometria do gradiente inválida.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "linearGradientPaint" | "radialGradientPaint"
                )
        }) {
            return;
        }
        let at = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let next = ["x", "y", "scaleX", "scaleY", "rotation"]
            .into_iter()
            .zip(values)
            .try_fold(snapshot.document.clone(), |document, (property, value)| {
                document.set_at_playhead(
                    &snapshot.timeline,
                    id,
                    property,
                    serde_json::json!(value),
                    at,
                )
            });
        self.commit(next);
    }

    fn change_selected_gradient_stops(&mut self, gradient_id: &str, remove: Option<&str>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(gradient) = holder.children.iter().find(|child| {
            child.id == gradient_id
                && matches!(
                    child.tag.as_str(),
                    "linearGradientPaint" | "radialGradientPaint"
                )
        }) else {
            return;
        };
        let next = if let Some(id) = remove {
            if !gradient
                .children
                .iter()
                .any(|child| child.tag == "colorStop" && child.id == id)
            {
                return;
            }
            snapshot.document.remove(id)
        } else {
            snapshot
                .document
                .insert_gradient_stop(gradient_id, 0.5, "#ffffff")
        };
        self.commit(next);
    }

    fn edit_selected_gradient(&mut self, id: &str, opacity: Option<f64>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if opacity.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            self.status = "Opacidade do gradiente inválida.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|child| {
            child.id == id
                && matches!(
                    child.tag.as_str(),
                    "linearGradientPaint" | "radialGradientPaint"
                )
        }) {
            return;
        }
        let next = if let Some(opacity) = opacity {
            snapshot.document.set_at_playhead(
                &snapshot.timeline,
                id,
                "opacity",
                serde_json::json!(opacity),
                self.transport
                    .as_ref()
                    .map_or(0.0, |transport| transport.time),
            )
        } else {
            snapshot.document.remove(id)
        };
        self.commit(next);
    }

    fn edit_selected_gradient_stop(&mut self, id: &str, draft: PaintDraft, offset: f64) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if !offset.is_finite()
            || !(0.0..=1.0).contains(&offset)
            || !draft.opacity.is_finite()
            || !(0.0..=1.0).contains(&draft.opacity)
        {
            self.status = "Posição ou opacidade da parada de cor inválida.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder.children.iter().any(|gradient| {
            matches!(
                gradient.tag.as_str(),
                "linearGradientPaint" | "radialGradientPaint"
            ) && gradient
                .children
                .iter()
                .any(|stop| stop.tag == "colorStop" && stop.id == id)
        }) {
            return;
        }
        let at = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let color = format!(
            "#{:02x}{:02x}{:02x}",
            draft.rgb[0], draft.rgb[1], draft.rgb[2]
        );
        let next = [
            ("color", serde_json::json!(color)),
            ("opacity", serde_json::json!(draft.opacity)),
            ("offset", serde_json::json!(offset)),
        ]
        .into_iter()
        .try_fold(snapshot.document.clone(), |document, (property, value)| {
            document.set_at_playhead(&snapshot.timeline, id, property, value, at)
        });
        self.commit(next);
    }

    fn edit_selected_paint(&mut self, id: &str, draft: Option<PaintDraft>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if draft.as_ref().is_some_and(|draft| {
            !draft.opacity.is_finite() || !(0.0..=1.0).contains(&draft.opacity)
        }) {
            self.status = "Opacidade do preenchimento inválida.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "solidPaint" && child.id == id)
        {
            return;
        }
        let next = if let Some(draft) = draft {
            let color = format!(
                "#{:02x}{:02x}{:02x}",
                draft.rgb[0], draft.rgb[1], draft.rgb[2]
            );
            let at = self
                .transport
                .as_ref()
                .map_or(0.0, |transport| transport.time);
            snapshot
                .document
                .set_at_playhead(
                    &snapshot.timeline,
                    id,
                    "color",
                    serde_json::json!(color),
                    at,
                )
                .and_then(|document| {
                    document.set_at_playhead(
                        &snapshot.timeline,
                        id,
                        "opacity",
                        serde_json::json!(draft.opacity),
                        at,
                    )
                })
        } else {
            snapshot.document.remove(id)
        };
        self.commit(next);
    }

    fn insert_selection_stroke(&mut self) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_stroke(id)
            });
        self.commit(next);
    }

    #[cfg(test)]
    fn refresh_stroke_draft(
        &mut self,
        snapshot: &Snapshot,
        holder: &ad3_editor::document::Node,
        stroke: &ad3_editor::document::Node,
    ) {
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let sampled = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, time, 30.0)
            .ok();
        self.refresh_stroke_draft_sampled(holder, stroke, sampled.as_ref());
    }

    fn refresh_stroke_draft_sampled(
        &mut self,
        holder: &ad3_editor::document::Node,
        stroke: &ad3_editor::document::Node,
        sampled: Option<&ad3_editor::document::Node>,
    ) {
        let authored = matches!(holder.tag.as_str(), "text" | "textRange");
        let time = if authored {
            0.0
        } else {
            self.transport
                .as_ref()
                .map_or(0.0, |transport| transport.time)
        };
        if self
            .stroke_drafts
            .get(&stroke.id)
            .is_some_and(|(at, _)| *at == time)
        {
            return;
        }
        let values = sampled
            .filter(|_| !authored)
            .and_then(|root| root.find(&stroke.id))
            .unwrap_or(stroke);
        let color = values
            .string("color")
            .and_then(ad3_editor::color::parse)
            .or_else(|| {
                values
                    .props
                    .get("color")
                    .and_then(serde_json::Value::as_f64)
                    .and_then(ad3_editor::color::number)
            })
            .unwrap_or(0);
        self.stroke_drafts.insert(
            stroke.id.clone(),
            (
                time,
                StrokeDraft {
                    rgb: [(color >> 16) as u8, (color >> 8) as u8, color as u8],
                    width: values.number("width", 1.0).unwrap_or(1.0),
                    opacity: values.number("opacity", 1.0).unwrap_or(1.0),
                    miter_limit: values.number("miterLimit", 10.0).unwrap_or(10.0),
                },
            ),
        );
    }

    fn edit_selected_stroke(&mut self, id: &str, draft: Option<StrokeDraft>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if draft.as_ref().is_some_and(|draft| {
            !draft.width.is_finite()
                || !draft.miter_limit.is_finite()
                || !(0.0..=1_000_000.0).contains(&draft.miter_limit)
                || !(0.0..=8192.0).contains(&draft.width)
                || !draft.opacity.is_finite()
                || !(0.0..=1.0).contains(&draft.opacity)
        }) {
            self.status = "Parâmetros do contorno inválidos.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "stroke" && child.id == id)
        {
            return;
        }
        let next = if let Some(draft) = draft {
            let color = format!(
                "#{:02x}{:02x}{:02x}",
                draft.rgb[0], draft.rgb[1], draft.rgb[2]
            );
            let at = self
                .transport
                .as_ref()
                .map_or(0.0, |transport| transport.time);
            [
                ("color", serde_json::json!(color)),
                ("width", serde_json::json!(draft.width)),
                ("opacity", serde_json::json!(draft.opacity)),
                ("miterLimit", serde_json::json!(draft.miter_limit)),
            ]
            .into_iter()
            .try_fold(snapshot.document.clone(), |document, (property, value)| {
                document.set_at_playhead(&snapshot.timeline, id, property, value, at)
            })
        } else {
            snapshot.document.remove(id)
        };
        self.commit(next);
    }

    fn insert_selection_shadow(&mut self) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_shadow(id)
            });
        self.commit(next);
    }

    fn change_effect_stack(&mut self, id: &str, enabled: Option<bool>, later: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "effect" && child.id == id)
        {
            return;
        }
        let next = if let Some(enabled) = enabled {
            snapshot
                .document
                .set(id, "hidden", Some(serde_json::json!(!enabled)))
        } else {
            snapshot.document.move_effect(id, later)
        };
        if next
            .as_ref()
            .is_ok_and(|document| document.source == snapshot.document.source)
        {
            return;
        }
        self.commit(next);
    }

    fn change_selected_effect_type(&mut self, id: &str, kind: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(&(_, unit, default)) = EFFECT_OPTIONS.iter().find(|(name, _, _)| *name == kind)
        else {
            self.status = "Tipo de efeito inválido.".into();
            return;
        };
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        let Some(effect) = holder
            .children
            .iter()
            .find(|child| child.id == id && child.tag == "effect")
        else {
            return;
        };
        let previous = effect.string("type").unwrap_or("blur");
        if previous == kind {
            return;
        }
        let previous_unit = EFFECT_OPTIONS
            .iter()
            .find(|(name, _, _)| *name == previous)
            .map_or("px", |(_, unit, _)| *unit);
        let next = (|| {
            let mut document = snapshot
                .document
                .set(id, "type", Some(serde_json::json!(kind)))?;
            if previous_unit != unit {
                for track in effect.children.iter().filter(|child| {
                    child.tag == "keyframeTrack" && child.string("property") == Some("value")
                }) {
                    document = document.remove(&track.id)?;
                }
                document = document.set(id, "value", Some(serde_json::json!(default)))?;
            }
            Ok(document)
        })();
        self.commit(next);
    }

    fn edit_selected_effect(&mut self, id: &str, value: Option<f64>) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if value.is_some_and(|value| !value.is_finite()) {
            self.status = "Valor do efeito inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "effect" && child.id == id)
        {
            return;
        }
        let next = if let Some(value) = value {
            snapshot.document.set_at_playhead(
                &snapshot.timeline,
                id,
                "value",
                serde_json::json!(value),
                self.transport
                    .as_ref()
                    .map_or(0.0, |transport| transport.time),
            )
        } else {
            snapshot.document.remove(id)
        };
        self.commit(next);
    }

    fn insert_selection_effect(&mut self, kind: &str, value: f64) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_effect(id, kind, value)
            });
        self.commit(next);
    }

    fn set_selection_blend(&mut self, mode: Option<&str>) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if mode.is_some_and(|mode| !BLEND_MODES.iter().any(|(value, _)| *value == mode)) {
            self.status = "Modo de mesclagem inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.set(id, "blendMode", mode.map(|mode| serde_json::json!(mode)))
            });
        self.commit(result);
    }

    fn change_selected_animation_preset(&mut self, id: &str, kind: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.id == id && child.tag == "animation")
        {
            return;
        }
        self.commit(snapshot.document.change_animation_preset(id, kind));
    }

    fn move_selected_animation(&mut self, id: &str, later: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.id == id && child.tag == "animation")
        {
            return;
        }
        let next = snapshot.document.move_animation(id, later);
        if next
            .as_ref()
            .is_ok_and(|document| document.source == snapshot.document.source)
        {
            return;
        }
        self.commit(next);
    }

    fn update_selected_animation(&mut self, id: &str, out: bool, duration: f64, delay: f64) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        if !duration.is_finite()
            || !(1.0 / 30.0..=86400.0).contains(&duration)
            || !delay.is_finite()
            || !(0.0..=86400.0).contains(&delay)
        {
            self.status = "Duração ou atraso da animação inválido.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.tag == "animation" && child.id == id)
        {
            self.status = "Animação não encontrada no clipe selecionado.".into();
            return;
        }
        let next = snapshot
            .document
            .set(
                id,
                "phase",
                Some(serde_json::json!(if out { "out" } else { "in" })),
            )
            .and_then(|document| document.set(id, "duration", Some(serde_json::json!(duration))))
            .and_then(|document| document.set(id, "delay", Some(serde_json::json!(delay))));
        self.commit(next);
    }

    fn remove_selected_animation(&mut self, id: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(holder) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if !holder
            .children
            .iter()
            .any(|child| child.id == id && child.tag == "animation")
        {
            self.status = "Animação não encontrada no clipe selecionado.".into();
            return;
        }
        self.commit(snapshot.document.remove(id));
    }

    fn insert_selection_animation(&mut self, kind: &str) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.insert_animation(
                    id,
                    kind,
                    self.animation_out,
                    self.animation_duration,
                    self.animation_delay,
                )
            });
        self.commit(result);
    }

    fn set_selection_text_leading(&mut self, percent: f64) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if !percent.is_finite() || !(0.0..=1200.0).contains(&percent) {
            self.status = "Entrelinhas deve estar entre 0% e 1200%.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let value = (percent != 100.0).then(|| serde_json::json!(percent / 100.0));
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                if document.root.find(id).is_none_or(|node| node.tag != "text") {
                    return Err("Selecione somente textos para alterar entrelinhas.".into());
                }
                document.set(id, "leading", value.clone())
            });
        self.commit(next);
    }

    fn set_selected_text_box(&mut self, fixed: bool) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(node) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if node.tag != "text" {
            return;
        }
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let next = if fixed {
            if node.props.contains_key("width") || node.props.contains_key("height") {
                return;
            }
            let size = self.frame_geometry.as_ref().and_then(|(at, geometry)| {
                (*at == time)
                    .then(|| geometry.get(&node.id).map(|item| item.frame.size))
                    .flatten()
            });
            let Some(size) = size else {
                self.status = "Aguarde a prévia do texto antes de fixar a caixa.".into();
                return;
            };
            if size.iter().any(|value| !value.is_finite() || *value < 0.0) {
                self.status = "Tamanho calculado do texto inválido.".into();
                return;
            }
            snapshot
                .document
                .set_at_playhead(
                    &snapshot.timeline,
                    &node.id,
                    "width",
                    serde_json::json!(size[0].round()),
                    time,
                )
                .and_then(|document| {
                    document.set_at_playhead(
                        &snapshot.timeline,
                        &node.id,
                        "height",
                        serde_json::json!(size[1].round()),
                        time,
                    )
                })
        } else {
            // Auto-sized glyphs have no authored box or size animation.
            let tracks: Vec<_> = node
                .children
                .iter()
                .filter(|child| {
                    child.tag == "keyframeTrack"
                        && matches!(child.string("property"), Some("width" | "height"))
                })
                .map(|child| child.id.clone())
                .collect();
            tracks
                .iter()
                .try_fold(snapshot.document.clone(), |document, id| {
                    document.remove(id)
                })
                .and_then(|document| document.set(&node.id, "width", None))
                .and_then(|document| document.set(&node.id, "height", None))
        };
        self.commit(next);
    }

    fn restore_text_inheritance(&mut self) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if !matches!(
            self.property.as_str(),
            "fontFamily"
                | "fontSize"
                | "fontWeight"
                | "fontStyle"
                | "letterSpacing"
                | "textCase"
                | "textAlign"
                | "textBaseline"
                | "color"
        ) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                let node = document.root.find(id).ok_or("Selected text not found")?;
                if !matches!(node.tag.as_str(), "text" | "textRange") {
                    return Err("Selecione somente textos ou trechos de texto.".into());
                }
                document.set(id, &self.property, None)
            });
        self.commit(result);
    }

    fn rename_selected_clip(&mut self, name: &str) {
        if self.exporting || self.capturing || self.selection.len() != 1 {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(node) = snapshot.document.root.find(&self.selected) else {
            return;
        };
        if name.trim().is_empty() {
            self.status = "Digite um nome para o clipe.".into();
            return;
        }
        if node.string("name") == Some(name) {
            return;
        }
        let next = snapshot
            .document
            .set(&node.id, "name", Some(serde_json::json!(name)));
        self.commit(next);
    }

    fn rename_active_scene(&mut self) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(scene) = snapshot.document.root.find(&snapshot.timeline.active) else {
            return;
        };
        if self.scene_name_draft.trim().is_empty() {
            self.status = "Digite um nome para a cena.".into();
            return;
        }
        if scene.string("name") == Some(self.scene_name_draft.as_str()) {
            return;
        }
        let next = snapshot.document.set(
            &scene.id,
            "name",
            Some(serde_json::json!(self.scene_name_draft)),
        );
        self.commit(next);
    }

    fn mark_workarea_endpoint(&mut self, start: bool) {
        let Some(transport) = &self.transport else {
            return;
        };
        let mut area = transport.workarea;
        area[if start { 0 } else { 1 }] = transport.time;
        self.set_workarea(area);
    }

    fn set_workarea(&mut self, area: [f64; 2]) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if snapshot.timeline.active.is_empty() {
            return;
        }
        if area.iter().any(|value| !value.is_finite()) || area[0] < 0.0 || area[1] <= area[0] {
            self.status = "Área de trabalho deve ter início ≥ 0 e fim maior que o início.".into();
            return;
        }
        let next = snapshot.document.set(
            &snapshot.timeline.active,
            "workarea",
            Some(serde_json::json!(area)),
        );
        self.commit(next);
    }

    fn create_scene(&mut self, width: u32, height: u32) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        match snapshot.document.create_scene(width, height, 5.0) {
            Ok((next, id)) => {
                if self.commit(Ok(next)) {
                    self.selection = vec![id.clone()];
                    self.selected = id;
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn duplicate_active_scene(&mut self) {
        if self.exporting || self.capturing {
            return;
        }
        let result = (|| {
            let snapshot = self.snapshot.as_ref().ok_or("No project")?;
            let id = &snapshot.timeline.active;
            if id.is_empty() {
                return Err("No active scene".into());
            }
            let original = snapshot.document.root.find(id).ok_or("Scene not found")?;
            let (next, copies) = snapshot.document.duplicate(std::slice::from_ref(id))?;
            let copy_id = copies.first().ok_or("Scene copy was not created")?.clone();
            let copied = next.root.find(&copy_id).ok_or("Scene copy not found")?;
            let mut skews = self.runtime_skews.clone();
            for (source, target) in original.nodes().iter().zip(copied.nodes()) {
                if let Some(skew) = self.runtime_skews.get(&source.id) {
                    skews.insert(target.id.clone(), *skew);
                }
            }
            Ok((next.activate_scene(&copy_id)?, copy_id, skews))
        })();
        match result {
            Ok((next, id, skews)) => {
                if self.commit_with_skews(Ok(next), skews) {
                    self.selected = id.clone();
                    self.selection = vec![id];
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn remove_active_scene(&mut self) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if snapshot.timeline.active.is_empty() {
            return;
        }
        let next = snapshot.document.remove(&snapshot.timeline.active);
        self.commit(next);
    }

    fn activate_scene(&mut self, id: &str) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if snapshot.timeline.active == id {
            return;
        }
        let next = snapshot.document.activate_scene(id);
        self.commit(next);
    }

    fn capture_frame(&mut self, context: egui::Context) {
        if self.capturing || self.exporting {
            return;
        }
        let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport) else {
            return;
        };
        let snapshot = snapshot.clone();
        let time = transport
            .time
            .min((transport.workarea[1] - 1.0 / 30.0).max(transport.workarea[0]));
        let destination = if self.export_path.is_empty() {
            snapshot.project.join("frame.png")
        } else {
            PathBuf::from(&self.export_path).with_extension("png")
        };
        let skews = self.runtime_skews.clone();
        let controls: Vec<_> = self
            .controls_draft
            .iter()
            .map(|control| {
                (
                    control.declaration.file.clone(),
                    control.declaration.name.clone(),
                    control.value.clone(),
                )
            })
            .collect();
        let sender = self.event_sender.clone();
        self.capturing = true;
        self.capture_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.capture_cancel.clone();
        self.status = "Salvando o frame atual…".into();
        self.capture_job = Some(thread::spawn(move || {
            let result = media_job(|| {
                let tools = Tools::default();
                let mut renderer = ProjectRenderer::new_cancellable(
                    &snapshot.document,
                    &snapshot.project,
                    tools.clone(),
                    false,
                    30.0,
                    Some(cancel.clone()),
                )?;
                renderer.set_runtime_skews(skews)?;
                for (file, name, value) in controls {
                    renderer.set_inspect(&file, &name, value)?;
                }
                let plan = renderer.frame_plan(&snapshot.timeline.active, time, 30.0, false)?;
                plan.capture_cancellable(&tools, &snapshot.project, &destination, Some(&cancel))?;
                Ok(destination)
            });
            let _ = sender.send(Event::Captured(result));
            context.request_repaint();
        }));
    }

    fn export_size_estimate(&self) -> Option<(f64, Option<f64>)> {
        let snapshot = self.snapshot.as_ref()?;
        let area = snapshot.timeline.workareas.get(&snapshot.timeline.active)?;
        let duration = area[1] - area[0];
        if !duration.is_finite() || duration <= 0.0 {
            return None;
        }
        if self.export_audio && self.export_audio_codec == "flac" {
            return Some((duration, None));
        }
        let bitrate = if self.export_video {
            self.export_video_bitrate as f64
        } else {
            0.0
        } + if self.export_audio {
            self.export_audio_bitrate as f64
        } else {
            0.0
        };
        Some((duration, Some(duration * bitrate / 8.0)))
    }

    fn apply_export_preset(&mut self, id: &str) {
        if self.exporting {
            return;
        }
        let (format, height, bitrate, video_codec, audio_codec) = match id {
            "h264-mp4-720p" => ("mp4", 720, 7_000_000, "avc", "aac"),
            "h264-mp4-1080p" | "youtube-1080p" | "instagram-1080p" | "tiktok-1080p" => {
                ("mp4", 1080, 12_000_000, "avc", "aac")
            }
            "h264-mp4-1440p" => ("mp4", 1440, 24_000_000, "avc", "aac"),
            "h264-mp4-2160p" | "youtube-4k" => ("mp4", 2160, 64_000_000, "avc", "aac"),
            "web-embedding-720p" => ("webm", 720, 3_000_000, "vp9", "opus"),
            _ => return,
        };
        self.set_export_format(format);
        self.export_resolution = height;
        self.export_video_bitrate = bitrate;
        self.export_video_codec = video_codec.into();
        self.export_audio_codec = audio_codec.into();
        self.export_fps = 30.0;
        self.export_video = true;
        self.export_audio = true;
        self.export_audio_channels = 2;
        self.export_audio_sample_rate = 48_000;
        self.export_audio_bitrate = 128_000;
    }

    fn set_export_format(&mut self, format: &str) {
        if self.exporting || !matches!(format, "mp4" | "mov" | "webm" | "ogg") {
            return;
        }
        let destination = if self.export_path.is_empty() {
            self.snapshot.as_ref().map_or_else(
                || PathBuf::from("export.mp4"),
                |snapshot| snapshot.project.join("export.mp4"),
            )
        } else {
            PathBuf::from(&self.export_path)
        };
        self.export_path = destination
            .with_extension(format)
            .to_string_lossy()
            .into_owned();
        if format == "ogg" {
            self.export_video = false;
            self.export_audio = true;
        }
        if !self.export_video_codec.is_empty()
            && ad3_editor::project_config::ProjectConfig::validate_video_codec(
                &self.export_video_codec,
                format,
            )
            .is_err()
        {
            self.export_video_codec.clear();
        }
        if !self.export_audio_codec.is_empty()
            && ad3_editor::project_config::ProjectConfig::validate_audio_codec(
                &self.export_audio_codec,
                format,
            )
            .is_err()
        {
            self.export_audio_codec.clear();
        }
        self.normalize_export_audio_rate(format);
    }

    fn normalize_export_audio_rate(&mut self, format: &str) {
        let codec = if self.export_audio_codec.is_empty() {
            if matches!(format, "webm" | "ogg") {
                "opus"
            } else {
                "aac"
            }
        } else {
            self.export_audio_codec.as_str()
        };
        if ad3_editor::project_config::ProjectConfig::validate_audio_sample_rate(
            self.export_audio_sample_rate as f64,
            codec,
        )
        .is_err()
        {
            self.export_audio_sample_rate = 48_000;
        }
    }

    fn save_export_preferences(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = (|| {
            let format = Path::new(&self.export_path)
                .extension()
                .and_then(|extension| extension.to_str())
                .ok_or("Escolha uma extensão para a exportação.")?
                .to_ascii_lowercase();
            let mut config = ad3_editor::project_config::ProjectConfig::load(&snapshot.project)?;
            config.set_export_timing(&snapshot.timeline.active, &format, self.export_fps)?;
            config.set_export_audio(&snapshot.timeline.active, self.export_audio)?;
            config
                .set_export_audio_channels(&snapshot.timeline.active, self.export_audio_channels)?;
            config.set_export_video(&snapshot.timeline.active, self.export_video)?;
            config.set_export_resolution(&snapshot.timeline.active, self.export_resolution)?;
            config
                .set_export_video_bitrate(&snapshot.timeline.active, self.export_video_bitrate)?;
            config.set_export_video_codec(
                &snapshot.timeline.active,
                &format,
                (!self.export_video_codec.is_empty()).then_some(self.export_video_codec.as_str()),
            )?;
            config.set_export_audio_codec(
                &snapshot.timeline.active,
                &format,
                (!self.export_audio_codec.is_empty()).then_some(self.export_audio_codec.as_str()),
            )?;
            config.set_export_audio_sample_rate(
                &snapshot.timeline.active,
                &format,
                self.export_audio_sample_rate,
            )?;
            config
                .set_export_audio_bitrate(&snapshot.timeline.active, self.export_audio_bitrate)?;
            config.save()?;
            Ok(config)
        })();
        match result {
            Ok(config) => {
                self.marker_config = Some(config);
                self.status = "Ajustes de exportação salvos.".into();
            }
            Err(error) => self.status = error,
        }
    }

    fn export(&mut self, context: egui::Context) {
        if self.exporting || self.capturing {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        if snapshot.timeline.active.is_empty() {
            self.status = "Escolha ou crie uma cena antes de exportar.".into();
            return;
        }
        let destination = PathBuf::from(&self.export_path);
        if ad3_editor::media::export_extension(&destination).is_none() {
            self.status = "Escolha MP4, MOV, WebM ou Ogg para exportar.".into();
            return;
        }
        let fps = self.export_fps;
        let audio_enabled = self.export_audio;
        let extension = ad3_editor::media::export_extension(&destination).unwrap();
        let audio_codec = if self.export_audio_codec.is_empty() {
            if matches!(extension, "webm" | "ogg") {
                "opus".into()
            } else {
                "aac".into()
            }
        } else {
            self.export_audio_codec.clone()
        };
        if audio_enabled
            && let Err(error) = ad3_editor::project_config::ProjectConfig::validate_audio_codec(
                &audio_codec,
                extension,
            )
        {
            self.status = error;
            return;
        }
        let video_enabled = self.export_video;
        let video_codec = if self.export_video_codec.is_empty() {
            if ad3_editor::media::export_extension(&destination) == Some("webm") {
                "vp9".into()
            } else {
                "avc".into()
            }
        } else {
            self.export_video_codec.clone()
        };
        if ad3_editor::media::export_extension(&destination) == Some("ogg") && video_enabled {
            self.status = "Ogg exporta apenas áudio; desative Exportar vídeo.".into();
            return;
        }
        if video_enabled
            && let Err(error) = ad3_editor::project_config::ProjectConfig::validate_video_codec(
                &video_codec,
                ad3_editor::media::export_extension(&destination).unwrap(),
            )
        {
            self.status = error;
            return;
        }
        if !audio_enabled && !video_enabled {
            self.status = "Ative vídeo ou áudio antes de exportar.".into();
            return;
        }
        let resolution = self.export_resolution;
        let video_bitrate = self.export_video_bitrate;
        if video_enabled && video_bitrate == 0 {
            self.status = "Bitrate de vídeo deve ser maior que zero.".into();
            return;
        }
        let audio_channels = self.export_audio_channels;
        let audio_sample_rate = self.export_audio_sample_rate;
        let audio_bitrate = self.export_audio_bitrate;
        if video_enabled && !(1..=8192).contains(&resolution) {
            self.status = "Altura de exportação deve estar entre 1 e 8192 pixels.".into();
            return;
        }
        let runtime_skews = self.runtime_skews.clone();
        if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
            self.status = "FPS de exportação deve estar entre 0 e 240.".into();
            return;
        }
        let tx = self.event_sender.clone();
        self.exporting = true;
        self.export_started = Some(Instant::now());
        self.export_progress = (0, 0);
        self.export_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.export_cancel.clone();
        self.status = "Exportando…".into();
        self.export_job = Some(thread::spawn(move || {
            let result = media_job(|| {
                ProjectRenderer::export_with_overrides(
                    &snapshot.document,
                    &snapshot.project,
                    Tools::default(),
                    &destination,
                    fps,
                    audio_enabled,
                    runtime_skews,
                    ad3_editor::project::ExportOverrides {
                        resolution: Some(resolution),
                        video_bitrate: Some(video_bitrate),
                        video_enabled: Some(video_enabled),
                        video_codec: Some(video_codec),
                        audio_codec: Some(audio_codec),
                        audio_channels: Some(audio_channels),
                        audio_sample_rate: Some(audio_sample_rate),
                        audio_bitrate: Some(audio_bitrate),
                    },
                    Some(cancel.clone()),
                    |frame, total| {
                        let _ = tx.send(Event::ExportProgress(frame, total));
                        context.request_repaint();
                    },
                )?;
                Ok(destination)
            });
            let _ = tx.send(Event::Exported(result));
            context.request_repaint();
        }));
    }

    fn poll(&mut self, context: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::ExportProgress(frame, total) => {
                    if self.exporting {
                        self.export_progress = (frame.min(total), total);
                        self.status = if total > 0 && frame >= total {
                            "Finalizando o arquivo de exportação…".into()
                        } else {
                            format!("Exportando: {frame}/{total} frames")
                        };
                    }
                }
                Event::Captured(result) => {
                    self.capturing = false;
                    if let Some(job) = self.capture_job.take() {
                        let _ = job.join();
                    }
                    self.status = match result {
                        Ok(path) => format!("Frame salvo: {}", path.display()),
                        Err(error) => error,
                    };
                }
                Event::Opened(result) => {
                    self.loading = false;
                    match *result {
                        Ok(snapshot) => {
                            if self
                                .snapshot
                                .as_ref()
                                .is_some_and(|current| current.project != snapshot.project)
                            {
                                self.clipboard = None;
                            }
                            self.history = Some(History::new(snapshot.document.clone()));
                            self.runtime_skews.clear();
                            self.skew_undo.clear();
                            self.skew_redo.clear();
                            self.export_path = snapshot
                                .project
                                .join("export.mp4")
                                .to_string_lossy()
                                .into_owned();
                            self.replace_snapshot(snapshot);
                            if let Some(snapshot) = self.snapshot.clone() {
                                self.apply_export_preferences(&snapshot);
                            }
                            self.selected.clear();
                            self.selection.clear();
                            self.texture = None;
                            self.status = if !self.controls_draft.is_empty() {
                                "Projeto aberto. Use os controles para ajustar a composição.".into()
                            } else if self
                                .snapshot
                                .as_ref()
                                .is_some_and(|snapshot| snapshot.document.module_sources.is_empty())
                            {
                                "Projeto aberto. Alterações são salvas no JSX.".into()
                            } else {
                                "Projeto aberto para renderização. Edite os módulos TSX e recarregue para alterar a composição.".into()
                            };
                            self.invalidate();
                        }
                        Err(error) => self.status = error,
                    }
                }
                Event::Exported(result) => {
                    self.exporting = false;
                    self.export_started = None;
                    if let Some(job) = self.export_job.take() {
                        let _ = job.join();
                    }
                    self.status = match result {
                        Ok(path) => format!("Exportado: {}", path.display()),
                        Err(error) => error,
                    };
                }
            }
        }
        while let Ok(reply) = self.frames.try_recv() {
            self.busy = false;
            if reply.generation == self.generation {
                match reply.result {
                    Ok((image, geometry)) => {
                        self.frame_geometry = Some((reply.time, geometry));
                        self.texture = Some(context.load_texture(
                            "scene-preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                    Err(error) => {
                        self.status = error;
                        self.pending_ungroup = None;
                        self.pending_wrap_scene = None;
                        if let Some(t) = &mut self.transport {
                            t.playing = false;
                        }
                    }
                }
            }
        }
        if self.frame_geometry.is_some()
            && let Some((selection, sequences_only)) = self.pending_ungroup.take()
        {
            if selection == self.selection {
                self.dissolve_selection(sequences_only);
            }
        }
        if self.frame_geometry.is_some()
            && let Some(selection) = self.pending_wrap_scene.take()
            && selection == self.selection
        {
            self.wrap_scene_selection();
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        #[cfg(windows)]
        self.tick_audio();
        #[cfg(windows)]
        if !self.audio_retired.is_empty() {
            context.request_repaint_after(Duration::from_millis(10));
        }
        if let Some(transport) = &mut self.transport
            && transport.playing
        {
            #[cfg(not(windows))]
            let _ = transport.advance(elapsed);
            #[cfg(windows)]
            if transport.speed <= 0.0 {
                let _ = transport.advance(elapsed);
            }
            self.dirty_preview = true;
            context.request_repaint_after(Duration::from_millis(16));
        }
        if !self.busy
            && self.dirty_preview
            && let (Some(snapshot), Some(transport)) = (&self.snapshot, &self.transport)
        {
            let time = transport
                .time
                .min((transport.workarea[1] - 1.0 / 30.0).max(transport.workarea[0]));
            if self
                .request
                .try_send(FrameRequest {
                    snapshot: snapshot.clone(),
                    generation: self.generation,
                    time,
                    playing: transport.playing,
                    full_resolution: self.preview_actual_size,
                    runtime_skews: self.runtime_skews.clone(),
                    controls: self
                        .controls_draft
                        .iter()
                        .map(|control| {
                            (
                                control.declaration.file.clone(),
                                control.declaration.name.clone(),
                                control.value.clone(),
                            )
                        })
                        .collect(),
                })
                .is_ok()
            {
                self.busy = true;
                self.dirty_preview = false;
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("AD3 Editing");
            ui.add(
                egui::TextEdit::singleline(&mut self.project_path)
                    .hint_text("Pasta do projeto")
                    .desired_width(350.0),
            );
            if ui
                .add_enabled(!self.loading, egui::Button::new("Abrir / recarregar"))
                .clicked()
            {
                self.open(ui.ctx().clone());
            }
            if ui.button("Desfazer").clicked() {
                self.history(false);
            }
            if ui.button("Refazer").clicked() {
                self.history(true);
            }
            if let Some(snapshot) = &self.snapshot {
                let active = snapshot.timeline.active.clone();
                let scenes: Vec<_> = snapshot
                    .document
                    .root
                    .children
                    .iter()
                    .filter(|node| node.tag == "scene")
                    .map(|node| {
                        (
                            node.id.clone(),
                            node.string("name").unwrap_or(&node.id).to_string(),
                        )
                    })
                    .collect();
                let active_name = snapshot
                    .document
                    .root
                    .find(&active)
                    .and_then(|node| node.string("name"))
                    .unwrap_or(&active)
                    .to_string();
                let editable = snapshot.document.module_sources.is_empty();
                ui.add_enabled_ui(editable && !self.exporting && !self.capturing, |ui| {
                    ui.menu_button("Nova cena", |ui| {
                        for (label, width, height) in [
                            ("Full HD · 1920 × 1080", 1920, 1080),
                            ("Vertical · 1080 × 1920", 1080, 1920),
                            ("Quadrada · 1080 × 1080", 1080, 1080),
                            ("4K · 3840 × 2160", 3840, 2160),
                        ] {
                            if ui.button(label).clicked() {
                                self.create_scene(width, height);
                                ui.close();
                            }
                        }
                    });
                    egui::ComboBox::from_id_salt("active-scene")
                        .selected_text(format!("Cena: {active_name}"))
                        .show_ui(ui, |ui| {
                            for (id, name) in scenes {
                                if ui
                                    .selectable_label(id == active, format!("{name} ({id})"))
                                    .clicked()
                                {
                                    self.activate_scene(&id);
                                }
                            }
                        });
                    ui.add_enabled_ui(!active.is_empty(), |ui| {
                        ui.menu_button("Inserir", |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.media_path)
                                    .hint_text("Caminho da mídia local"),
                            );
                            for (kind, label) in [
                                ("image", "Inserir imagem"),
                                ("video", "Inserir vídeo"),
                                ("audio", "Inserir áudio"),
                            ] {
                                if ui.button(label).clicked() {
                                    self.insert_media(kind);
                                    ui.close();
                                }
                            }
                            ui.separator();
                            for (kind, label) in [
                                ("rect", "Retângulo"),
                                ("circle", "Círculo"),
                                ("text", "Texto"),
                            ] {
                                if ui.button(label).clicked() {
                                    self.insert_visual(kind);
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Formato da cena", |ui| {
                            for (label, width, height) in [
                                ("Full HD · 1920 × 1080", 1920, 1080),
                                ("Vertical · 1080 × 1920", 1080, 1920),
                                ("Quadrado · 1080 × 1080", 1080, 1080),
                                ("4K · 3840 × 2160", 3840, 2160),
                            ] {
                                if ui.button(label).clicked() {
                                    self.resize_active_scene(width, height);
                                    ui.close();
                                }
                            }
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut self.scene_name_draft)
                                .hint_text("Nome da cena")
                                .desired_width(150.0),
                        );
                        if ui.button("Renomear cena").clicked() {
                            self.rename_active_scene();
                        }
                    });
                    if ui
                        .add_enabled(!active.is_empty(), egui::Button::new("Duplicar cena"))
                        .clicked()
                    {
                        self.duplicate_active_scene();
                    }
                    if ui
                        .add_enabled(!active.is_empty(), egui::Button::new("Excluir cena"))
                        .on_hover_text(
                            "Remove a cena ativa e seu conteúdo. Desfazer restaura a cena.",
                        )
                        .clicked()
                    {
                        self.remove_active_scene();
                    }
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.transport.is_some() && !self.exporting && !self.capturing, |ui| {
                ui.add(egui::DragValue::new(&mut self.workarea_draft[0]).speed(1.0 / 30.0).prefix("Início: ").suffix(" s"));
                ui.add(egui::DragValue::new(&mut self.workarea_draft[1]).speed(1.0 / 30.0).prefix("Fim: ").suffix(" s"));
                if ui.button("Aplicar trecho").clicked() { self.set_workarea(self.workarea_draft); }
                if ui.button("Início no cursor").clicked() { self.mark_workarea_endpoint(true); }
                if ui.button("Fim no cursor").clicked() { self.mark_workarea_endpoint(false); }
            });
            if let Some(transport) = &mut self.transport {
                let previous = transport.generation;
                if ui.button("J ◀").clicked() {
                    transport.shuttle(true);
                }
                if ui
                    .button(if transport.playing {
                        "Pausar"
                    } else {
                        "Reproduzir"
                    })
                    .clicked()
                {
                    transport.toggle();
                }
                if ui.button("K").clicked() {
                    transport.k();
                }
                if ui.button("L ▶").clicked() {
                    transport.shuttle(false);
                }
                ui.label(format!("{:.3}s   {}×", transport.time, transport.speed));
                ui.checkbox(&mut transport.looping, "Loop");
                ui.label("Prévia sem áudio");
                if previous != transport.generation {
                    self.invalidate();
                }
            }
            if ui.button("Cortar (W)").clicked() {
                self.split();
            }
            if ui.button("Ripple entrada (Q)").clicked() {
                self.ripple_trim(true);
            }
            if ui.button("Ripple saída (E)").clicked() {
                self.ripple_trim(false);
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.export_path)
                    .hint_text("Arquivo de exportação.mp4")
                    .desired_width(230.0),
            );
            ui.add_enabled_ui(!self.exporting, |ui| {
                ui.menu_button("Predefinições", |ui| {
                    for (id, name) in [
                        ("h264-mp4-720p", "HD 720p"),
                        ("h264-mp4-1080p", "Full HD 1080p"),
                        ("h264-mp4-1440p", "2K Quad HD"),
                        ("h264-mp4-2160p", "4K Ultra HD"),
                        ("youtube-1080p", "YouTube Shorts"),
                        ("youtube-4k", "YouTube 4K"),
                        ("instagram-1080p", "Instagram Reels"),
                        ("tiktok-1080p", "TikTok"),
                        ("web-embedding-720p", "Web HD 720p"),
                    ] {
                        if ui.button(name).clicked() {
                            self.apply_export_preset(id);
                            ui.close();
                        }
                    }
                });
            });
            let current_format = Path::new(&self.export_path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            ui.add_enabled_ui(!self.exporting, |ui| {
                egui::ComboBox::from_id_salt("export-format")
                    .selected_text(if current_format.is_empty() {
                        "Formato".into()
                    } else {
                        current_format.to_uppercase()
                    })
                    .show_ui(ui, |ui| {
                        for (format, label) in [
                            ("mp4", "MP4"),
                            ("mov", "MOV"),
                            ("webm", "WebM"),
                            ("ogg", "Ogg"),
                        ] {
                            if ui
                                .selectable_label(current_format == format, label)
                                .clicked()
                            {
                                self.set_export_format(format);
                            }
                        }
                    });
            });
            // The format picker can change the path during this frame.
            // Codec menus must use the newly selected container immediately.
            let current_format = Path::new(&self.export_path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            ui.add(
                egui::DragValue::new(&mut self.export_fps)
                    .speed(0.1)
                    .prefix("FPS: "),
            );
            ui.add_enabled_ui(!self.exporting && self.export_video, |ui| {
                let mut mbps = self.export_video_bitrate as f64 / 1_000_000.0;
                if ui.add(egui::DragValue::new(&mut mbps).speed(0.1).range(0.001..=u32::MAX as f64 / 1_000_000.0).suffix(" Mbps")).changed() {
                    self.export_video_bitrate = (mbps * 1_000_000.0).round().clamp(1.0, u32::MAX as f64) as u32;
                }
                egui::ComboBox::from_id_salt("export-video-codec")
                    .selected_text(if self.export_video_codec.is_empty() {
                        "Codec automático".into()
                    } else {
                        self.export_video_codec.to_uppercase()
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.export_video_codec,
                            String::new(),
                            "Automático",
                        );
                        let codecs: &[&str] = match current_format.as_str() {
                            "mp4" => &["avc", "hevc", "av1"],
                            "mov" => &["avc", "hevc"],
                            "webm" => &["vp8", "vp9", "av1"],
                            _ => &[],
                        };
                        for codec in codecs {
                            ui.selectable_value(
                                &mut self.export_video_codec,
                                (*codec).into(),
                                codec.to_uppercase(),
                            );
                        }
                    });
            });
            ui.add_enabled(
                !self.exporting,
                egui::DragValue::new(&mut self.export_resolution)
                    .range(1..=8192)
                    .speed(1)
                    .prefix("Altura: ")
                    .suffix(" px"),
            );
            if ui.add_enabled(self.transport.is_some(), egui::Button::new("Salvar ajustes")).clicked() {
                self.save_export_preferences();
            }
            ui.checkbox(&mut self.export_audio, "Exportar áudio");
            ui.add_enabled_ui(!self.exporting && self.export_audio, |ui| {
                let previous_codec = self.export_audio_codec.clone();
                egui::ComboBox::from_id_salt("export-audio-codec")
                    .selected_text(if self.export_audio_codec.is_empty() {
                        "Áudio automático".into()
                    } else {
                        self.export_audio_codec.to_uppercase()
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.export_audio_codec,
                            String::new(),
                            "Automático",
                        );
                        let codecs: &[&str] = match current_format.as_str() {
                            "mp4" => &["aac", "opus", "mp3", "flac"],
                            "mov" => &["aac", "mp3"],
                            "webm" => &["opus", "vorbis"],
                            "ogg" => &["opus", "vorbis", "flac"],
                            _ => &[],
                        };
                        for codec in codecs {
                            ui.selectable_value(
                                &mut self.export_audio_codec,
                                (*codec).into(),
                                codec.to_uppercase(),
                            );
                        }
                    });
                if self.export_audio_codec != previous_codec {
                    self.normalize_export_audio_rate(&current_format);
                }
            });
            ui.add_enabled_ui(!self.exporting && self.export_audio, |ui| {
                let codec = if self.export_audio_codec.is_empty() {
                    if matches!(current_format.as_str(), "webm" | "ogg") { "opus" } else { "aac" }
                } else { self.export_audio_codec.as_str() };
                egui::ComboBox::from_id_salt("export-audio-sample-rate")
                    .selected_text(format!("{} Hz", self.export_audio_sample_rate))
                    .show_ui(ui, |ui| {
                        for rate in [7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000] {
                            if ad3_editor::project_config::ProjectConfig::validate_audio_sample_rate(rate as f64, codec).is_ok() {
                                ui.selectable_value(&mut self.export_audio_sample_rate, rate, format!("{rate} Hz"));
                            }
                        }
                    });
                ui.add_enabled_ui(codec != "flac", |ui| {
                    let mut kbps = self.export_audio_bitrate as f64 / 1000.0;
                    if ui.add(egui::DragValue::new(&mut kbps).speed(1.0).range(1.0..=u32::MAX as f64 / 1000.0).suffix(" kbps")).changed() {
                        self.export_audio_bitrate = (kbps * 1000.0).round().clamp(1.0, u32::MAX as f64) as u32;
                    }
                });
                egui::ComboBox::from_id_salt("export-audio-channels")
                    .selected_text(if self.export_audio_channels == 1 {
                        "Mono"
                    } else {
                        "Estéreo"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.export_audio_channels, 1, "Mono");
                        ui.selectable_value(&mut self.export_audio_channels, 2, "Estéreo");
                    });
            });
            ui.checkbox(&mut self.export_video, "Exportar vídeo");
            if ui.add_enabled(!self.capturing && !self.exporting && self.transport.is_some(), egui::Button::new("Salvar frame PNG"))
                .on_hover_text("Salva o instante da timeline na resolução da cena. Usa o caminho de exportação com extensão .png.")
                .clicked() {
                self.capture_frame(ui.ctx().clone());
            }
            if let Some((duration, bytes)) = self.export_size_estimate() {
                let size = bytes.map_or_else(
                    || "tamanho variável (FLAC)".into(),
                    |bytes| format!("≈ {:.1} MB", bytes / 1_000_000.0),
                );
                ui.label(format!("{duration:.2}s · {size}"))
                    .on_hover_text("Estimativa pelos bitrates selecionados e duração da área de trabalho. O conteúdo e o contêiner podem alterar o tamanho final.");
            }
            if ui
                .add_enabled(
                    !self.exporting && !self.capturing && self.transport.is_some(),
                    egui::Button::new("Exportar"),
                )
                .clicked()
            {
                self.export(ui.ctx().clone());
            }
            if self.exporting && ui.button("Cancelar exportação").clicked() {
                self.export_cancel.store(true, Ordering::Relaxed);
            }
            if self.capturing && ui.button("Cancelar captura").clicked() {
                self.capture_cancel.store(true, Ordering::Relaxed);
                self.status = "Cancelando captura…".into();
            }
            if self.exporting {
                let (frame, total) = self.export_progress;
                let fraction = if total > 0 { frame as f64 / total as f64 } else { 0.0 };
                let elapsed = self.export_started.map_or(0.0, |started| started.elapsed().as_secs_f64());
                let text = if self.export_cancel.load(Ordering::Relaxed) {
                    "Cancelando…".into()
                } else if total > 0 && frame >= total {
                    format!("Finalizando · {elapsed:.0}s")
                } else if frame > 0 && elapsed >= 1.0 {
                    let remaining = elapsed * (total - frame) as f64 / frame as f64;
                    format!("{:.0}% · {elapsed:.0}s · ≈ {remaining:.0}s restantes", fraction * 100.0)
                } else {
                    format!("{:.0}% · {elapsed:.0}s", fraction * 100.0)
                };
                ui.add(egui::ProgressBar::new(fraction as f32).desired_width(250.0).text(text));
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        });
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            self.marker_hold = None;
        }
        let cancel_marker = !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|input| input.key_pressed(egui::Key::Escape));
        if cancel_marker {
            self.marker_drag = None;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let Some(transport) = &self.transport else {
            ui.label("Selecione uma cena ativa no projeto.");
            return;
        };
        let playhead = transport.time;
        let end = transport.workarea[1];
        let scene = &snapshot.timeline.active;
        let mut nodes = snapshot
            .document
            .root
            .nodes()
            .into_iter()
            .filter(|node| node.is_timed() && node.tag != "scene")
            .filter_map(|node| {
                snapshot
                    .timeline
                    .spans
                    .get(&node.id)
                    .filter(|span| span.scene_id == *scene)
                    .map(|span| (node, span))
            })
            .collect::<Vec<_>>();
        nodes.reverse();
        ui.horizontal(|ui| {
            ui.label("Timeline");
            ui.add(egui::Slider::new(&mut self.zoom, 10.0..=400.0).text("Zoom"));

            let label = COLORS
                .iter()
                .find(|(_, hex)| *hex == self.marker_color)
                .map_or("Rosa", |(label, _)| *label);
            egui::ComboBox::from_id_salt("marker-color")
                .selected_text(format!("Marcador: {label}"))
                .show_ui(ui, |ui| {
                    for (label, hex) in COLORS {
                        ui.selectable_value(&mut self.marker_color, hex.into(), label);
                    }
                });
            if ui.button("Marcar (M)").clicked() {
                self.edit_markers(false);
            }
            egui::Window::new("Cor do marcador")
                .open(&mut self.marker_picker)
                .show(ui.ctx(), |ui| {
                    for (label, hex) in COLORS {
                        ui.selectable_value(&mut self.marker_color, hex.into(), label);
                    }
                });
            if self.gap_selection.is_some() && ui.button("Fechar intervalo").clicked() {
                self.close_gap();
            }
        });
        egui::ScrollArea::both()
            .id_salt("timeline-scroll")
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: egui::scroll_area::DragScroll::Never,
                ..Default::default()
            })
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ((end as f32 + 2.0) * self.zoom + 160.0).max(ui.available_width());
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click_and_drag());
                let origin = rect.left() + 155.0;
                let content_origin = Pos2::new(origin, rect.bottom());
                let mut regions = Vec::new();
                let mut keyframe_regions = Vec::new();
                #[cfg(test)]
                self.keyframe_hits.clear();
                let mut body_bottom = rect.bottom();
                let painter = ui.painter();
                for second in 0..=(end.ceil() as usize).min(10000) {
                    let x = origin + second as f32 * self.zoom;
                    painter.line_segment(
                        [Pos2::new(x, rect.top() + 20.0), Pos2::new(x, rect.bottom())],
                        Stroke::new(1.0, Color32::GRAY),
                    );
                    if self.zoom > 35.0 || second % 5 == 0 {
                        painter.text(
                            Pos2::new(x + 2.0, rect.top()),
                            egui::Align2::LEFT_TOP,
                            format!("{second}s"),
                            FontId::monospace(11.0),
                            Color32::LIGHT_GRAY,
                        );
                    }
                }
                let markers = self
                    .marker_config
                    .as_ref()
                    .map_or_else(Vec::new, |config| config.markers(scene));
                #[cfg(test)]
                self.marker_hits.clear();
                for (index, marker) in markers.iter().enumerate() {
                    let time = self
                        .marker_drag
                        .filter(|(drag_index, _)| *drag_index == index)
                        .map_or(marker.time, |(_, time)| time);
                    let x = origin + time as f32 * self.zoom;
                    let hit =
                        Rect::from_min_size(Pos2::new(x - 7.0, rect.top()), Vec2::new(14.0, 18.0));
                    let pin = ui
                        .interact(
                            hit,
                            ui.id().with(("marker", index)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    #[cfg(test)]
                    self.marker_hits.push(hit);
                    if pin.clicked() {
                        if let Some(transport) = &mut self.transport {
                            let _ = transport.seek(marker.time);
                        }
                        self.invalidate();
                    }
                    pin.context_menu(|ui| {
                        if ui.button("Remover marcador").clicked() {
                            let mut remaining = markers.clone();
                            remaining.remove(index);
                            self.marker_drag = None;
                            self.replace_markers(&markers, &remaining);
                            ui.close();
                        }
                    });
                    if pin.drag_started() && !cancel_marker {
                        self.marker_drag = Some((index, marker.time));
                    }
                    if pin.dragged()
                        && self
                            .marker_drag
                            .is_some_and(|(drag_index, _)| drag_index == index)
                    {
                        self.marker_drag = Some((
                            index,
                            ad3_editor::timing::snap(
                                (marker.time + f64::from(pin.drag_delta().x / self.zoom)).max(0.0),
                            ),
                        ));
                    }
                    if pin.drag_stopped()
                        && let Some((drag_index, time)) = self.marker_drag.take()
                    {
                        if drag_index == index {
                            let result = (|| {
                                let mut config = ad3_editor::project_config::ProjectConfig::load(
                                    &snapshot.project,
                                )?;
                                if config.markers(scene) != markers {
                                    return Err("Markers changed externally; reload before moving"
                                        .to_string());
                                }
                                let mut moved = markers.clone();
                                moved[index].time = time;
                                config.set_markers(scene, &moved)?;
                                config.save()?;
                                Ok(config)
                            })();
                            match result {
                                Ok(config) => {
                                    self.marker_config = Some(config);
                                    self.status = "Marcador movido.".into();
                                }
                                Err(error) => self.status = error,
                            }
                        }
                    }
                    let color = Color32::from_rgb(
                        u8::from_str_radix(&marker.color[1..3], 16).unwrap(),
                        u8::from_str_radix(&marker.color[3..5], 16).unwrap(),
                        u8::from_str_radix(&marker.color[5..7], 16).unwrap(),
                    );
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(x - 5.0, rect.top()),
                            Pos2::new(x + 5.0, rect.top()),
                            Pos2::new(x + 5.0, rect.top() + 10.0),
                            Pos2::new(x, rect.top() + 16.0),
                            Pos2::new(x - 5.0, rect.top() + 10.0),
                        ],
                        color,
                        Stroke::NONE,
                    ));
                }
                if (response.clicked() || response.dragged())
                    && self.marker_drag.is_none()
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                    if let Some(transport) = &mut self.transport {
                        let _ =
                            transport.seek(f64::from((pointer.x - origin).max(0.0) / self.zoom));
                    }
                    self.invalidate();
                }
                for (node, span) in nodes {
                    let (row, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
                    body_bottom = row.bottom();
                    let clip = Rect::from_min_max(
                        Pos2::new(origin + span.start as f32 * self.zoom, row.top() + 3.0),
                        Pos2::new(origin + span.end as f32 * self.zoom, row.bottom() - 3.0),
                    );
                    let response =
                        ui.interact(clip, ui.id().with(&node.id), Sense::click_and_drag());
                    let selected = self.selection.contains(&node.id);
                    let custom = snapshot.document.source_key(&node.id).and_then(|key| {
                        self.marker_config
                            .as_ref()?
                            .clip_color(&key)
                            .and_then(ad3_editor::color::parse)
                    });
                    let color = if selected {
                        Color32::from_rgb(30, 142, 128)
                    } else if let Some(rgb) = custom {
                        Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
                    } else if node.tag == "audio" {
                        Color32::from_rgb(91, 79, 143)
                    } else {
                        Color32::from_rgb(47, 79, 112)
                    };
                    ui.painter().text(
                        Pos2::new(row.left() + 6.0, row.center().y),
                        egui::Align2::LEFT_CENTER,
                        node.string("name").unwrap_or(&node.id),
                        FontId::proportional(12.0),
                        Color32::WHITE,
                    );
                    ui.painter().rect_filled(clip, 3.0, color);
                    if span.hidden || span.muted {
                        ui.painter().text(
                            Pos2::new(clip.left() + 4.0, clip.center().y),
                            egui::Align2::LEFT_CENTER,
                            match (span.hidden, span.muted) {
                                (true, true) => "Oculto · sem áudio",
                                (true, false) => "Oculto",
                                _ => "Sem áudio",
                            },
                            FontId::proportional(10.0),
                            Color32::WHITE,
                        );
                    }
                    let gaps = if node.tag == "sequence" {
                        snapshot
                            .document
                            .sequence_gaps(&snapshot.timeline, &node.id)
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let mut start = span.start;
                    for gap in gaps
                        .iter()
                        .copied()
                        .chain(std::iter::once([span.end, span.end]))
                    {
                        if start < gap[0] {
                            regions.push((
                                node.id.clone(),
                                ad3_editor::geometry::Aabb {
                                    min_x: start * f64::from(self.zoom),
                                    max_x: gap[0] * f64::from(self.zoom),
                                    min_y: f64::from(clip.top() - content_origin.y),
                                    max_y: f64::from(clip.bottom() - content_origin.y),
                                },
                            ));
                        }
                        start = gap[1];
                    }
                    for gap in &gaps {
                        let empty = Rect::from_min_max(
                            Pos2::new(origin + gap[0] as f32 * self.zoom, clip.top()),
                            Pos2::new(origin + gap[1] as f32 * self.zoom, clip.bottom()),
                        );
                        let selected = self
                            .gap_selection
                            .as_ref()
                            .is_some_and(|(id, selected)| id == &node.id && selected == gap);
                        ui.painter().rect_filled(
                            empty,
                            0.0,
                            if selected {
                                Color32::from_rgb(85, 125, 140)
                            } else {
                                Color32::from_rgb(25, 28, 33)
                            },
                        );
                    }
                    ui.painter().text(
                        clip.left_center() + Vec2::new(5.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &node.tag,
                        FontId::proportional(12.0),
                        Color32::WHITE,
                    );
                    let x = origin + playhead as f32 * self.zoom;
                    ui.painter().line_segment(
                        [Pos2::new(x, row.top()), Pos2::new(x, row.bottom())],
                        Stroke::new(1.5, Color32::from_rgb(240, 174, 54)),
                    );
                    if response.clicked() {
                        self.selected_keyframes.clear();
                        self.gap_selection = None;
                        let gap = response.interact_pointer_pos().and_then(|pointer| {
                            let time = f64::from((pointer.x - origin) / self.zoom);
                            gaps.iter()
                                .find(|gap| time >= gap[0] && time < gap[1])
                                .copied()
                        });
                        if let Some(gap) = gap {
                            self.gap_selection = Some((node.id.clone(), gap));
                            self.selected.clear();
                            self.selection.clear();
                            continue;
                        }
                        let extend =
                            ui.input(|input| input.modifiers.shift || input.modifiers.command);
                        if extend {
                            if self.selection.contains(&node.id) {
                                self.selection.retain(|id| id != &node.id);
                            } else {
                                self.selection.push(node.id.clone());
                            }
                        } else {
                            self.selection = vec![node.id.clone()];
                        }
                        self.selected = self.selection.last().cloned().unwrap_or_default();
                    }
                    if response.drag_started() && self.marquee.is_none() {
                        self.selected_keyframes.clear();
                        self.gap_selection = None;
                        if !self.selection.contains(&node.id) {
                            self.selection = vec![node.id.clone()];
                        }
                        self.selected = node.id.clone();
                        self.drag_start = Some(span.start);
                    }
                    if response.drag_stopped()
                        && self.marquee.is_none()
                        && self.drag_start.take().is_some()
                    {
                        self.commit(snapshot.document.move_clips(
                            &snapshot.timeline,
                            &self.selection,
                            f64::from(response.drag_delta().x / self.zoom),
                        ));
                    }
                    for (target_id, property) in snapshot
                        .document
                        .keyframe_targets(&node.id)
                        .unwrap_or_default()
                    {
                        let Ok(points) = snapshot.document.keyframe_points(
                            &snapshot.timeline,
                            &target_id,
                            &property,
                        ) else {
                            continue;
                        };
                        let (track_row, _) =
                            ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
                        body_bottom = track_row.bottom();
                        ui.painter().text(
                            Pos2::new(track_row.left() + 12.0, track_row.center().y),
                            egui::Align2::LEFT_CENTER,
                            if target_id == node.id {
                                property.clone()
                            } else {
                                format!("{target_id}: {property}")
                            },
                            FontId::proportional(11.0),
                            Color32::LIGHT_GRAY,
                        );
                        for (id, time, value) in points {
                            let center =
                                Pos2::new(origin + time as f32 * self.zoom, track_row.center().y);
                            let hit = Rect::from_center_size(center, Vec2::splat(14.0));
                            keyframe_regions.push(hit);
                            #[cfg(test)]
                            self.keyframe_hits.push((id.clone(), hit));
                            let point = ui.interact(
                                hit,
                                ui.id().with(("keyframe", &id)),
                                Sense::click_and_drag(),
                            );
                            let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
                            if cancel {
                                self.keyframe_drag = None;
                            }
                            if point.drag_started() && !cancel {
                                if !self.selected_keyframes.contains(&id) {
                                    self.selected_keyframes = vec![id.clone()];
                                }
                                self.keyframe_drag_delta = 0.0;
                                self.keyframe_drag = Some((id.clone(), time));
                                self.marquee = None;
                                self.gap_selection = None;
                            }
                            if point.dragged()
                                && self
                                    .keyframe_drag
                                    .as_ref()
                                    .is_some_and(|(drag_id, _)| drag_id == &id)
                            {
                                let at = time + f64::from(point.drag_delta().x / self.zoom);
                                self.keyframe_drag_delta =
                                    f64::from(point.drag_delta().x / self.zoom);
                                let source_time =
                                    ad3_editor::timing::snap(span.source_at(at).max(0.0));
                                self.keyframe_drag = Some((
                                    id.clone(),
                                    span.origin + source_time / span.playback_rate,
                                ));
                            }
                            if point.drag_stopped()
                                && let Some((drag_id, _)) = self.keyframe_drag.take()
                            {
                                if drag_id == id {
                                    let selected = self.selected_keyframes.clone();
                                    if self.commit(snapshot.document.move_keyframes(
                                        &snapshot.timeline,
                                        &self.selected_keyframes,
                                        self.keyframe_drag_delta,
                                    )) {
                                        self.selected_keyframes = selected;
                                    }
                                }
                            }
                            let center = self
                                .keyframe_drag
                                .as_ref()
                                .filter(|_| self.selected_keyframes.contains(&id))
                                .map_or(center, |_| {
                                    let source_time = ad3_editor::timing::snap(
                                        span.source_at(time + self.keyframe_drag_delta).max(0.0),
                                    );
                                    let at = span.origin + source_time / span.playback_rate;
                                    Pos2::new(origin + at as f32 * self.zoom, track_row.center().y)
                                });
                            ui.painter().add(egui::Shape::convex_polygon(
                                vec![
                                    center + Vec2::new(0.0, -5.0),
                                    center + Vec2::new(5.0, 0.0),
                                    center + Vec2::new(0.0, 5.0),
                                    center + Vec2::new(-5.0, 0.0),
                                ],
                                if self.selected_keyframes.contains(&id) {
                                    Color32::WHITE
                                } else {
                                    Color32::from_rgb(240, 174, 54)
                                },
                                Stroke::NONE,
                            ));
                            if point.clicked() {
                                let extend = ui.input(|input| {
                                    input.modifiers.shift
                                        || input.modifiers.command
                                        || input.modifiers.ctrl
                                });
                                if extend {
                                    if self.selected_keyframes.contains(&id) {
                                        self.selected_keyframes.retain(|selected| selected != &id);
                                    } else {
                                        self.selected_keyframes.push(id.clone());
                                    }
                                } else {
                                    self.selected_keyframes = vec![id.clone()];
                                }
                                self.selection = if self.selected_keyframes.is_empty() {
                                    Vec::new()
                                } else {
                                    vec![target_id.clone()]
                                };
                                self.selected = target_id.clone();
                                self.property = property.clone();
                                self.value = value.to_string();
                                self.gap_selection = None;
                                self.marquee = None;
                                if let Some(transport) = &mut self.transport {
                                    let _ = transport.seek(time);
                                }
                                self.invalidate();
                            }
                        }
                    }
                }
                let (pressed, down, pointer, extend) = ui.input(|input| {
                    (
                        input.pointer.primary_pressed(),
                        input.pointer.primary_down(),
                        input.pointer.interact_pos(),
                        input.modifiers.shift || input.modifiers.command,
                    )
                });
                #[cfg(test)]
                {
                    self.hit_regions = regions
                        .iter()
                        .map(|(id, bounds)| {
                            (
                                id.clone(),
                                Rect::from_min_max(
                                    content_origin
                                        + Vec2::new(bounds.min_x as f32, bounds.min_y as f32),
                                    content_origin
                                        + Vec2::new(bounds.max_x as f32, bounds.max_y as f32),
                                ),
                            )
                        })
                        .collect();
                }
                if let Some(pointer) = pointer {
                    let local =
                        Pos2::new(pointer.x - content_origin.x, pointer.y - content_origin.y);
                    let point = ad3_editor::geometry::Point {
                        x: f64::from(local.x),
                        y: f64::from(local.y),
                    };
                    let body =
                        Rect::from_min_max(content_origin, Pos2::new(rect.right(), body_bottom));
                    if pressed
                        && !keyframe_regions.iter().any(|hit| hit.contains(pointer))
                        && ui.clip_rect().contains(pointer)
                        && body.contains(pointer)
                        && !regions.iter().any(|(_, bounds)| {
                            bounds.intersects(ad3_editor::geometry::Aabb {
                                min_x: point.x,
                                max_x: point.x,
                                min_y: point.y,
                                max_y: point.y,
                            })
                        })
                    {
                        self.selected_keyframes.clear();
                        self.marquee = Some((
                            local,
                            if extend {
                                self.selection.clone()
                            } else {
                                Vec::new()
                            },
                        ));
                        self.gap_selection = None;
                    }
                    if let Some((start, base)) = &self.marquee {
                        ui.ctx().request_repaint();
                        if start.distance(local) > 3.0 {
                            self.selection = ad3_editor::geometry::marquee_selection(
                                ad3_editor::geometry::Point {
                                    x: f64::from(start.x),
                                    y: f64::from(start.y),
                                },
                                point,
                                &regions,
                                base,
                            );
                            self.selected = self.selection.last().cloned().unwrap_or_default();
                            let area =
                                Rect::from_two_pos(content_origin + start.to_vec2(), pointer);
                            ui.painter().rect_filled(
                                area,
                                0.0,
                                Color32::from_rgba_unmultiplied(70, 150, 180, 45),
                            );
                            ui.painter().rect_stroke(
                                area,
                                0.0,
                                Stroke::new(1.0, Color32::LIGHT_BLUE),
                                egui::StrokeKind::Inside,
                            );
                        } else if !down && self.gap_selection.is_none() {
                            self.selection = base.clone();
                            self.selected = self.selection.last().cloned().unwrap_or_default();
                        }
                    }
                }
                if !down {
                    self.marquee = None;
                }
            });
    }

    fn project_controls(&mut self, ui: &mut egui::Ui) {
        if self.controls_draft.is_empty() {
            return;
        }
        ui.heading("Controles do projeto");
        let mut changed = false;
        let mut commit = None;
        egui::ScrollArea::vertical()
            .id_salt("project-controls")
            .max_height(400.0)
            .show(ui, |ui| {
                for control in &mut self.controls_draft {
                    let declaration = &control.declaration;
                    ui.push_id((&declaration.file, &declaration.name), |ui| {
                        if !control.group.is_empty() {
                            ui.small(control.group.join(" / "));
                        }
                        ui.label(&control.label);
                        match declaration.kind.as_str() {
                            "number" => {
                                let mut number = control.value.as_f64().unwrap_or_default();
                                let mut input = egui::DragValue::new(&mut number)
                                    .speed(declaration.step.unwrap_or(1.0).abs().max(0.001));
                                let min = declaration.min.unwrap_or(f64::NEG_INFINITY);
                                let max = declaration.max.unwrap_or(f64::INFINITY);
                                if min <= max {
                                    input = input.range(min..=max);
                                }
                                if ui.add(input).changed() && number.is_finite() {
                                    control.value = serde_json::json!(number);
                                    changed = true;
                                }
                            }
                            "boolean" => {
                                let mut value = control.value.as_bool().unwrap_or(false);
                                if ui.checkbox(&mut value, "Ativo").changed() {
                                    control.value = value.into();
                                    changed = true;
                                }
                            }
                            "select" => {
                                let mut value =
                                    control.value.as_str().unwrap_or_default().to_owned();
                                egui::ComboBox::from_id_salt("value")
                                    .selected_text(&value)
                                    .show_ui(ui, |ui| {
                                        for option in &declaration.options {
                                            if ui
                                                .selectable_value(
                                                    &mut value,
                                                    option.clone(),
                                                    option,
                                                )
                                                .changed()
                                            {
                                                changed = true;
                                            }
                                        }
                                    });
                                control.value = value.into();
                            }
                            _ => {
                                let mut value =
                                    control.value.as_str().unwrap_or_default().to_owned();
                                if ui.text_edit_singleline(&mut value).changed() {
                                    control.value = value.into();
                                    changed = true;
                                }
                            }
                        }
                        if control.value != control.committed {
                            ui.horizontal(|ui| {
                                if ui.button("Salvar").clicked() {
                                    commit = Some((
                                        declaration.file.clone(),
                                        declaration.name.clone(),
                                        control.value.clone(),
                                    ));
                                }
                                if ui.button("Restaurar").clicked() {
                                    control.value = control.committed.clone();
                                    changed = true;
                                }
                            });
                        }
                        ui.separator();
                    });
                }
            });
        if changed {
            self.invalidate();
        }
        if let Some((file, name, value)) = commit
            && let Some(snapshot) = &self.snapshot
        {
            let drafts = self.controls_draft.clone();
            let next = snapshot.document.set_control(&file, &name, value);
            self.commit(next);
            for control in &mut self.controls_draft {
                if (control.declaration.file != file || control.declaration.name != name)
                    && let Some(previous) = drafts.iter().find(|old| {
                        old.declaration.file == control.declaration.file
                            && old.declaration.name == control.declaration.name
                            && old.value != old.committed
                    })
                {
                    control.value = previous.value.clone();
                }
            }
        }
    }

    fn set_selection_color(&mut self, color: Option<&str>) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        if self.selection.is_empty() {
            return;
        }
        if self
            .selection
            .iter()
            .any(|id| snapshot.document.source_key(id).is_none())
            && !self.commit(snapshot.document.stamp_ids())
        {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let result = (|| {
            let keys = self
                .selection
                .iter()
                .map(|id| {
                    snapshot.document.source_key(id).ok_or_else(|| {
                        "Este clipe precisa de um ID salvo para receber uma cor.".to_string()
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mut config = ad3_editor::project_config::ProjectConfig::load(&snapshot.project)?;
            config.set_clip_colors(&keys, color)?;
            config.save()?;
            Ok(config)
        })();
        match result {
            Ok(config) => {
                self.marker_config = Some(config);
                self.status = "Cores dos clipes salvas.".into();
            }
            Err(error) => self.status = error,
        }
    }

    fn refresh_opacity_draft(&mut self, snapshot: &Snapshot, id: &str) {
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        if self
            .opacity_draft
            .as_ref()
            .is_some_and(|(current, at, _)| current == id && *at == time)
        {
            return;
        }
        let opacity = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, time, 30.0)
            .ok()
            .and_then(|root| {
                root.find(id)
                    .and_then(|node| node.number("opacity", 1.0).ok())
            })
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        self.opacity_draft = Some((id.into(), time, opacity));
    }

    fn set_selection_opacity(&mut self, opacity: f64) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            self.status = "A opacidade deve estar entre 0 e 100%.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let at = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.set_at_playhead(
                    &snapshot.timeline,
                    id,
                    "opacity",
                    serde_json::json!(opacity),
                    at,
                )
            });
        self.commit(result);
    }

    fn refresh_volume_draft(&mut self, snapshot: &Snapshot, id: &str) {
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        if self
            .volume_draft
            .as_ref()
            .is_some_and(|(current, at, _)| current == id && *at == time)
        {
            return;
        }
        let volume = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, time, 30.0)
            .ok()
            .and_then(|root| {
                root.find(id)
                    .and_then(|node| node.number("volume", 0.0).ok())
            })
            .unwrap_or(0.0)
            .clamp(-60.0, 12.0);
        self.volume_draft = Some((id.into(), time, volume));
    }

    fn set_selection_volume(&mut self, db: f64) {
        if self.exporting || self.capturing || self.selection.is_empty() {
            return;
        }
        if !db.is_finite() || !(-60.0..=12.0).contains(&db) {
            self.status = "O volume deve estar entre -60 e +12 dB.".into();
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let at = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let result = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.set_at_playhead(
                    &snapshot.timeline,
                    id,
                    "volume",
                    serde_json::json!(db),
                    at,
                )
            });
        self.commit(result);
    }

    fn set_selection_flag(&mut self, name: &str, value: bool) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if self.selection.is_empty() || !matches!(name, "hidden" | "muted") {
            return;
        }
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.set(id, name, Some(serde_json::Value::Bool(value)))
            });
        self.commit(next);
    }

    fn remove_selection_keyframe_tracks(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let next = self
            .selection
            .iter()
            .try_fold(snapshot.document.clone(), |document, id| {
                document.remove_keyframe_track(id, &self.property)
            });
        self.commit(next);
    }

    fn toggle_selection_keyframe(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let at = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let next = serde_json::from_str::<serde_json::Value>(&self.value)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                self.selection
                    .iter()
                    .try_fold(snapshot.document.clone(), |document, id| {
                        document.toggle_keyframe_at_playhead(
                            &snapshot.timeline,
                            id,
                            &self.property,
                            value.clone(),
                            at,
                        )
                    })
            });
        self.commit(next);
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        self.project_controls(ui);
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        ui.heading("Cenas");
        for scene in snapshot
            .document
            .root
            .children
            .iter()
            .filter(|node| node.tag == "scene")
        {
            if ui
                .selectable_label(
                    snapshot.timeline.active == scene.id,
                    scene.string("name").unwrap_or(&scene.id),
                )
                .clicked()
            {
                self.commit(snapshot.document.activate_scene(&scene.id));
                if let Some(snapshot) = &self.snapshot {
                    self.transport = snapshot
                        .timeline
                        .workareas
                        .get(&snapshot.timeline.active)
                        .and_then(|area| Transport::new(*area).ok());
                }
            }
        }
        ui.separator();
        ui.heading("Inspector");
        if self.selection.len() > 1 {
            ui.label(format!("{} clipes selecionados", self.selection.len()));
        }
        let Some(node) = snapshot.document.root.find(&self.selected) else {
            ui.label("Selecione um clipe.");
            return;
        };
        ui.label(format!("{} · {}", node.id, node.tag));
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) {
            let mut gradient = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    ui.menu_button("Adicionar gradiente", |ui| {
                        if ui.button("Linear").clicked() {
                            gradient = Some(false);
                            ui.close();
                        }
                        if ui.button("Radial").clicked() {
                            gradient = Some(true);
                            ui.close();
                        }
                    });
                },
            );
            if let Some(radial) = gradient {
                self.insert_selection_gradient(radial);
            }
        }
        let paint_time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let mut edit_paint = None;
        let mut move_paint = None;
        let mut enable_paint = None;
        let mut remove_media_paint = None;
        let mut fit_media_paint = None;
        let mut replace_media_paint = None;
        let mut blend_paint = None;
        let mut duplicate_style = None;
        ui.collapsing("Ordem dos preenchimentos", |ui| {
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty()
                    && self.selection.len() == 1
                    && !self.exporting
                    && !self.capturing,
                |ui| {
                    let paints = node
                        .children
                        .iter()
                        .filter(|child| {
                            matches!(
                                child.tag.as_str(),
                                "solidPaint"
                                    | "linearGradientPaint"
                                    | "radialGradientPaint"
                                    | "imagePaint"
                                    | "videoPaint"
                            )
                        })
                        .collect::<Vec<_>>();
                    for (index, paint) in paints.iter().enumerate() {
                        ui.push_id(("paint-order", &paint.id), |ui| {
                            ui.horizontal(|ui| {
                                let mut enabled = !paint.flag("hidden");
                                if ui.checkbox(&mut enabled, "Ativo").changed() {
                                    enable_paint = Some((paint.id.clone(), enabled));
                                }
                                ui.label(match paint.tag.as_str() {
                                    "solidPaint" => "Cor sólida",
                                    "linearGradientPaint" => "Gradiente linear",
                                    "radialGradientPaint" => "Gradiente radial",
                                    "imagePaint" => "Imagem",
                                    _ => "Vídeo",
                                });
                                if ui.button("Duplicar").clicked() {
                                    duplicate_style = Some(paint.id.clone());
                                }
                                if matches!(paint.tag.as_str(), "imagePaint" | "videoPaint")
                                    && ui.button("Remover").clicked()
                                {
                                    remove_media_paint = Some(paint.id.clone());
                                }
                                if matches!(paint.tag.as_str(), "imagePaint" | "videoPaint") {
                                    let current = paint.string("objectFit").unwrap_or("cover");
                                    let options = [
                                        ("cover", "Cobrir"),
                                        ("contain", "Conter"),
                                        ("fill", "Esticar"),
                                    ];
                                    egui::ComboBox::from_id_salt("paint-fit")
                                        .selected_text(
                                            options
                                                .iter()
                                                .find(|(key, _)| *key == current)
                                                .map_or(current, |(_, label)| *label),
                                        )
                                        .show_ui(ui, |ui| {
                                            for (fit, label) in options {
                                                if ui
                                                    .selectable_label(current == fit, label)
                                                    .clicked()
                                                    && current != fit
                                                {
                                                    fit_media_paint = Some((paint.id.clone(), fit));
                                                }
                                            }
                                        });
                                }
                                if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked() {
                                    move_paint = Some((paint.id.clone(), false));
                                }
                                if ui
                                    .add_enabled(index + 1 < paints.len(), egui::Button::new("↓"))
                                    .clicked()
                                {
                                    move_paint = Some((paint.id.clone(), true));
                                }
                            });
                            let current = paint.string("blendMode");
                            egui::ComboBox::from_id_salt("paint-blend")
                                .selected_text(current.map_or("Mesclagem herdada", |mode| {
                                    BLEND_MODES
                                        .iter()
                                        .find(|(key, _)| *key == mode)
                                        .map_or(mode, |(_, label)| *label)
                                }))
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(current.is_none(), "Mesclagem herdada")
                                        .clicked()
                                        && current.is_some()
                                    {
                                        blend_paint = Some((paint.id.clone(), None));
                                    }
                                    for &(mode, label) in BLEND_MODES {
                                        if ui
                                            .selectable_label(current == Some(mode), label)
                                            .clicked()
                                            && current != Some(mode)
                                        {
                                            blend_paint = Some((paint.id.clone(), Some(mode)));
                                        }
                                    }
                                });
                            if matches!(paint.tag.as_str(), "imagePaint" | "videoPaint") {
                                let draft = self
                                    .media_paint_source_drafts
                                    .entry(paint.id.clone())
                                    .or_insert_with(|| {
                                        paint.string("src").unwrap_or_default().to_string()
                                    });
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(draft)
                                            .hint_text("Arquivo do preenchimento"),
                                    );
                                    if ui.button("Substituir arquivo").clicked() {
                                        replace_media_paint =
                                            Some((paint.id.clone(), draft.clone()));
                                    }
                                });
                            }
                        });
                    }
                },
            );
        });
        if let Some((id, later)) = move_paint {
            self.move_selected_paint(&id, later);
        }
        if let Some((id, enabled)) = enable_paint {
            self.set_selected_paint_enabled(&id, enabled);
        }
        if let Some(id) = remove_media_paint {
            self.remove_selected_media_paint(&id);
        }
        if let Some((id, fit)) = fit_media_paint {
            self.set_selected_media_paint_fit(&id, fit);
        }
        if let Some((id, source)) = replace_media_paint {
            self.replace_selected_media_paint(&id, &source);
        }
        if let Some((id, mode)) = blend_paint {
            self.set_selected_paint_blend(&id, mode);
        }
        let mut edit_stop = None;
        let mut edit_gradient = None;
        let mut change_stops = None;
        let mut edit_geometry = None;
        let refresh_gradients = node.children.iter().any(|gradient| {
            matches!(
                gradient.tag.as_str(),
                "linearGradientPaint" | "radialGradientPaint"
            ) && (self
                .paint_drafts
                .get(&gradient.id)
                .is_none_or(|(at, _)| *at != paint_time)
                || gradient.children.iter().any(|stop| {
                    stop.tag == "colorStop"
                        && self
                            .stop_drafts
                            .get(&stop.id)
                            .is_none_or(|(at, _, _)| *at != paint_time)
                }))
        });
        let refresh_styles = refresh_gradients
            || node.children.iter().any(|style| match style.tag.as_str() {
                "solidPaint" | "imagePaint" | "videoPaint" => self
                    .paint_drafts
                    .get(&style.id)
                    .is_none_or(|(at, _)| *at != paint_time),
                "effect" => self
                    .effect_drafts
                    .get(&style.id)
                    .is_none_or(|(at, _)| *at != paint_time),
                "stroke" if !matches!(node.tag.as_str(), "text" | "textRange") => self
                    .stroke_drafts
                    .get(&style.id)
                    .is_none_or(|(at, _)| *at != paint_time),
                "shadow" if !matches!(node.tag.as_str(), "text" | "textRange") => self
                    .shadow_drafts
                    .get(&style.id)
                    .is_none_or(|(at, _)| *at != paint_time),
                _ => false,
            });
        let sampled_styles = refresh_styles
            .then(|| {
                ad3_editor::motion::Motion::default()
                    .sample(
                        &snapshot.document.root,
                        &snapshot.timeline,
                        paint_time,
                        30.0,
                    )
                    .ok()
            })
            .flatten();
        let mut media_opacity = None;
        for paint in node
            .children
            .iter()
            .filter(|child| matches!(child.tag.as_str(), "imagePaint" | "videoPaint"))
        {
            if self
                .paint_drafts
                .get(&paint.id)
                .is_none_or(|(at, _)| *at != paint_time)
            {
                let values = sampled_styles
                    .as_ref()
                    .and_then(|root| root.find(&paint.id))
                    .unwrap_or(paint);
                self.paint_drafts.insert(
                    paint.id.clone(),
                    (
                        paint_time,
                        PaintDraft {
                            rgb: [0; 3],
                            opacity: values.number("opacity", 1.0).unwrap_or(1.0),
                        },
                    ),
                );
            }
            ui.push_id(("media-opacity", &paint.id), |ui| {
                ui.collapsing(
                    if paint.tag == "videoPaint" {
                        "Opacidade do vídeo"
                    } else {
                        "Opacidade da imagem"
                    },
                    |ui| {
                        ui.add_enabled_ui(
                            snapshot.document.module_sources.is_empty()
                                && self.selection.len() == 1
                                && !self.exporting
                                && !self.capturing,
                            |ui| {
                                let (_, draft) = self.paint_drafts.get_mut(&paint.id).unwrap();
                                ui.add(
                                    egui::Slider::new(&mut draft.opacity, 0.0..=1.0)
                                        .text("Opacidade"),
                                );
                                if ui.button("Aplicar opacidade").clicked() {
                                    media_opacity = Some((paint.id.clone(), draft.opacity));
                                }
                            },
                        );
                    },
                );
            });
        }
        if let Some((id, opacity)) = media_opacity {
            self.set_selected_media_paint_opacity(&id, opacity);
        }
        for gradient in node.children.iter().filter(|child| {
            matches!(
                child.tag.as_str(),
                "linearGradientPaint" | "radialGradientPaint"
            )
        }) {
            if self
                .gradient_geometry_drafts
                .get(&gradient.id)
                .is_none_or(|(at, _)| *at != paint_time)
            {
                let values = sampled_styles
                    .as_ref()
                    .and_then(|root| root.find(&gradient.id))
                    .unwrap_or(gradient);
                let positioned =
                    gradient.props.contains_key("x") || gradient.props.contains_key("y");
                let axis_scale =
                    values.props.contains_key("scaleX") || values.props.contains_key("scaleY");
                let scale = if axis_scale {
                    1.0
                } else {
                    values.number("scale", 1.0).unwrap_or(1.0)
                };
                self.gradient_geometry_drafts.insert(
                    gradient.id.clone(),
                    (
                        paint_time,
                        [
                            values
                                .number("x", if positioned { 0.0 } else { 0.5 })
                                .unwrap_or(0.5),
                            values
                                .number("y", if positioned { 0.0 } else { 0.5 })
                                .unwrap_or(0.5),
                            values.number("scaleX", scale).unwrap_or(scale),
                            values.number("scaleY", scale).unwrap_or(scale),
                            values.number("rotation", 0.0).unwrap_or(0.0),
                        ],
                    ),
                );
            }
            if self
                .paint_drafts
                .get(&gradient.id)
                .is_none_or(|(at, _)| *at != paint_time)
            {
                let values = sampled_styles
                    .as_ref()
                    .and_then(|root| root.find(&gradient.id))
                    .unwrap_or(gradient);
                self.paint_drafts.insert(
                    gradient.id.clone(),
                    (
                        paint_time,
                        PaintDraft {
                            rgb: [0; 3],
                            opacity: values.number("opacity", 1.0).unwrap_or(1.0),
                        },
                    ),
                );
            }
            ui.push_id(&gradient.id, |ui| {
                ui.collapsing(
                    if gradient.tag == "radialGradientPaint" {
                        "Gradiente radial"
                    } else {
                        "Gradiente linear"
                    },
                    |ui| {
                        ui.add_enabled_ui(
                            snapshot.document.module_sources.is_empty()
                                && self.selection.len() == 1
                                && !self.exporting
                                && !self.capturing,
                            |ui| {
                                let (_, draft) = self.paint_drafts.get_mut(&gradient.id).unwrap();
                                ui.add(
                                    egui::Slider::new(&mut draft.opacity, 0.0..=1.0)
                                        .text("Opacidade do gradiente"),
                                );
                                if ui.button("Aplicar opacidade do gradiente").clicked() {
                                    edit_gradient =
                                        Some((gradient.id.clone(), Some(draft.opacity)));
                                }
                                if ui.button("Remover gradiente").clicked() {
                                    edit_gradient = Some((gradient.id.clone(), None));
                                }
                                if ui.button("Adicionar parada de cor").clicked() {
                                    change_stops = Some((gradient.id.clone(), None));
                                }
                                ui.collapsing("Geometria", |ui| {
                                    let (_, values) = self
                                        .gradient_geometry_drafts
                                        .get_mut(&gradient.id)
                                        .unwrap();
                                    for (index, label) in
                                        ["Centro X", "Centro Y", "Escala X", "Escala Y", "Rotação"]
                                            .iter()
                                            .enumerate()
                                    {
                                        ui.horizontal(|ui| {
                                            ui.label(*label);
                                            ui.add(
                                                egui::DragValue::new(&mut values[index])
                                                    .speed(if index == 4 { 1.0 } else { 0.01 }),
                                            );
                                        });
                                    }
                                    if ui.button("Aplicar geometria").clicked() {
                                        edit_geometry = Some((gradient.id.clone(), *values));
                                    }
                                });
                                for stop in gradient
                                    .children
                                    .iter()
                                    .filter(|child| child.tag == "colorStop")
                                {
                                    if self
                                        .stop_drafts
                                        .get(&stop.id)
                                        .is_none_or(|(at, _, _)| *at != paint_time)
                                    {
                                        let values = sampled_styles
                                            .as_ref()
                                            .and_then(|root| root.find(&stop.id))
                                            .unwrap_or(stop);
                                        let color = values
                                            .string("color")
                                            .and_then(ad3_editor::color::parse)
                                            .or_else(|| {
                                                values
                                                    .props
                                                    .get("color")
                                                    .and_then(serde_json::Value::as_f64)
                                                    .and_then(ad3_editor::color::number)
                                            })
                                            .unwrap_or(0);
                                        self.stop_drafts.insert(
                                            stop.id.clone(),
                                            (
                                                paint_time,
                                                PaintDraft {
                                                    rgb: [
                                                        (color >> 16) as u8,
                                                        (color >> 8) as u8,
                                                        color as u8,
                                                    ],
                                                    opacity: values
                                                        .number("opacity", 1.0)
                                                        .unwrap_or(1.0),
                                                },
                                                values.number("offset", 0.0).unwrap_or(0.0),
                                            ),
                                        );
                                    }
                                    let (_, draft, offset) =
                                        self.stop_drafts.get_mut(&stop.id).unwrap();
                                    ui.push_id(&stop.id, |ui| {
                                        ui.color_edit_button_srgb(&mut draft.rgb);
                                        ui.add(
                                            egui::Slider::new(offset, 0.0..=1.0).text("Posição"),
                                        );
                                        ui.add(
                                            egui::Slider::new(&mut draft.opacity, 0.0..=1.0)
                                                .text("Opacidade"),
                                        );
                                        if ui.button("Aplicar parada de cor").clicked() {
                                            edit_stop =
                                                Some((stop.id.clone(), draft.clone(), *offset));
                                        }
                                        if ui.button("Remover parada de cor").clicked() {
                                            change_stops =
                                                Some((gradient.id.clone(), Some(stop.id.clone())));
                                        }
                                        ui.separator();
                                    });
                                }
                            },
                        );
                    },
                );
            });
        }
        if let Some((id, draft, offset)) = edit_stop {
            self.edit_selected_gradient_stop(&id, draft, offset);
        }
        if let Some((id, opacity)) = edit_gradient {
            self.edit_selected_gradient(&id, opacity);
        }
        if let Some((gradient, remove)) = change_stops {
            self.change_selected_gradient_stops(&gradient, remove.as_deref());
        }
        if let Some((id, values)) = edit_geometry {
            self.edit_selected_gradient_geometry(&id, values);
        }
        for paint in node
            .children
            .iter()
            .filter(|child| child.tag == "solidPaint")
        {
            if self
                .paint_drafts
                .get(&paint.id)
                .is_none_or(|(at, _)| *at != paint_time)
            {
                let values = sampled_styles
                    .as_ref()
                    .and_then(|root| root.find(&paint.id))
                    .unwrap_or(paint);
                let color = values
                    .string("color")
                    .and_then(ad3_editor::color::parse)
                    .or_else(|| {
                        values
                            .props
                            .get("color")
                            .and_then(serde_json::Value::as_f64)
                            .and_then(ad3_editor::color::number)
                    })
                    .unwrap_or(0);
                self.paint_drafts.insert(
                    paint.id.clone(),
                    (
                        paint_time,
                        PaintDraft {
                            rgb: [(color >> 16) as u8, (color >> 8) as u8, color as u8],
                            opacity: values.number("opacity", 1.0).unwrap_or(1.0),
                        },
                    ),
                );
            }
            let (_, draft) = self.paint_drafts.get_mut(&paint.id).unwrap();
            ui.push_id(&paint.id, |ui| {
                ui.collapsing("Preenchimento sólido", |ui| {
                    ui.add_enabled_ui(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        |ui| {
                            ui.color_edit_button_srgb(&mut draft.rgb);
                            ui.add(
                                egui::Slider::new(&mut draft.opacity, 0.0..=1.0).text("Opacidade"),
                            );
                            if ui.button("Aplicar preenchimento").clicked() {
                                edit_paint = Some((paint.id.clone(), Some(draft.clone())));
                            }
                            if ui.button("Remover preenchimento").clicked() {
                                edit_paint = Some((paint.id.clone(), None));
                            }
                        },
                    );
                });
            });
        }
        if let Some((id, draft)) = edit_paint {
            self.edit_selected_paint(&id, draft);
        }
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) && ui
            .add_enabled(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                egui::Button::new("Adicionar preenchimento sólido"),
            )
            .clicked()
        {
            self.insert_selection_paint();
        }
        let mut edit_stroke = None;
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) {
            let mut select_mask = None;
            let mut remove_mask = None;
            let mut duplicate_mask = None;
            let mut insert_mask = false;
            ui.collapsing("Máscaras", |ui| {
                ui.add_enabled_ui(
                    snapshot.document.module_sources.is_empty()
                        && self.selection.len() == 1
                        && !self.exporting
                        && !self.capturing,
                    |ui| {
                        for mask in node
                            .children
                            .iter()
                            .filter(|child| child.tag == "rect" && child.flag("mask"))
                        {
                            ui.push_id(("mask", &mask.id), |ui| {
                                ui.horizontal(|ui| {
                                    if ui.button("Duplicar").clicked() {
                                        duplicate_mask = Some(mask.id.clone());
                                    }
                                    if ui
                                        .button(mask.string("name").unwrap_or("Selecionar máscara"))
                                        .clicked()
                                    {
                                        select_mask = Some(mask.id.clone());
                                    }
                                    if ui.button("Remover").clicked() {
                                        remove_mask = Some(mask.id.clone());
                                    }
                                });
                            });
                        }
                        insert_mask = ui.button("Adicionar máscara").clicked();
                    },
                );
            });
            if let Some(id) = remove_mask {
                self.remove_selected_mask(&id);
            }
            if let Some(id) = duplicate_mask {
                self.duplicate_selected_mask(&id);
            }
            if insert_mask {
                self.insert_selected_mask();
            }
            if let Some(id) = select_mask {
                self.selected = id.clone();
                self.selection = vec![id];
            }
        }
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) {
            let mut insert_media_paint = None;
            ui.collapsing("Preenchimento de mídia", |ui| {
                ui.add_enabled_ui(
                    snapshot.document.module_sources.is_empty()
                        && !self.exporting
                        && !self.capturing,
                    |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.media_path)
                                .hint_text("Caminho do arquivo local"),
                        );
                        if ui.button("Adicionar imagem como preenchimento").clicked() {
                            insert_media_paint = Some(false);
                        }
                        if ui.button("Adicionar vídeo como preenchimento").clicked() {
                            insert_media_paint = Some(true);
                        }
                    },
                );
            });
            if let Some(video) = insert_media_paint {
                self.insert_selection_media_paint(video);
            }
        }
        let mut stroke_shape = None;
        let mut outline_stack = None;
        let mut outline_blend = None;
        for (tag, label) in [
            ("stroke", "Pilha de contornos"),
            ("shadow", "Pilha de sombras"),
        ] {
            ui.collapsing(label, |ui| {
                ui.add_enabled_ui(
                    snapshot.document.module_sources.is_empty()
                        && self.selection.len() == 1
                        && !self.exporting
                        && !self.capturing,
                    |ui| {
                        let styles = node
                            .children
                            .iter()
                            .filter(|child| child.tag == tag)
                            .collect::<Vec<_>>();
                        for (index, style) in styles.iter().enumerate() {
                            ui.push_id(("outline-stack", &style.id), |ui| {
                                ui.horizontal(|ui| {
                                    let mut enabled = !style.flag("hidden");
                                    if ui.checkbox(&mut enabled, "Ativo").changed() {
                                        outline_stack =
                                            Some((style.id.clone(), Some(enabled), false));
                                    }
                                    ui.label(format!(
                                        "{} {}",
                                        if tag == "stroke" {
                                            "Contorno"
                                        } else {
                                            "Sombra"
                                        },
                                        index + 1
                                    ));
                                    if ui.button("Duplicar").clicked() {
                                        duplicate_style = Some(style.id.clone());
                                    }
                                    if ui.add_enabled(index > 0, egui::Button::new("↑")).clicked()
                                    {
                                        outline_stack = Some((style.id.clone(), None, false));
                                    }
                                    if ui
                                        .add_enabled(
                                            index + 1 < styles.len(),
                                            egui::Button::new("↓"),
                                        )
                                        .clicked()
                                    {
                                        outline_stack = Some((style.id.clone(), None, true));
                                    }
                                });
                                if tag == "stroke" {
                                    let current = style.string("blendMode");
                                    egui::ComboBox::from_id_salt("stroke-blend")
                                        .selected_text(current.map_or(
                                            "Mesclagem herdada",
                                            |mode| {
                                                BLEND_MODES
                                                    .iter()
                                                    .find(|(key, _)| *key == mode)
                                                    .map_or(mode, |(_, label)| *label)
                                            },
                                        ))
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    current.is_none(),
                                                    "Mesclagem herdada",
                                                )
                                                .clicked()
                                                && current.is_some()
                                            {
                                                outline_blend = Some((style.id.clone(), None));
                                            }
                                            for &(mode, label) in BLEND_MODES {
                                                if ui
                                                    .selectable_label(current == Some(mode), label)
                                                    .clicked()
                                                    && current != Some(mode)
                                                {
                                                    outline_blend =
                                                        Some((style.id.clone(), Some(mode)));
                                                }
                                            }
                                        });
                                }
                            });
                        }
                    },
                );
            });
        }
        if let Some((id, enabled, later)) = outline_stack {
            self.change_selected_outline_stack(&id, enabled, later);
        }
        if let Some((id, mode)) = outline_blend {
            self.set_selected_paint_blend(&id, mode);
        }
        if let Some(id) = duplicate_style {
            self.duplicate_selected_style(&id);
        }
        for stroke in node.children.iter().filter(|child| child.tag == "stroke") {
            self.refresh_stroke_draft_sampled(node, stroke, sampled_styles.as_ref());
            let (_, draft) = self.stroke_drafts.get_mut(&stroke.id).unwrap();
            ui.push_id(&stroke.id, |ui| {
                ui.collapsing("Contorno", |ui| {
                    ui.add_enabled_ui(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        |ui| {
                            for (property, label, default, options) in [
                                (
                                    "join",
                                    "Junção",
                                    "miter",
                                    [
                                        ("miter", "Angular"),
                                        ("round", "Arredondada"),
                                        ("bevel", "Chanfrada"),
                                    ],
                                ),
                                (
                                    "cap",
                                    "Ponta",
                                    "butt",
                                    [
                                        ("butt", "Reta"),
                                        ("round", "Arredondada"),
                                        ("square", "Quadrada"),
                                    ],
                                ),
                            ] {
                                let current = stroke.string(property).unwrap_or(default);
                                egui::ComboBox::from_id_salt(property)
                                    .selected_text(format!(
                                        "{label}: {}",
                                        options
                                            .iter()
                                            .find(|(key, _)| *key == current)
                                            .map_or(current, |(_, label)| *label)
                                    ))
                                    .show_ui(ui, |ui| {
                                        for (value, label) in options {
                                            if ui
                                                .selectable_label(current == value, label)
                                                .clicked()
                                                && current != value
                                            {
                                                stroke_shape =
                                                    Some((stroke.id.clone(), property, value));
                                            }
                                        }
                                    });
                            }
                            ui.color_edit_button_srgb(&mut draft.rgb);
                            ui.add(
                                egui::DragValue::new(&mut draft.width)
                                    .speed(0.5)
                                    .range(0.0..=8192.0)
                                    .prefix("Espessura: ")
                                    .suffix(" px"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.miter_limit)
                                    .speed(0.1)
                                    .range(0.0..=1_000_000.0)
                                    .prefix("Limite da junção: "),
                            );
                            ui.add(
                                egui::Slider::new(&mut draft.opacity, 0.0..=1.0).text("Opacidade"),
                            );
                            if ui.button("Aplicar contorno").clicked() {
                                edit_stroke = Some((stroke.id.clone(), Some(draft.clone())));
                            }
                            if ui.button("Remover contorno").clicked() {
                                edit_stroke = Some((stroke.id.clone(), None));
                            }
                        },
                    );
                });
            });
        }
        if let Some((id, draft)) = edit_stroke {
            self.edit_selected_stroke(&id, draft);
        }
        if let Some((id, property, value)) = stroke_shape {
            self.edit_selected_stroke_shape(&id, property, value);
        }
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) && ui
            .add_enabled(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                egui::Button::new("Adicionar contorno"),
            )
            .clicked()
        {
            self.insert_selection_stroke();
        }
        let mut edit_shadow = None;
        for shadow in node.children.iter().filter(|child| child.tag == "shadow") {
            self.refresh_shadow_draft_sampled(node, shadow, sampled_styles.as_ref());
            let (_, draft) = self.shadow_drafts.get_mut(&shadow.id).unwrap();
            ui.push_id(&shadow.id, |ui| {
                ui.collapsing("Sombra", |ui| {
                    ui.add_enabled_ui(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        |ui| {
                            ui.color_edit_button_srgb(&mut draft.rgb);
                            ui.add(
                                egui::Slider::new(&mut draft.opacity, 0.0..=1.0).text("Opacidade"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.blur)
                                    .range(0.0..=512.0)
                                    .speed(1.0)
                                    .prefix("Desfoque: ")
                                    .suffix(" px"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.offset[0])
                                    .range(-1e6..=1e6)
                                    .speed(1.0)
                                    .prefix("X: ")
                                    .suffix(" px"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.offset[1])
                                    .range(-1e6..=1e6)
                                    .speed(1.0)
                                    .prefix("Y: ")
                                    .suffix(" px"),
                            );
                            if ui.button("Aplicar sombra").clicked() {
                                edit_shadow = Some((shadow.id.clone(), Some(draft.clone())));
                            }
                            if ui.button("Remover sombra").clicked() {
                                edit_shadow = Some((shadow.id.clone(), None));
                            }
                        },
                    );
                });
            });
        }
        if let Some((id, draft)) = edit_shadow {
            self.edit_selected_shadow(&id, draft);
        }
        if matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) && ui
            .add_enabled(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                egui::Button::new("Adicionar sombra"),
            )
            .clicked()
        {
            self.insert_selection_shadow();
        }
        let time = self
            .transport
            .as_ref()
            .map_or(0.0, |transport| transport.time);
        let mut edit_effect = None;
        let mut duplicate_effect = None;
        let mut change_effect = None;
        let mut effect_type = None;
        for effect in node.children.iter().filter(|child| child.tag == "effect") {
            let needs_refresh = self
                .effect_drafts
                .get(&effect.id)
                .is_none_or(|(at, _)| *at != time);
            if needs_refresh {
                let value = sampled_styles
                    .as_ref()
                    .and_then(|root| {
                        root.find(&effect.id)
                            .and_then(|node| node.number("value", 0.0).ok())
                    })
                    .unwrap_or_else(|| effect.number("value", 0.0).unwrap_or(0.0));
                self.effect_drafts.insert(effect.id.clone(), (time, value));
            }
            ui.push_id(&effect.id, |ui| {
                ui.collapsing(
                    effect_label(effect.string("type").unwrap_or("blur")),
                    |ui| {
                        ui.add_enabled_ui(
                            snapshot.document.module_sources.is_empty()
                                && self.selection.len() == 1
                                && !self.exporting
                                && !self.capturing,
                            |ui| {
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("effect-type")
                                        .selected_text(effect_label(
                                            effect.string("type").unwrap_or("blur"),
                                        ))
                                        .show_ui(ui, |ui| {
                                            for &(kind, _, _) in EFFECT_OPTIONS {
                                                if ui
                                                    .selectable_label(
                                                        effect.string("type") == Some(kind),
                                                        effect_label(kind),
                                                    )
                                                    .clicked()
                                                {
                                                    effect_type = Some((effect.id.clone(), kind));
                                                }
                                            }
                                        });
                                    let mut enabled = !effect.flag("hidden");
                                    if ui.checkbox(&mut enabled, "Ativo").changed() {
                                        change_effect =
                                            Some((effect.id.clone(), Some(enabled), false));
                                    }
                                    if ui
                                        .small_button("↑")
                                        .on_hover_text("Aplicar antes")
                                        .clicked()
                                    {
                                        change_effect = Some((effect.id.clone(), None, false));
                                    }
                                    if ui
                                        .small_button("↓")
                                        .on_hover_text("Aplicar depois")
                                        .clicked()
                                    {
                                        change_effect = Some((effect.id.clone(), None, true));
                                    }
                                });
                                let (_, value) = self.effect_drafts.get_mut(&effect.id).unwrap();
                                let range = match effect.string("type") {
                                    Some("blur") => 0.0..=256.0,
                                    Some("hueRotate") => -360.0..=360.0,
                                    _ => 0.0..=1.0,
                                };
                                ui.add(
                                    egui::DragValue::new(value)
                                        .speed(
                                            if matches!(
                                                effect.string("type"),
                                                Some("blur" | "hueRotate")
                                            ) {
                                                1.0
                                            } else {
                                                0.01
                                            },
                                        )
                                        .range(range)
                                        .prefix(match effect.string("type") {
                                            Some("blur") => "Raio: ",
                                            Some("hueRotate") => "Ângulo: ",
                                            _ => "Intensidade: ",
                                        })
                                        .suffix(match effect.string("type") {
                                            Some("blur") => " px",
                                            Some("hueRotate") => "°",
                                            _ => "",
                                        }),
                                );
                                if ui.button("Aplicar efeito").clicked() {
                                    edit_effect = Some((effect.id.clone(), Some(*value)));
                                }
                                if ui.button("Remover efeito").clicked() {
                                    edit_effect = Some((effect.id.clone(), None));
                                }
                                if ui.button("Duplicar efeito").clicked() {
                                    duplicate_effect = Some(effect.id.clone());
                                }
                            },
                        );
                    },
                );
            });
        }
        if let Some((id, enabled, later)) = change_effect {
            self.change_effect_stack(&id, enabled, later);
        }
        if let Some((id, value)) = edit_effect {
            self.edit_selected_effect(&id, value);
        }
        if let Some(id) = duplicate_effect {
            self.duplicate_selected_style(&id);
        }
        if let Some((id, kind)) = effect_type {
            self.change_selected_effect_type(&id, kind);
        }
        if node.is_timed() && !matches!(node.tag.as_str(), "stage" | "sequence" | "audio") {
            let mut effect = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    ui.menu_button("Adicionar efeito", |ui| {
                        for &(kind, _, value) in EFFECT_OPTIONS {
                            if ui.button(effect_label(kind)).clicked() {
                                effect = Some((kind, value));
                                ui.close();
                            }
                        }
                    });
                },
            );
            if let Some((kind, value)) = effect {
                self.insert_selection_effect(kind, value);
            }
        }
        if node.tag != "audio" {
            self.refresh_opacity_draft(&snapshot, &node.id);
            let mut apply_opacity = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    let (_, _, opacity) = self.opacity_draft.as_mut().unwrap();
                    let mut percent = *opacity * 100.0;
                    ui.add(
                        egui::Slider::new(&mut percent, 0.0..=100.0)
                            .text("Opacidade")
                            .suffix("%"),
                    );
                    *opacity = percent / 100.0;
                    if ui.button("Aplicar opacidade").clicked() {
                        apply_opacity = Some(*opacity);
                    }
                },
            );
            if let Some(opacity) = apply_opacity {
                self.set_selection_opacity(opacity);
            }
        }
        if node.tag != "audio" {
            let current = node.string("blendMode");
            let label = current
                .and_then(|current| {
                    BLEND_MODES
                        .iter()
                        .find(|(value, _)| *value == current)
                        .map(|(_, label)| *label)
                })
                .unwrap_or("Padrão / herdado");
            let mut blend = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    egui::ComboBox::from_id_salt("blend-mode-selector")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(current.is_none(), "Padrão / herdado")
                                .clicked()
                            {
                                blend = Some(None);
                            }
                            for (value, label) in BLEND_MODES {
                                if ui
                                    .selectable_label(current == Some(*value), *label)
                                    .clicked()
                                {
                                    blend = Some(Some(*value));
                                }
                            }
                        });
                },
            );
            if let Some(mode) = blend {
                self.set_selection_blend(mode);
            }
        }
        let mut remove_animation = None;
        let mut duplicate_animation = None;
        let mut update_animation = None;
        let mut change_preset = None;
        let mut move_animation = None;
        for animation in node
            .children
            .iter()
            .filter(|child| child.tag == "animation")
        {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(
                    snapshot.document.module_sources.is_empty()
                        && self.selection.len() == 1
                        && !self.exporting
                        && !self.capturing,
                    |ui| {
                        if ui.button("Duplicar").clicked() {
                            duplicate_animation = Some(animation.id.clone());
                        }
                        if ui
                            .small_button("↑")
                            .on_hover_text("Aplicar antes")
                            .clicked()
                        {
                            move_animation = Some((animation.id.clone(), false));
                        }
                        if ui
                            .small_button("↓")
                            .on_hover_text("Aplicar depois")
                            .clicked()
                        {
                            move_animation = Some((animation.id.clone(), true));
                        }
                    },
                );
                ui.label(format!(
                    "{} · {} · {:.2} s",
                    animation_label(animation.string("type").unwrap_or("fade")),
                    if animation.string("phase") == Some("out") {
                        "Saída"
                    } else {
                        "Entrada"
                    },
                    animation.number("duration", 1.0).unwrap_or(1.0)
                ));
                if ui
                    .add_enabled(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        egui::Button::new("Remover"),
                    )
                    .clicked()
                {
                    remove_animation = Some(animation.id.clone());
                }
            });
            ui.push_id(&animation.id, |ui| {
                ui.collapsing("Editar animação", |ui| {
                    let current = animation.string("type").unwrap_or("fade");
                    ui.add_enabled_ui(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        |ui| {
                            egui::ComboBox::from_id_salt("existing-animation-preset")
                                .selected_text(animation_label(current))
                                .show_ui(ui, |ui| {
                                    for &(kind, label) in ANIMATION_OPTIONS {
                                        if !animation_available(node, kind, Some(current)) {
                                            continue;
                                        }
                                        if ui.selectable_label(current == kind, label).clicked()
                                            && current != kind
                                        {
                                            change_preset = Some((animation.id.clone(), kind));
                                        }
                                    }
                                });
                        },
                    );
                    ui.add_enabled_ui(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        |ui| {
                            let draft = self
                                .animation_drafts
                                .entry(animation.id.clone())
                                .or_insert((
                                    animation.string("phase") == Some("out"),
                                    animation.number("duration", 1.0).unwrap_or(1.0),
                                    animation.number("delay", 0.0).unwrap_or(0.0),
                                ));
                            ui.checkbox(&mut draft.0, "Animação de saída");
                            ui.add(
                                egui::DragValue::new(&mut draft.1)
                                    .speed(0.1)
                                    .range(1.0 / 30.0..=86400.0)
                                    .prefix("Duração: ")
                                    .suffix(" s"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut draft.2)
                                    .speed(0.1)
                                    .range(0.0..=86400.0)
                                    .prefix("Atraso: ")
                                    .suffix(" s"),
                            );
                            if ui.button("Aplicar animação").clicked() {
                                update_animation = Some((animation.id.clone(), *draft));
                            }
                        },
                    );
                });
            });
        }
        if let Some((id, kind)) = change_preset {
            self.change_selected_animation_preset(&id, kind);
        }
        if let Some((id, later)) = move_animation {
            self.move_selected_animation(&id, later);
        }
        if let Some((id, (out, duration, delay))) = update_animation {
            self.update_selected_animation(&id, out, duration, delay);
        }
        if let Some(id) = remove_animation {
            self.remove_selected_animation(&id);
        }
        if let Some(id) = duplicate_animation {
            self.duplicate_selected_style(&id);
        }
        if node.is_timed() && !matches!(node.tag.as_str(), "stage" | "sequence") {
            let mut preset = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    ui.menu_button("Adicionar animação", |ui| {
                        ui.checkbox(&mut self.animation_out, "Animação de saída");
                        ui.add(
                            egui::DragValue::new(&mut self.animation_duration)
                                .speed(0.1)
                                .range(1.0 / 30.0..=86400.0)
                                .prefix("Duração: ")
                                .suffix(" s"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.animation_delay)
                                .speed(0.1)
                                .range(0.0..=86400.0)
                                .prefix("Atraso: ")
                                .suffix(" s"),
                        );
                        for &(kind, label) in ANIMATION_OPTIONS {
                            if !animation_available(node, kind, None) {
                                continue;
                            }
                            if ui.button(label).clicked() {
                                preset = Some(kind);
                                ui.close();
                            }
                        }
                    });
                },
            );
            if let Some(kind) = preset {
                self.insert_selection_animation(kind);
            }
        }
        if matches!(
            node.tag.as_str(),
            "audio" | "video" | "scene" | "group" | "sequence"
        ) || node.props.contains_key("volume")
        {
            self.refresh_volume_draft(&snapshot, &node.id);
            let mut apply = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Volume");
                        let (_, _, db) = self.volume_draft.as_mut().unwrap();
                        ui.add(
                            egui::DragValue::new(db)
                                .speed(0.1)
                                .range(-60.0..=12.0)
                                .suffix(" dB"),
                        );
                        if ui.button("Aplicar volume").clicked() {
                            apply = Some(*db);
                        }
                    });
                },
            );
            if let Some(db) = apply {
                self.set_selection_volume(db);
            }
        }
        if matches!(
            node.tag.as_str(),
            "image" | "video" | "audio" | "imagePaint" | "videoPaint"
        ) {
            if self
                .media_source_draft
                .as_ref()
                .is_none_or(|(id, _)| id != &node.id)
            {
                self.media_source_draft =
                    Some((node.id.clone(), node.string("src").unwrap_or("").into()));
            }
            let mut replace = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty()
                    && self.selection.len() == 1
                    && !self.exporting
                    && !self.capturing,
                |ui| {
                    ui.label("Arquivo de mídia");
                    let (_, source) = self.media_source_draft.as_mut().unwrap();
                    ui.add(egui::TextEdit::singleline(source).desired_width(220.0));
                    if ui.button("Substituir arquivo").clicked() {
                        replace = Some(source.clone());
                    }
                },
            );
            if let Some(source) = replace {
                self.replace_selected_media(&source);
            }
            if matches!(node.tag.as_str(), "image" | "video")
                && ui
                    .add_enabled(
                        snapshot.document.module_sources.is_empty()
                            && self.selection.len() == 1
                            && !self.exporting
                            && !self.capturing,
                        egui::Button::new("Remover mídia do elemento"),
                    )
                    .clicked()
            {
                self.remove_selected_intrinsic_media();
            }
        }
        if matches!(
            node.tag.as_str(),
            "image" | "video" | "imagePaint" | "videoPaint"
        ) {
            let current = node.string("objectFit").unwrap_or("cover");
            let mut fit = current.to_string();
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && !self.exporting && !self.capturing,
                |ui| {
                    egui::ComboBox::from_id_salt("media-fit")
                        .selected_text(match current {
                            "contain" => "Conter",
                            "fill" => "Esticar",
                            _ => "Cobrir",
                        })
                        .show_ui(ui, |ui| {
                            for (value, label) in [
                                ("cover", "Cobrir"),
                                ("contain", "Conter"),
                                ("fill", "Esticar"),
                            ] {
                                ui.selectable_value(&mut fit, value.into(), label);
                            }
                        });
                },
            );
            if fit != current {
                self.set_media_fit(&fit);
            }
        }
        if self
            .clip_name_draft
            .as_ref()
            .is_none_or(|(id, _)| id != &node.id)
        {
            self.clip_name_draft = Some((
                node.id.clone(),
                node.string("name").unwrap_or(&node.id).into(),
            ));
        }
        let mut rename = None;
        ui.add_enabled_ui(
            snapshot.document.module_sources.is_empty()
                && self.selection.len() == 1
                && !self.exporting
                && !self.capturing,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label("Nome");
                    let (_, name) = self.clip_name_draft.as_mut().unwrap();
                    ui.add(egui::TextEdit::singleline(name).desired_width(130.0));
                    if ui.button("Renomear").clicked() {
                        rename = Some(name.clone());
                    }
                });
            },
        );
        if let Some(name) = rename {
            self.rename_selected_clip(&name);
        }
        if matches!(node.tag.as_str(), "text" | "textRange") {
            if node.tag == "text" {
                let fixed = node.props.contains_key("width") || node.props.contains_key("height");
                let mut requested = fixed;
                ui.add_enabled_ui(
                    snapshot.document.module_sources.is_empty()
                        && self.selection.len() == 1
                        && !self.exporting
                        && !self.capturing,
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Caixa do texto");
                            ui.selectable_value(&mut requested, false, "Automática");
                            ui.selectable_value(&mut requested, true, "Fixa");
                        });
                    },
                );
                if requested != fixed {
                    self.set_selected_text_box(requested);
                }
            }
            ui.menu_button("Tipografia", |ui| {
                if node.tag == "text" && ui.button("Entrelinhas").clicked() {
                    self.property = "leading".into();
                    self.value = serde_json::json!(
                        node.number("leading", 1.0)
                            .ok()
                            .filter(|value| *value > 0.0)
                            .unwrap_or(1.0)
                    )
                    .to_string();
                    ui.close();
                }
                for (property, label, default) in [
                    ("fontSize", "Tamanho", serde_json::json!(16)),
                    ("fontWeight", "Peso", serde_json::json!("normal")),
                    ("fontStyle", "Estilo", serde_json::json!("normal")),
                    ("textCase", "Caixa do texto", serde_json::json!("original")),
                    (
                        "letterSpacing",
                        "Espaçamento entre letras",
                        serde_json::json!(0),
                    ),
                    ("textAlign", "Alinhamento", serde_json::json!("left")),
                    (
                        "textBaseline",
                        "Linha de base",
                        serde_json::json!("alphabetic"),
                    ),
                    ("color", "Cor do texto", serde_json::json!("white")),
                ] {
                    if ui.button(label).clicked() {
                        self.property = property.into();
                        self.value = self
                            .text_property_at_playhead(&snapshot, &node.id, property, &default)
                            .to_string();
                        ui.close();
                    }
                }
            });
            if ui.button("Escolher fonte").clicked() {
                self.property = "fontFamily".into();
                self.value = self
                    .text_property_at_playhead(
                        &snapshot,
                        &node.id,
                        "fontFamily",
                        &serde_json::json!("Inter"),
                    )
                    .to_string();
            }
            if self
                .text_draft
                .as_ref()
                .is_none_or(|(id, _)| id != &node.id)
            {
                self.text_draft = Some((node.id.clone(), node.text.clone()));
            }
            let mut apply = None;
            ui.add_enabled_ui(
                snapshot.document.module_sources.is_empty() && self.selection.len() == 1,
                |ui| {
                    let (_, text) = self.text_draft.as_mut().unwrap();
                    ui.add(egui::TextEdit::multiline(text).desired_rows(3));
                    if ui.button("Aplicar texto").clicked() {
                        apply = Some(text.clone());
                    }
                },
            );
            if let Some(text) = apply {
                self.commit(snapshot.document.set_text(&node.id, &text));
            }
        }
        ui.menu_button("Cor dos clipes", |ui| {
            for (label, color) in COLORS {
                if ui.button(label).clicked() {
                    self.set_selection_color(Some(color));
                    ui.close();
                }
            }
            if ui.button("Restaurar cor padrão").clicked() {
                self.set_selection_color(None);
                ui.close();
            }
        });
        let mut hidden = node.flag("hidden");
        let mut muted = node.flag("muted");
        if ui.checkbox(&mut hidden, "Ocultar seleção").changed() {
            self.set_selection_flag("hidden", hidden);
        }
        if ui.checkbox(&mut muted, "Silenciar seleção").changed() {
            self.set_selection_flag("muted", muted);
        }
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for (name, value) in &node.props {
                    if ui
                        .selectable_label(self.property == *name, format!("{name}: {value}"))
                        .clicked()
                    {
                        self.property = name.clone();
                        self.value = value.to_string();
                    }
                }
            });
        ui.label("Propriedade");
        ui.text_edit_singleline(&mut self.property);
        if matches!(node.tag.as_str(), "text" | "textRange")
            && matches!(
                self.property.as_str(),
                "fontFamily"
                    | "fontSize"
                    | "fontWeight"
                    | "fontStyle"
                    | "letterSpacing"
                    | "textCase"
                    | "textAlign"
                    | "textBaseline"
                    | "color"
            )
            && ui
                .add_enabled(
                    snapshot.document.module_sources.is_empty()
                        && !self.exporting
                        && !self.capturing,
                    egui::Button::new("Restaurar estilo herdado"),
                )
                .clicked()
        {
            self.restore_text_inheritance();
        }
        ui.label("Valor JSON");
        ui.text_edit_singleline(&mut self.value);
        if self.property == "fontFamily" {
            if self.font_families.is_none() {
                match self.font_book.available_families(None) {
                    Ok(families) => self.font_families = Some(families),
                    Err(error) => {
                        self.status = error;
                        self.font_families = Some(Vec::new());
                    }
                }
            }
            let current = serde_json::from_str::<String>(&self.value).unwrap_or_default();
            ui.add(
                egui::TextEdit::singleline(&mut self.font_search)
                    .hint_text("Buscar fonte")
                    .desired_width(200.0),
            );
            let query = self.font_search.trim().to_lowercase();
            egui::ComboBox::from_id_salt("font-family-picker")
                .selected_text(&current)
                .show_ui(ui, |ui| {
                    let mut found = false;
                    for family in self
                        .font_families
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|family| family.to_lowercase().contains(&query))
                    {
                        found = true;
                        if ui.selectable_label(current == *family, family).clicked() {
                            self.value = serde_json::json!(family).to_string();
                        }
                    }
                    if !found {
                        ui.label("Nenhuma fonte encontrada.");
                    }
                });
            if ui.button("Atualizar fontes disponíveis").clicked() {
                self.font_families = None;
                self.font_book = ad3_editor::fonts::FontBook::default();
                self.font_weights.clear();
            }
        }
        let weight_family = if self.property == "fontWeight" {
            let family = inherited_text_property(
                &snapshot.document.root,
                &node.id,
                "fontFamily",
                &serde_json::json!("Inter"),
            )
            .as_str()
            .unwrap_or("Inter")
            .to_string();
            if !self.font_weights.contains_key(&family) {
                let weights = match self.font_book.available_weights(&family, None) {
                    Ok(weights) => weights,
                    Err(error) => {
                        self.status = error;
                        Vec::new()
                    }
                };
                self.font_weights.insert(family.clone(), weights);
            }
            Some(family)
        } else {
            None
        };
        let options: &[(&str, &str)] = match self.property.as_str() {
            "textCase" => &[
                ("original", "Original"),
                ("upper", "Maiúsculas"),
                ("lower", "Minúsculas"),
            ],
            "textAlign" => &[
                ("left", "Esquerda"),
                ("center", "Centro"),
                ("right", "Direita"),
            ],
            "textBaseline" => &[
                ("top", "Topo"),
                ("middle", "Meio"),
                ("bottom", "Base"),
                ("alphabetic", "Linha de base"),
            ],
            "fontWeight" => &[
                ("100", "Fino · 100"),
                ("200", "Extraleve · 200"),
                ("300", "Leve · 300"),
                ("400", "Normal · 400"),
                ("500", "Médio · 500"),
                ("600", "Seminegrito · 600"),
                ("700", "Negrito · 700"),
                ("800", "Extranegrito · 800"),
                ("900", "Pesado · 900"),
            ],
            "fontStyle" => &[
                ("normal", "Normal"),
                ("italic", "Itálico"),
                ("oblique", "Oblíquo"),
            ],
            _ => &[],
        };
        if !options.is_empty() {
            let parsed = serde_json::from_str::<serde_json::Value>(&self.value).ok();
            let mut selected = parsed
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&self.value)
                .to_string();
            if self.property == "fontWeight" {
                selected = match selected.as_str() {
                    "normal" => "400".into(),
                    "bold" => "700".into(),
                    _ => selected,
                };
            }
            let label = options
                .iter()
                .find(|(value, _)| *value == selected)
                .map_or(selected.as_str(), |(_, label)| *label)
                .to_string();
            egui::ComboBox::from_id_salt("property-options")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (value, label) in options {
                        if let Some(weights) = weight_family
                            .as_ref()
                            .and_then(|family| self.font_weights.get(family))
                            && !weights.is_empty()
                            && value
                                .parse::<u16>()
                                .is_ok_and(|weight| !weights.contains(&weight))
                        {
                            continue;
                        }
                        if ui.selectable_label(selected == *value, *label).clicked() {
                            self.value = if self.property == "fontWeight" {
                                // JSX fontWeight uses CSS numbers; aliases are
                                // recognized above without rewriting on display.
                                value.to_string()
                            } else {
                                serde_json::json!(value).to_string()
                            };
                        }
                    }
                });
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.value) {
            if let Some(mut number) = value.as_f64().filter(|number| number.is_finite()) {
                let leading = self.property == "leading";
                if leading {
                    number = (number * 100.0).round();
                }
                ui.horizontal(|ui| {
                    ui.label("Valor");
                    let mut control = egui::DragValue::new(&mut number).speed(
                        if matches!(
                            self.property.as_str(),
                            "opacity" | "scale" | "scaleX" | "scaleY" | "playbackRate"
                        ) {
                            0.01
                        } else if self.property == "letterSpacing" {
                            0.1
                        } else {
                            1.0
                        },
                    );
                    if self.property == "opacity" {
                        control = control.range(0.0..=1.0);
                    } else if self.property == "fontSize" {
                        control = control.range(1.0..=8192.0);
                    } else if self.property == "fontWeight" {
                        control = control.range(1.0..=1000.0);
                    } else if leading {
                        control = control.range(0.0..=1200.0).suffix(" %");
                    }
                    if ui.add(control).changed() {
                        self.value =
                            serde_json::json!(if leading { number / 100.0 } else { number })
                                .to_string();
                    }
                });
            } else if let Some(values) = value.as_array().filter(|values| values.len() == 2) {
                if let (Some(x), Some(y)) = (values[0].as_f64(), values[1].as_f64()) {
                    if x.is_finite() && y.is_finite() {
                        let mut pair = [x, y];
                        let speed = if self.property == "scale" { 0.01 } else { 1.0 };
                        ui.horizontal(|ui| {
                            let x_changed = ui
                                .add(
                                    egui::DragValue::new(&mut pair[0])
                                        .speed(speed)
                                        .prefix("X: "),
                                )
                                .changed();
                            let y_changed = ui
                                .add(
                                    egui::DragValue::new(&mut pair[1])
                                        .speed(speed)
                                        .prefix("Y: "),
                                )
                                .changed();
                            if x_changed || y_changed {
                                self.value = serde_json::json!(pair).to_string();
                            }
                        });
                    }
                }
            }
        }
        if matches!(self.property.as_str(), "fill" | "color") {
            let packed = serde_json::from_str::<serde_json::Value>(&self.value)
                .ok()
                .and_then(|value| match value {
                    serde_json::Value::String(text) => ad3_editor::color::parse(&text),
                    serde_json::Value::Number(number) => {
                        number.as_f64().and_then(ad3_editor::color::number)
                    }
                    _ => None,
                });
            if let Some(packed) = packed {
                let mut rgb = [(packed >> 16) as u8, (packed >> 8) as u8, packed as u8];
                ui.horizontal(|ui| {
                    ui.label("Cor");
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        self.value = serde_json::json!(format!(
                            "#{:02x}{:02x}{:02x}",
                            rgb[0], rgb[1], rgb[2]
                        ))
                        .to_string();
                    }
                });
            }
        }
        if ui.button("Aplicar").clicked() {
            if self.property == "leading" {
                match serde_json::from_str::<f64>(&self.value) {
                    Ok(value) => self.set_selection_text_leading(value * 100.0),
                    Err(_) => self.status = "Entrelinhas deve ser um número.".into(),
                }
            } else {
                let result = serde_json::from_str::<serde_json::Value>(&self.value)
                    .map_err(|e| e.to_string())
                    .and_then(|value| {
                        self.selection
                            .iter()
                            .try_fold(snapshot.document.clone(), |document, id| {
                                document.set_at_playhead(
                                    &snapshot.timeline,
                                    id,
                                    &self.property,
                                    value.clone(),
                                    self.transport
                                        .as_ref()
                                        .map_or(0.0, |transport| transport.time),
                                )
                            })
                    });
                self.commit(result);
            }
        }
        if let Ok(points) =
            snapshot
                .document
                .keyframe_points(&snapshot.timeline, &self.selected, &self.property)
            && !points.is_empty()
        {
            ui.label("Keyframes — tempo na cena");
            for (id, time, value) in points {
                if ui.button(format!("{time:.3}s: {value}")).clicked() {
                    if let Some(transport) = &mut self.transport {
                        let _ = transport.seek(time);
                    }
                    self.value = value.to_string();
                    self.invalidate();
                }
                let draft_time = self.keyframe_time_drafts.entry(id.clone()).or_insert(time);
                let mut move_point = false;
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(draft_time)
                            .speed(1.0 / 30.0)
                            .prefix("Tempo: ")
                            .suffix(" s"),
                    );
                    move_point = ui.button("Mover ponto").clicked();
                });
                if move_point {
                    let at = *draft_time;
                    self.commit(snapshot.document.move_keyframe(&snapshot.timeline, &id, at));
                }
                let authored_easing = snapshot
                    .document
                    .root
                    .find(&id)
                    .and_then(|point| point.string("easing"))
                    .unwrap_or("linear");
                let draft = self
                    .easing_drafts
                    .entry(id.clone())
                    .or_insert_with(|| authored_easing.into());
                let mut apply = false;
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("keyframe-easing", &id))
                        .selected_text(draft.as_str())
                        .show_ui(ui, |ui| {
                            for easing in [
                                "linear",
                                "easeIn",
                                "easeOut",
                                "easeInOut",
                                "gentle",
                                "snappy",
                                "bouncy",
                                "strong",
                            ] {
                                if ui.selectable_value(draft, easing.into(), easing).changed() {
                                    apply = true;
                                }
                            }
                        });
                    ui.text_edit_singleline(draft);
                    apply |= ui.button("Aplicar curva").clicked();
                });
                if apply {
                    let easing = draft.clone();
                    self.commit(snapshot.document.set_keyframe_easing(&id, &easing));
                }
            }
        }
        if ui.button("Adicionar/remover keyframe").clicked() {
            self.toggle_selection_keyframe();
        }
        if ui.button("Remover animação da propriedade").clicked() {
            self.remove_selection_keyframe_tracks();
        }
        if ui.button("Remover propriedade").clicked() {
            self.commit(
                self.selection
                    .iter()
                    .try_fold(snapshot.document.clone(), |document, id| {
                        document.set(id, &self.property, None)
                    }),
            );
        }
        ui.horizontal(|ui| {
            if ui.button("Copiar").clicked() {
                self.copy_selection();
            }
            if ui.button("Recortar").clicked() && self.copy_selection() {
                self.delete_selection();
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("Colar"))
                .clicked()
            {
                self.paste_selection();
            }
        });
        if ui.button("Criar cena com a seleção").clicked() {
            self.wrap_scene_selection();
        }
        if ui.button("Retirar sequências da seleção").clicked() {
            self.dissolve_selection(true);
        }
        if ui.button("Desagrupar seleção").clicked() {
            self.ungroup_selection();
        }
        if ui.button("Excluir seleção").clicked() {
            self.delete_selection();
        }
    }
}

impl eframe::App for Editor {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.export_cancel.store(true, Ordering::Relaxed);
        self.capture_cancel.store(true, Ordering::Relaxed);
        if let Some(job) = self.export_job.take() {
            let _ = job.join();
        }
        if let Some(job) = self.capture_job.take() {
            let _ = job.join();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
        let files: Vec<_> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        self.insert_dropped_files(&files);
        #[cfg(debug_assertions)]
        if let Some(smoke) = &mut self.smoke {
            // Native-window test hook: capture the actual eframe framebuffer
            // after the asynchronous composition arrives, then close the app.
            use image::ImageEncoder;
            let screenshot = ui.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = screenshot {
                let result = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&smoke.output)
                    .map_err(|e| e.to_string())
                    .and_then(|file| {
                        image::codecs::png::PngEncoder::new(file)
                            .write_image(
                                image.as_raw(),
                                image.width() as u32,
                                image.height() as u32,
                                image::ExtendedColorType::Rgba8,
                            )
                            .map_err(|e| e.to_string())
                    });
                if let Err(error) = result {
                    eprintln!("Native UI smoke failed: {error}");
                    std::process::exit(2);
                }
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.texture.is_some()
                && !smoke.requested
                && smoke.started.elapsed() > Duration::from_secs(1)
            {
                smoke.requested = true;
                ui.ctx()
                    .send_viewport_cmd(
                        egui::ViewportCommand::Screenshot(egui::UserData::default()),
                    );
            } else if smoke.started.elapsed() > Duration::from_secs(30) {
                eprintln!("Native UI smoke timed out: {}", self.status);
                std::process::exit(2);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if !ui.ctx().egui_wants_keyboard_input() {
            self.edit_shortcuts(ui.ctx());
            self.playback_shortcuts(ui.ctx());
        }
        egui::Panel::top("toolbar").show(ui, |ui| self.controls(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(300.0)
            .min_size(220.0)
            .show(ui, |ui| self.timeline(ui));
        egui::Panel::right("inspector")
            .default_size(250.0)
            .min_size(230.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector-content-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.inspector(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let drop_rect = ui.max_rect();
            ui.vertical_centered(|ui| {
                if let Some(texture) = &self.texture {
                    let available = ui.available_size();
                    let size = texture.size_vec2();
                    let scale = if self.preview_actual_size {
                        self.frame_geometry
                            .as_ref()
                            .and_then(|(_, geometry)| {
                                let scene = &self.snapshot.as_ref()?.timeline.active;
                                geometry
                                    .get(scene)
                                    .map(|item| item.frame.size[1] as f32 / size.y)
                            })
                            .unwrap_or(1.0)
                    } else {
                        (available.x / size.x).min(available.y / size.y).min(2.0)
                    };
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.image((texture.id(), size * scale));
                    });
                } else {
                    ui.label(if self.busy {
                        "Preparando prévia…"
                    } else if self.snapshot.is_some() {
                        "Escolha ou crie uma cena no menu acima."
                    } else {
                        "Abra um projeto para visualizar a cena."
                    });
                }
            });
            let file_count = ui.input(|input| input.raw.hovered_files.len());
            if file_count > 0 {
                let message = if self.exporting || self.capturing {
                    "Aguarde o trabalho de mídia atual terminar.".to_string()
                } else if self.transport.is_none() {
                    if self.snapshot.as_ref().is_some_and(|snapshot| {
                        !snapshot
                            .document
                            .root
                            .children
                            .iter()
                            .any(|node| node.tag == "scene")
                    }) {
                        "Solte os arquivos para criar a primeira cena.".to_string()
                    } else {
                        "Crie ou escolha uma cena para inserir mídia.".to_string()
                    }
                } else if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot.document.module_sources.is_empty())
                {
                    "A inserção neste projeto ainda requer edição do TSX.".to_string()
                } else {
                    format!("Solte {file_count} arquivo(s) na cena atual")
                };
                ui.painter().rect_filled(
                    drop_rect,
                    8.0,
                    Color32::from_rgba_unmultiplied(15, 25, 40, 220),
                );
                ui.painter().rect_stroke(
                    drop_rect.shrink(4.0),
                    8.0,
                    Stroke::new(2.0, Color32::from_rgb(80, 160, 255)),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    drop_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    FontId::proportional(20.0),
                    Color32::WHITE,
                );
            }
        });
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn editor() -> (egui::Context, Editor) {
        let ctx = egui::Context::default();
        let mut editor = Editor::create(&ctx, String::new());
        let document=SourceDocument::parse(PathBuf::from("index.tsx"),r#"export default <stage><scene id="s" active workarea={[0,8]}><rect id="a" end={2}/><rect id="b" start={1} end={4}/></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        for _ in 0..3 {
            frame(&ctx, &mut editor, Vec::new(), Default::default());
        }
        (ctx, editor)
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a Windows audio output device"]
    fn desktop_audio_clock_pause_seek_workarea_end_and_loop() {
        let (_, mut editor) = editor();
        let source = editor.snapshot.as_ref().unwrap().document.source.clone();
        editor.transport.as_mut().unwrap().toggle();
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.transport.as_ref().unwrap().time == 0.0 {
            editor.tick_audio();
            assert!(
                editor.transport.as_ref().unwrap().playing,
                "{}",
                editor.status
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let (_, _, start, session) = editor.audio_playback.as_ref().unwrap();
        assert_eq!(
            editor.transport.as_ref().unwrap().time,
            *start + session.played_frames() as f64 / 48000.0
        );
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        editor.tick_audio();
        let (generation, _, start, _) = editor.audio_playback.as_ref().unwrap();
        assert_eq!(*generation, editor.transport.as_ref().unwrap().generation);
        assert_eq!(*start, 1.0);
        editor.transport.as_mut().unwrap().toggle();
        let paused = editor.transport.as_ref().unwrap().time;
        editor.tick_audio();
        assert!(editor.audio_playback.is_none());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !editor.audio_retired.is_empty() {
            editor.tick_audio();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(editor.transport.as_ref().unwrap().time, paused);
        editor
            .transport
            .as_mut()
            .unwrap()
            .seek(8.0 - 2.0 / 30.0)
            .unwrap();
        editor.transport.as_mut().unwrap().toggle();
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.transport.as_ref().unwrap().playing {
            editor.tick_audio();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(editor.transport.as_ref().unwrap().time, 8.0);
        assert!(editor.audio_playback.is_none());
        let transport = editor.transport.as_mut().unwrap();
        transport.looping = true;
        transport.seek(8.0 - 1.0 / 30.0).unwrap();
        transport.toggle();
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.transport.as_ref().unwrap().time > 7.0 {
            editor.tick_audio();
            assert!(
                editor.transport.as_ref().unwrap().playing,
                "{}",
                editor.status
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(editor.transport.as_ref().unwrap().time, 0.0);
        editor.transport.as_mut().unwrap().toggle();
        editor.tick_audio();
        editor.transport.as_mut().unwrap().shuttle(false);
        editor.tick_audio();
        assert_eq!(editor.transport.as_ref().unwrap().speed, 2.0);
        assert_eq!(editor.audio_playback.as_ref().unwrap().3.speed(), 2.0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.transport.as_ref().unwrap().time == 0.0 {
            editor.tick_audio();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let (_, _, start, session) = editor.audio_playback.as_ref().unwrap();
        assert_eq!(
            editor.transport.as_ref().unwrap().time,
            *start + session.played_frames() as f64 / 48000.0 * 2.0
        );
        editor.transport.as_mut().unwrap().toggle();
        editor.tick_audio();
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, source);
    }

    fn frame(
        ctx: &egui::Context,
        editor: &mut Editor,
        mut events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0))),
                events,
                ..Default::default()
            },
            |ui| editor.timeline(ui),
        );
        output.textures_delta.clear();
    }

    fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]
    }

    #[test]
    fn mouse_marquee_selects_real_widgets_in_both_directions_and_keeps_additive_base() {
        let (ctx, mut editor) = editor();
        let a = editor
            .hit_regions
            .iter()
            .find(|(id, _)| id == "a")
            .unwrap()
            .1;
        let b = editor
            .hit_regions
            .iter()
            .find(|(id, _)| id == "b")
            .unwrap()
            .1;
        let empty = Pos2::new(b.right() + 30.0, a.bottom() + 1.0);
        let target = Pos2::new(a.left() + 2.0, b.top() + 2.0);
        frame(
            &ctx,
            &mut editor,
            button(empty, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(target)],
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert!(editor.selection.contains(&"a".into()));
        assert!(editor.selection.contains(&"b".into()));
        assert!(editor.marquee.is_none());
        assert!(editor.drag_start.is_none());
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .timeline
                .get("a")
                .unwrap()
                .start,
            0.0
        );
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        let extend = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let empty = Pos2::new(b.right() + 30.0, b.top() - 2.0);
        let target = Pos2::new(b.left() + 2.0, b.bottom() - 2.0);
        frame(&ctx, &mut editor, button(empty, true, extend), extend);
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(target)],
            extend,
        );
        frame(&ctx, &mut editor, button(target, false, extend), extend);
        assert_eq!(editor.selection, vec!["a", "b"]);
    }

    #[test]
    fn mouse_clicks_toggle_selection_and_empty_click_clears_it() {
        let (ctx, mut editor) = editor();
        let a = editor
            .hit_regions
            .iter()
            .find(|(id, _)| id == "a")
            .unwrap()
            .1;
        let b = editor
            .hit_regions
            .iter()
            .find(|(id, _)| id == "b")
            .unwrap()
            .1;
        for (pos, modifiers) in [
            (a.center(), egui::Modifiers::default()),
            (
                b.center(),
                egui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
        ] {
            frame(&ctx, &mut editor, button(pos, true, modifiers), modifiers);
            frame(&ctx, &mut editor, button(pos, false, modifiers), modifiers);
        }
        assert_eq!(editor.selection, vec!["a", "b"]);
        let empty = Pos2::new(b.right() + 30.0, b.center().y);
        frame(
            &ctx,
            &mut editor,
            button(empty, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(empty, false, Default::default()),
            Default::default(),
        );
        assert!(editor.selection.is_empty());
        assert!(editor.selected.is_empty());
    }

    #[test]
    fn sequence_gap_click_and_marquee_never_start_a_clip_move() {
        let (ctx, mut editor) = editor();
        let document=SourceDocument::parse(PathBuf::from("index.tsx"),r#"export default <stage><scene id="s" active workarea={[0,8]}><sequence id="track"><rect id="first" end={2}/><rect id="second" start={4} end={6}/></sequence></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        let original = document.source.clone();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        for _ in 0..3 {
            frame(&ctx, &mut editor, Vec::new(), Default::default());
        }
        let track = editor
            .hit_regions
            .iter()
            .filter(|(id, _)| id == "track")
            .map(|(_, rect)| *rect)
            .collect::<Vec<_>>();
        let gap = Pos2::new(
            (track[0].right() + track[1].left()) / 2.0,
            track[0].center().y,
        );
        frame(
            &ctx,
            &mut editor,
            button(gap, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(gap, false, Default::default()),
            Default::default(),
        );
        assert_eq!(editor.gap_selection, Some(("track".into(), [2.0, 4.0])));
        let first = editor
            .hit_regions
            .iter()
            .find(|(id, _)| id == "first")
            .unwrap()
            .1;
        let target = Pos2::new(first.left() + 2.0, first.top() + 2.0);
        frame(
            &ctx,
            &mut editor,
            button(gap, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(target)],
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert!(editor.selection.contains(&"first".into()));
        assert!(editor.gap_selection.is_none());
        assert!(editor.drag_start.is_none());
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, original);
    }

    fn persist(editor: &mut Editor) -> (PathBuf, String) {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "ad3-native-interaction-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&folder).unwrap();
        let entry = folder.join("index.tsx");
        let original = editor.snapshot.as_ref().unwrap().document.source.clone();
        std::fs::write(&entry, &original).unwrap();
        let document = SourceDocument::open(&folder).unwrap();
        editor.history = Some(History::new(document.clone()));
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        (folder, original)
    }

    fn cleanup(folder: PathBuf) {
        let folder = std::fs::canonicalize(folder).unwrap();
        assert!(folder.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(
            folder
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ad3-native-interaction-")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    fn key(ctx: &egui::Context, editor: &mut Editor, key: egui::Key, modifiers: egui::Modifiers) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0))),
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |ui| {
                editor.edit_shortcuts(ui.ctx());
                editor.playback_shortcuts(ui.ctx());
                editor.timeline(ui);
            },
        );
        output.textures_delta.clear();
    }

    #[test]
    #[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
    fn desktop_saved_mov_and_fractional_fps_reach_the_finished_export() {
        let (ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,0.5]}><rect id="r" width={32} height={16} fill="red"/></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        let (folder, original) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r#"{"name":"export-test","diffusion":{"export":{"s":{"video":{"resolution":16}}}}}"#,
        )
        .unwrap();
        editor.export_fps = 23.976;
        editor.export_resolution = 16;
        editor.set_export_format("mov");
        editor.save_export_preferences();
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        editor.snapshot = None;
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        assert_eq!(editor.export_fps, 23.976);
        assert!(editor.export_path.ends_with("export.mov"));
        let output = PathBuf::from(&editor.export_path);
        assert_eq!(editor.export_resolution, 16);
        editor.export_resolution = 24;
        editor.export_video_bitrate = 2_000_000;
        editor.export(ctx.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while editor.exporting && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.exporting, "{}", editor.status);
        assert!(output.is_file(), "{}", editor.status);
        let tools = Tools::default();
        let probe = ad3_editor::media::checked_output(
            ad3_editor::media::command(&tools.ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_format",
                    "-show_streams",
                    "-of",
                    "json",
                ])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(
            value["format"]["tags"]["major_brand"]
                .as_str()
                .unwrap()
                .trim(),
            "qt"
        );
        assert_eq!(value["streams"][0]["r_frame_rate"], "2997/125");
        assert_eq!(value["streams"][0]["nb_frames"], "12");
        assert_eq!(value["streams"][0]["width"], 48);
        assert_eq!(value["streams"][0]["height"], 24);
        let encoded = ad3_editor::media::checked_output(
            ad3_editor::media::command(&tools.ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&output)
                .args([
                    "-map",
                    "0:v:0",
                    "-c:v",
                    "copy",
                    "-bsf:v",
                    "h264_mp4toannexb",
                    "-f",
                    "h264",
                    "pipe:1",
                ]),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&encoded.stdout).contains("bitrate=2000"));
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .video_bitrate("s")
                .unwrap(),
            Some(10_000_000)
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .video_resolution("s")
                .unwrap(),
            Some(16)
        );
        editor.export_video = false;
        editor.export_audio = true;
        editor.export_audio_codec = "mp3".into();
        editor.export_resolution = 0;
        editor.export_audio_channels = 1;
        editor.export_audio_sample_rate = 44_100;
        let audio_only = folder.join("audio-only.mov");
        editor.export_audio_bitrate = 64_000;
        editor.export_path = audio_only.to_string_lossy().into_owned();
        editor.export(ctx.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while editor.exporting && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.exporting, "{}", editor.status);
        let probe = ad3_editor::media::checked_output(
            ad3_editor::media::command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&audio_only),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"].as_array().unwrap().len(), 1);
        assert_eq!(value["streams"][0]["codec_type"], "audio");
        assert_eq!(value["streams"][0]["codec_name"], "mp3");
        assert_eq!(value["streams"][0]["channels"], 1);
        assert_eq!(value["streams"][0]["sample_rate"], "44100");
        // MP3 frame padding makes the container's measured average differ
        // slightly from the requested encoder target.
        let measured_bitrate = value["streams"][0]["bit_rate"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(
            measured_bitrate.abs_diff(64_000) < 640,
            "{measured_bitrate}"
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .audio_bitrate("s")
                .unwrap(),
            Some(128_000)
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .audio_channels("s")
                .unwrap(),
            2
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .audio_codec("s", "mov")
                .unwrap(),
            "aac"
        );
        assert!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .video_enabled("s")
                .unwrap()
        );
        editor.export_audio = false;
        editor.export(ctx.clone());
        assert!(!editor.exporting);
        assert!(editor.status.contains("Ative vídeo ou áudio"));
        editor.export_video = true;
        editor.export_resolution = 24;
        editor.set_export_format("webm");
        editor.export_video_codec = "vp8".into();
        let vp8 = PathBuf::from(&editor.export_path);
        editor.export(ctx.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while editor.exporting && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.exporting, "{}", editor.status);
        let probe = ad3_editor::media::checked_output(
            ad3_editor::media::command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&vp8),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"][0]["codec_name"], "vp8");
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .video_codec("s", "mov")
                .unwrap(),
            "avc"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn hidden_shortcut_hides_mixed_selection_and_reveals_when_all_hidden() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.set_selection_flag("hidden", true);
        editor.selection = vec!["a".into(), "b".into()];
        editor.selected = "b".into();
        let modifiers = egui::Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Default::default()
        };
        key(&ctx, &mut editor, egui::Key::H, modifiers);
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .hidden
            );
        }
        key(&ctx, &mut editor, egui::Key::H, modifiers);
        for id in ["a", "b"] {
            assert!(
                !editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .hidden
            );
        }
        editor.history(false);
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn bracket_shortcuts_restack_selection_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        key(
            &ctx,
            &mut editor,
            egui::Key::CloseBracket,
            Default::default(),
        );
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("s")
                .unwrap()
                .children
                .last()
                .unwrap()
                .id,
            "a"
        );
        assert_eq!(editor.selection, vec!["a"]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        key(
            &ctx,
            &mut editor,
            egui::Key::CloseBracket,
            egui::Modifiers::COMMAND,
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn coloring_unstamped_clips_persists_ids_before_metadata_and_survives_reopen() {
        let (_ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene active workarea={[0,8]}><rect end={2}/><rect start={1} end={4}/></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("package.json"), r#"{"name":"test"}"#).unwrap();
        let ids = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .nodes()
            .into_iter()
            .filter(|node| node.tag == "rect")
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        editor.selection = ids.clone();
        editor.selected = ids[0].clone();
        editor.set_selection_color(Some("#3B82F6"));
        let reopened = SourceDocument::open(&folder).unwrap();
        let config = ad3_editor::project_config::ProjectConfig::load(&folder).unwrap();
        for id in ids {
            assert!(reopened.root.find(&id).is_some());
            assert_eq!(
                config.clip_color(&reopened.source_key(&id).unwrap()),
                Some("#3B82F6")
            );
        }
        cleanup(folder);
    }

    #[test]
    fn selection_colors_save_together_and_restore_without_editing_tsx() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r##"{"diffusion":{"clipColors":{"other.tsx:a":"#10B981"}}}"##,
        )
        .unwrap();
        editor.selection = vec!["a".into(), "b".into()];
        editor.selected = "b".into();
        editor.set_selection_color(Some("#EC4899"));
        let config = ad3_editor::project_config::ProjectConfig::load(&folder).unwrap();
        assert_eq!(config.clip_color("index.tsx:a"), Some("#EC4899"));
        assert_eq!(config.clip_color("index.tsx:b"), Some("#EC4899"));
        editor.set_selection_color(None);
        let config = ad3_editor::project_config::ProjectConfig::load(&folder).unwrap();
        assert_eq!(config.clip_color("index.tsx:a"), None);
        assert_eq!(config.clip_color("index.tsx:b"), None);
        assert_eq!(config.clip_color("other.tsx:a"), Some("#10B981"));
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn timeline_colors_resolve_original_project_relative_source_keys() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r##"{"diffusion":{"clipColors":{"index.tsx:a":"#3B82F6","other.tsx:a":"#10B981"}}}"##,
        )
        .unwrap();
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        let key = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .source_key("a")
            .unwrap();
        assert_eq!(key, "index.tsx:a");
        assert_eq!(
            editor.marker_config.as_ref().unwrap().clip_color(&key),
            Some("#3B82F6")
        );
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 600.0))),
                ..Default::default()
            },
            |ui| editor.timeline(ui),
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == Color32::from_rgb(59, 130, 246))));
        output.textures_delta.clear();
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn visibility_and_mute_selection_save_as_single_undoable_edits() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        editor.selected = "b".into();
        editor.set_selection_flag("hidden", true);
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .hidden
            );
        }
        assert_eq!(editor.selection, vec!["a", "b"]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        editor.set_selection_flag("muted", true);
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .muted
            );
        }
        editor.set_selection_flag("muted", false);
        for id in ["a", "b"] {
            assert!(
                !editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .muted
            );
        }
        editor.history(false);
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .timeline
                    .get(id)
                    .unwrap()
                    .muted
            );
        }
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn explicit_project_reload_restores_saved_export_preferences() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("package.json"), r#"{"diffusion":{"export":{"s":{"format":"webm","video":{"fps":24},"audio":{"enabled":false}}}}}"#).unwrap();
        editor.export_fps = 60.0;
        editor.export_audio = true;
        editor.export_path = folder.join("custom.mp4").to_string_lossy().into_owned();
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        let opened = Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap();
        editor
            .event_sender
            .send(Event::Opened(Box::new(Ok(opened))))
            .unwrap();
        editor.poll(&ctx);
        assert_eq!(editor.export_fps, 24.0);
        assert!(!editor.export_audio);
        assert!(editor.export_path.ends_with("export.webm"));
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn format_selection_keeps_directory_and_filename_and_preserves_active_job() {
        let (_ctx, mut editor) = editor();
        editor.export_path = PathBuf::from("D:/tmp/My project/cut.final.MP4")
            .to_string_lossy()
            .into_owned();
        editor.set_export_format("mov");
        assert_eq!(
            Path::new(&editor.export_path),
            Path::new("D:/tmp/My project/cut.final.mov")
        );
        editor.exporting = true;
        editor.set_export_format("mp4");
        assert!(editor.export_path.ends_with("cut.final.mov"));
        editor.exporting = false;
        editor.set_export_format("ogg");
        assert!(editor.export_path.ends_with("cut.final.ogg"));
        assert!(!editor.export_video && editor.export_audio);
        editor.set_export_format("mp4");
        assert!(editor.export_path.ends_with("cut.final.mp4"));
        editor.export_path.clear();
        editor.set_export_format("mov");
        assert!(editor.export_path.ends_with("export.mov"));
    }

    #[test]
    fn audio_format_changes_preserve_supported_rates_and_replace_incompatible_rates() {
        let (_ctx, mut editor) = editor();
        editor.export_path = "export.mp4".into();
        editor.export_audio_sample_rate = 44_100;
        editor.set_export_format("mov");
        assert_eq!(editor.export_audio_sample_rate, 44_100);
        editor.set_export_format("webm");
        assert_eq!(editor.export_audio_sample_rate, 48_000);
        editor.export_audio_sample_rate = 24_000;
        editor.set_export_format("ogg");
        assert_eq!(editor.export_audio_sample_rate, 24_000);
        editor.export_audio_codec = "flac".into();
        editor.export_audio_sample_rate = 192_000;
        editor.normalize_export_audio_rate("ogg");
        assert_eq!(editor.export_audio_sample_rate, 192_000);
        editor.export_audio_codec = "opus".into();
        editor.normalize_export_audio_rate("ogg");
        assert_eq!(editor.export_audio_sample_rate, 48_000);
        editor.exporting = true;
        editor.export_audio_sample_rate = 44_100;
        editor.set_export_format("webm");
        assert_eq!(editor.export_audio_sample_rate, 44_100);
    }

    #[test]
    fn export_presets_replace_draft_settings_and_save_without_changing_source() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r#"{"name":"preset-fixture","custom":true}"#,
        )
        .unwrap();
        let source = editor.snapshot.as_ref().unwrap().document.source.clone();
        editor.export_path = folder.join("custom.ogg").to_string_lossy().into_owned();
        editor.export_video = false;
        editor.export_audio = false;
        editor.export_fps = 60.0;
        editor.export_audio_channels = 1;
        editor.export_audio_sample_rate = 192_000;
        editor.apply_export_preset("web-embedding-720p");
        assert!(editor.export_path.ends_with("custom.webm"));
        assert!(editor.export_video && editor.export_audio);
        assert_eq!(editor.export_resolution, 720);
        assert_eq!(editor.export_video_bitrate, 3_000_000);
        assert_eq!(editor.export_video_codec, "vp9");
        assert_eq!(editor.export_audio_codec, "opus");
        assert_eq!(editor.export_audio_sample_rate, 48_000);
        assert_eq!(editor.export_audio_channels, 2);
        assert_eq!(editor.export_fps, 30.0);
        assert!(
            !std::fs::read_to_string(folder.join("package.json"))
                .unwrap()
                .contains("diffusion")
        );
        editor.save_export_preferences();
        let config = ad3_editor::project_config::ProjectConfig::load(&folder).unwrap();
        assert_eq!(config.video_bitrate("s").unwrap(), Some(3_000_000));
        assert_eq!(config.video_codec("s", "webm").unwrap(), "vp9");
        assert_eq!(config.video_resolution("s").unwrap(), Some(720));
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, source);
        editor.exporting = true;
        editor.apply_export_preset("h264-mp4-2160p");
        assert_eq!(editor.export_resolution, 720);
        editor.exporting = false;
        editor.apply_export_preset("h264-mp4-2160p");
        assert_eq!(editor.export_resolution, 2160);
        assert_eq!(editor.export_video_bitrate, 64_000_000);
        assert_eq!(editor.export_video_codec, "avc");
        editor.apply_export_preset("unknown");
        assert_eq!(editor.export_resolution, 2160);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn export_estimate_uses_active_workarea_and_only_enabled_tracks() {
        let (_ctx, mut editor) = editor();
        let snapshot = editor.snapshot.take();
        assert!(editor.export_size_estimate().is_none());
        editor.snapshot = snapshot;
        let (folder, _) = persist(&mut editor);
        let duration = editor.export_size_estimate().unwrap().0;
        editor.export_video_bitrate = 8_000_000;
        editor.export_audio_bitrate = 128_000;
        assert_eq!(
            editor.export_size_estimate(),
            Some((duration, Some(duration * 1_016_000.0)))
        );
        editor.export_video = false;
        assert_eq!(
            editor.export_size_estimate(),
            Some((duration, Some(duration * 16_000.0)))
        );
        editor.export_audio_codec = "flac".into();
        assert_eq!(editor.export_size_estimate(), Some((duration, None)));
        editor.export_audio = false;
        assert_eq!(editor.export_size_estimate(), Some((duration, Some(0.0))));
        editor.export_video = true;
        assert_eq!(
            editor.export_size_estimate(),
            Some((duration, Some(duration * 1_000_000.0)))
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn export_progress_tracks_frames_and_waits_for_completion_after_finalizing() {
        let (ctx, mut editor) = editor();
        editor.exporting = true;
        editor.export_started = Some(Instant::now());
        editor
            .event_sender
            .send(Event::ExportProgress(4, 10))
            .unwrap();
        editor.poll(&ctx);
        assert_eq!(editor.export_progress, (4, 10));
        assert!(editor.exporting);
        editor
            .event_sender
            .send(Event::ExportProgress(12, 10))
            .unwrap();
        editor.poll(&ctx);
        assert_eq!(editor.export_progress, (10, 10));
        assert!(editor.status.contains("Finalizando"));
        assert!(editor.exporting);
        editor
            .event_sender
            .send(Event::Exported(Err("Export cancelled".into())))
            .unwrap();
        editor.poll(&ctx);
        assert!(!editor.exporting);
        assert!(editor.export_started.is_none());
        editor
            .event_sender
            .send(Event::ExportProgress(1, 10))
            .unwrap();
        editor.poll(&ctx);
        assert_eq!(editor.status, "Export cancelled");
        assert_eq!(editor.export_progress, (10, 10));
    }

    #[test]
    fn panicking_media_workers_report_failure_and_release_ui_for_another_job() {
        let (ctx, mut editor) = editor();
        editor.exporting = true;
        editor.export_started = Some(Instant::now());
        let sender = editor.event_sender.clone();
        editor.export_job = Some(thread::spawn(move || {
            let result = media_job::<PathBuf>(|| panic!("simulated export worker failure"));
            sender.send(Event::Exported(result)).unwrap();
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while editor.exporting && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!editor.exporting);
        assert!(editor.export_job.is_none());
        assert!(editor.export_started.is_none());
        assert!(editor.status.contains("Falha interna"));
        editor.capturing = true;
        let sender = editor.event_sender.clone();
        editor.capture_job = Some(thread::spawn(move || {
            let result = media_job::<PathBuf>(|| panic!("simulated capture worker failure"));
            sender.send(Event::Captured(result)).unwrap();
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while editor.capturing && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!editor.capturing);
        assert!(editor.capture_job.is_none());
        assert!(editor.status.contains("Falha interna"));
        assert_eq!(media_job(|| Ok(42)), Ok(42));
        assert_eq!(
            media_job::<()>(|| Err("cancelled".into())),
            Err("cancelled".into())
        );
    }

    #[test]
    fn preview_worker_reports_bad_request_and_renders_next_request_on_same_channel() {
        let ctx = egui::Context::default();
        let editor = Editor::create(&ctx, String::new());
        let document = SourceDocument::parse(PathBuf::from("preview-recovery.tsx"), r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="r" width={32} height={16} fill="red"/></scene></stage>;"#.into()).unwrap();
        let snapshot = Arc::new(
            Snapshot::from_document(document, PathBuf::from("."), &Tools::default()).unwrap(),
        );
        for (generation, skews) in [
            (1, BTreeMap::from([("r".into(), [f64::NAN, 0.0])])),
            (2, BTreeMap::new()),
        ] {
            editor
                .request
                .send(FrameRequest {
                    snapshot: snapshot.clone(),
                    generation,
                    time: 0.0,
                    playing: false,
                    full_resolution: true,
                    runtime_skews: skews,
                    controls: Vec::new(),
                })
                .unwrap();
            let reply = editor.frames.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(reply.generation, generation);
            if generation == 1 {
                assert!(reply.result.is_err());
            } else {
                let (image, geometry) = reply.result.unwrap();
                assert_eq!(image.size, [32, 16]);
                assert!(geometry.contains_key("r"));
                assert!(image.pixels.iter().any(|pixel| pixel.r() > 240));
            }
        }
    }

    #[test]
    fn selecting_scene_switches_timeline_preferences_and_selection_and_is_undoable() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,8]}><rect id="a" end={2}/></scene><scene id="other" workarea={[2,5]}><rect id="b" end={5}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        std::fs::write(folder.join("package.json"), r#"{"diffusion":{"export":{"s":{"format":"mov","video":{"fps":24}},"other":{"format":"webm","video":{"fps":60,"resolution":720}}}}}"#).unwrap();
        let document = SourceDocument::open(&folder).unwrap();
        editor.history = Some(History::new(document.clone()));
        editor.snapshot = None;
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        editor.activate_scene("other");
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "other");
        assert!(editor.selection.is_empty());
        assert_eq!(editor.transport.as_ref().unwrap().workarea, [2.0, 5.0]);
        assert_eq!(editor.export_fps, 60.0);
        assert_eq!(editor.export_resolution, 720);
        assert!(editor.export_path.ends_with("export.webm"));
        assert_eq!(
            SourceDocument::open(&folder)
                .unwrap()
                .root
                .children
                .iter()
                .filter(|node| node.flag("active"))
                .count(),
            1
        );
        editor.history(false);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "s");
        assert_eq!(editor.export_fps, 24.0);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "other");
        editor.exporting = true;
        editor.activate_scene("s");
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "other");
        editor.exporting = false;
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn new_scene_is_unique_active_renderable_and_one_undoable_transaction() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.create_scene(64, 32);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.timeline.active, "scene-1");
        assert_eq!(snapshot.timeline.workareas["scene-1"], [0.0, 5.0]);
        assert_eq!(editor.selection, vec!["scene-1"]);
        assert!(snapshot.document.root.find("a").is_some());
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let plan = renderer.frame_plan("scene-1", 0.0, 30.0, false).unwrap();
        assert_eq!((plan.width, plan.height), (64, 32));
        let pixels = plan.frame(&tools, &folder).unwrap();
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
        editor.create_scene(32, 64);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "scene-2");
        editor.history(false);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "scene-1");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "scene-1");
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.create_scene(0, 1080);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        let empty =
            SourceDocument::parse(folder.join("empty.tsx"), "export default <stage/>;".into())
                .unwrap();
        let (created, id) = empty.create_scene(32, 16, 2.0).unwrap();
        assert_eq!(
            Timeline::resolve(&created.root, &Default::default())
                .unwrap()
                .active,
            id
        );
        assert!(empty.create_scene(32, 16, f64::NAN).is_err());
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn empty_project_opens_creates_first_scene_and_undo_returns_to_empty_stage() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("index.tsx"), "export default <stage/>;").unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        assert!(snapshot.timeline.active.is_empty());
        assert!(snapshot.timeline.workareas.is_empty());
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        assert!(editor.transport.is_none());
        assert!(editor.texture.is_none());
        editor.export(ctx);
        assert!(!editor.exporting);
        assert!(editor.status.contains("crie uma cena"));
        editor.create_scene(64, 32);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "scene-1");
        assert!(editor.transport.is_some());
        editor.history(false);
        assert!(editor.snapshot.as_ref().unwrap().timeline.active.is_empty());
        assert!(editor.transport.is_none());
        assert!(editor.texture.is_none());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            "export default <stage/>;"
        );
        editor.history(true);
        assert!(editor.transport.is_some());
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn deleting_active_scene_preserves_other_scenes_and_undo_restores_exact_source() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.create_scene(64, 32);
        let two_scenes = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.remove_active_scene();
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("scene-1").is_none());
        assert!(snapshot.document.root.find("a").is_some());
        assert!(snapshot.timeline.active.is_empty());
        assert!(editor.transport.is_none());
        editor.activate_scene("s");
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "s");
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            two_scenes
        );
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "scene-1");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        editor.remove_active_scene();
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .children
                .is_empty()
        );
        assert!(editor.transport.is_none());
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn scene_copy_remaps_descendants_preserves_runtime_skew_and_undoes_together() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.runtime_skews.insert("a".into(), [15.0, -5.0]);
        editor.duplicate_active_scene();
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.timeline.active, "s-2");
        assert_eq!(editor.selection, vec!["s-2"]);
        assert!(snapshot.document.root.find("a-2").is_some());
        assert!(snapshot.document.root.find("b-2").is_some());
        assert_eq!(editor.runtime_skews["a-2"], [15.0, -5.0]);
        assert_eq!(
            snapshot
                .document
                .root
                .children
                .iter()
                .filter(|node| node.flag("active"))
                .count(),
            1
        );
        assert_eq!(
            snapshot.timeline.workareas["s-2"],
            snapshot.timeline.workareas["s"]
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        assert!(!editor.runtime_skews.contains_key("a-2"));
        assert_eq!(editor.runtime_skews["a"], [15.0, -5.0]);
        editor.history(true);
        assert_eq!(editor.snapshot.as_ref().unwrap().timeline.active, "s-2");
        assert_eq!(editor.runtime_skews["a-2"], [15.0, -5.0]);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn workarea_edits_trim_playback_without_moving_clips_and_undo_exactly() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        let spans = editor.snapshot.as_ref().unwrap().timeline.spans.clone();
        editor.transport.as_mut().unwrap().seek(7.0).unwrap();
        editor.set_workarea([1.0, 4.0]);
        assert_eq!(editor.workarea_draft, [1.0, 4.0]);
        assert_eq!(editor.transport.as_ref().unwrap().workarea, [1.0, 4.0]);
        assert_eq!(editor.transport.as_ref().unwrap().time, 4.0);
        assert_eq!(editor.export_size_estimate().unwrap().0, 3.0);
        for id in ["a", "b"] {
            let old = &spans[id];
            let next = &editor.snapshot.as_ref().unwrap().timeline.spans[id];
            assert_eq!(
                (next.start, next.end, next.origin),
                (old.start, old.end, old.origin)
            );
        }
        editor.history(false);
        assert_eq!(editor.workarea_draft, [0.0, 8.0]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(editor.workarea_draft, [1.0, 4.0]);
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        for area in [[-1.0, 4.0], [2.0, 2.0], [0.0, f64::NAN], [0.0, 0.001]] {
            editor.set_workarea(area);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                saved
            );
        }
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn marking_workarea_endpoints_uses_playhead_and_preserves_one_undo_per_mark() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.mark_workarea_endpoint(true);
        assert_eq!(editor.workarea_draft, [2.0, 8.0]);
        editor.transport.as_mut().unwrap().seek(5.0).unwrap();
        editor.mark_workarea_endpoint(false);
        assert_eq!(editor.workarea_draft, [2.0, 5.0]);
        editor.history(false);
        assert_eq!(editor.workarea_draft, [2.0, 8.0]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.transport.as_mut().unwrap().seek(0.0).unwrap();
        editor.mark_workarea_endpoint(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn scene_rename_preserves_id_and_clips_and_undo_restores_exact_source() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.scene_name_draft = "Cena \"Abertura\" — ação".into();
        editor.rename_active_scene();
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.timeline.active, "s");
        assert_eq!(
            snapshot.document.root.find("s").unwrap().string("name"),
            Some("Cena \"Abertura\" — ação")
        );
        assert!(snapshot.document.root.find("a").is_some());
        assert_eq!(editor.transport.as_ref().unwrap().workarea, [0.0, 8.0]);
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.scene_name_draft = "   ".into();
        editor.rename_active_scene();
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(editor.scene_name_draft.is_empty());
        editor.history(true);
        assert_eq!(editor.scene_name_draft, "Cena \"Abertura\" — ação");
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn scene_format_changes_canvas_without_moving_clips_and_undoes_both_dimensions() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        let original_clips: Vec<_> = ["a", "b"]
            .iter()
            .map(|id| {
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .props
                    .clone()
            })
            .collect();
        editor.resize_active_scene(32, 64);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.timeline.active, "s");
        assert_eq!(snapshot.timeline.workareas["s"], [0.0, 8.0]);
        for (id, props) in ["a", "b"].iter().zip(original_clips) {
            assert_eq!(snapshot.document.root.find(id).unwrap().props, props);
        }
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        assert_eq!((plan.width, plan.height), (32, 64));
        assert_eq!(plan.frame(&tools, &folder).unwrap().len(), 32 * 64 * 4);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.resize_active_scene(0, 64);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn visual_insertion_creates_selected_timed_shapes_and_text_and_undoes_each() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        for (kind, id) in [
            ("rect", "rect-1"),
            ("circle", "circle-1"),
            ("text", "text-1"),
        ] {
            editor.insert_visual(kind);
            assert_eq!(editor.selected, id);
            let snapshot = editor.snapshot.as_ref().unwrap();
            assert_eq!(snapshot.timeline.spans[id].start, 1.0);
            assert_eq!(
                snapshot.document.root.find(id).unwrap().tag,
                if kind == "circle" { "rect" } else { kind }
            );
        }
        let snapshot = editor.snapshot.as_ref().unwrap();
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let mut plan = renderer.frame_plan("s", 1.0, 30.0, false).unwrap();
        plan.resize_output(32).unwrap();
        assert!(plan.frame(&tools, &folder).is_ok());
        for _ in 0..3 {
            editor.history(false);
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("rect-1")
                .is_some()
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn text_content_edits_escape_tsx_preserve_child_ranges_and_undo_exactly() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"// keep this comment
export default <stage><scene id="s" active width={320} height={180} workarea={[0,2]}><text id="t" fontSize={32} color="white">Old<textRange id="range" start={0} end={2} color="red"/></text></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let document = SourceDocument::open(&folder).unwrap();
        editor.history = Some(History::new(document.clone()));
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        let text = "Olá <AD3> & \"Rust\"\nsegunda linha {literal}";
        let next = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .set_text("t", text);
        assert!(editor.commit(next));
        let snapshot = editor.snapshot.as_ref().unwrap();
        let node = snapshot.document.root.find("t").unwrap();
        assert_eq!(node.text, text);
        assert_eq!(node.children[0].id, "range");
        assert!(snapshot.document.source.starts_with("// keep this comment"));
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        assert!(plan.frame(&tools, &folder).is_ok());
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("t")
                .unwrap()
                .text,
            text
        );
        let empty = SourceDocument::parse(folder.join("empty.tsx"), "export default <stage><scene id=\"s\" active><text id=\"t\" color=\"white\"/></scene></stage>;".into()).unwrap();
        assert_eq!(
            empty
                .set_text("t", "new")
                .unwrap()
                .root
                .find("t")
                .unwrap()
                .text,
            "new"
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn gradient_insertion_renders_linear_and_radial_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        for radial in [false, true] {
            editor.insert_selection_gradient(radial);
            let snapshot = editor.snapshot.as_ref().unwrap();
            let kind = if radial {
                "radialGradientPaint"
            } else {
                "linearGradientPaint"
            };
            assert!(
                snapshot
                    .document
                    .root
                    .find(&format!("{kind}-1-start"))
                    .is_some()
            );
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            let pixels = renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap();
            let left = &pixels[(8 * 32) * 4..(8 * 32) * 4 + 4];
            let center = &pixels[(8 * 32 + 16) * 4..(8 * 32 + 16) * 4 + 4];
            assert_ne!(left, center);
            assert_eq!(left[3], 255);
            assert_eq!(center[3], 255);
            drop(renderer);
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                source
            );
        }
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn gradient_stop_edit_renders_and_undoes_atomically() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} end={2}><linearGradientPaint id="g"><colorStop id="c" color="white" offset={0}/><colorStop id="d" color="black" offset={1}/></linearGradientPaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let draft = PaintDraft {
            rgb: [255, 0, 0],
            opacity: 0.75,
        };
        editor.edit_selected_gradient_stop("c", draft.clone(), f64::NAN);
        editor.edit_selected_gradient_stop("g", draft.clone(), 0.25);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.edit_selected_gradient_stop("c", draft, 0.25);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let stop = snapshot.document.root.find("c").unwrap();
        assert_eq!(stop.string("color"), Some("#ff0000"));
        assert_eq!(stop.number("offset", 0.0).unwrap(), 0.25);
        assert_eq!(stop.number("opacity", 1.0).unwrap(), 0.75);
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(pixels[(8 * 32) * 4] > 150);
        assert_eq!(pixels[(8 * 32) * 4 + 1], 0);
        drop(renderer);
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn gradient_opacity_and_removal_preserve_holder_and_restore_stops() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        for tag in ["linearGradientPaint", "radialGradientPaint"] {
            let source = format!(
                r#"export default <stage><scene id="s" active width={{32}} height={{16}} workarea={{[0,2]}}><rect id="a" width={{32}} height={{16}} fill="blue" end={{2}}><{tag} id="g"><colorStop id="c" color="red" offset={{0}}/><colorStop id="d" color="red" offset={{1}}/></{tag}></rect></scene></stage>;"#
            );
            std::fs::write(folder.join("index.tsx"), &source).unwrap();
            let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
            editor.history = Some(History::new(snapshot.document.clone()));
            editor.replace_snapshot(snapshot);
            editor.selected = "a".into();
            editor.selection = vec!["a".into()];
            editor.edit_selected_gradient("g", Some(f64::NAN));
            editor.edit_selected_gradient("a", None);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                source
            );
            editor.edit_selected_gradient("g", Some(0.5));
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            let pixels = renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap();
            assert!((120..=136).contains(&pixels[0]));
            assert!((120..=136).contains(&pixels[2]));
            drop(renderer);
            let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
            editor.edit_selected_gradient("g", None);
            let snapshot = editor.snapshot.as_ref().unwrap();
            assert!(snapshot.document.root.find("g").is_none());
            assert!(snapshot.document.root.find("c").is_none());
            assert_eq!(
                snapshot.document.root.find("a").unwrap().string("fill"),
                Some("blue")
            );
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                edited
            );
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                source
            );
        }
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn gradient_stops_insert_remove_preserve_other_stops_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        for radial in [false, true] {
            editor.insert_selection_gradient(radial);
            let gradient_id = if radial {
                "radialGradientPaint-1"
            } else {
                "linearGradientPaint-1"
            };
            let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
            editor.change_selected_gradient_stops(gradient_id, None);
            editor.change_selected_gradient_stops(gradient_id, None);
            let snapshot = editor.snapshot.as_ref().unwrap();
            let gradient = snapshot.document.root.find(gradient_id).unwrap();
            assert_eq!(gradient.children.len(), 4);
            assert_eq!(
                snapshot
                    .document
                    .root
                    .find("colorStop-1")
                    .unwrap()
                    .number("offset", 0.0)
                    .unwrap(),
                0.5
            );
            assert!(snapshot.document.root.find("colorStop-2").is_some());
            let inserted = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
            editor.change_selected_gradient_stops(gradient_id, Some("a"));
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                inserted
            );
            editor.change_selected_gradient_stops(gradient_id, Some("colorStop-1"));
            let snapshot = editor.snapshot.as_ref().unwrap();
            assert!(snapshot.document.root.find("colorStop-1").is_none());
            assert!(snapshot.document.root.find("colorStop-2").is_some());
            assert!(
                snapshot
                    .document
                    .root
                    .find(&format!("{gradient_id}-start"))
                    .is_some()
            );
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                inserted
            );
            editor.history(false);
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                before
            );
            editor.history(false);
        }
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(
            snapshot
                .document
                .insert_gradient_stop("a", 0.5, "red")
                .is_err()
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn gradient_geometry_rotates_render_and_undoes_all_properties() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" width={32} height={32} end={2}><linearGradientPaint id="g"><colorStop id="c" color="red" offset={0}/><colorStop id="d" color="blue" offset={1}/></linearGradientPaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        let before = render(&editor);
        editor.edit_selected_gradient_geometry("g", [0.5, 0.5, 1.0, 1.0, 90.0]);
        let after = render(&editor);
        let left = (16 * 32) * 4;
        assert!(before[left] > 240);
        assert!((115..=140).contains(&after[left]));
        assert!(after[16 * 4] > 240);
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_gradient_geometry("g", [f64::NAN, 0.5, 1.0, 1.0, 0.0]);
        editor.edit_selected_gradient_geometry("a", [0.0; 5]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert_eq!(render(&editor), before);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn paint_stack_reordering_changes_pixels_preserves_other_children_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={16} height={16} workarea={[0,2]}><rect id="a" width={16} height={16} end={2}><solidPaint id="p" color="red"/><animation id="anim" type="fade" duration={0.5}/><linearGradientPaint id="g"><colorStop id="c" color="blue" offset={0}/><colorStop id="d" color="blue" offset={1}/></linearGradientPaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 1.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        assert_eq!(&render(&editor)[..4], &[0, 0, 255, 255]);
        editor.move_selected_paint("p", true);
        assert_eq!(&render(&editor)[..4], &[255, 0, 0, 255]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let children = &snapshot.document.root.find("a").unwrap().children;
        assert_eq!(
            children
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            ["g", "anim", "p"]
        );
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.move_selected_paint("p", true);
        editor.move_selected_paint("anim", false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn paint_bypass_preserves_stops_and_tracks_renders_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={16} height={16} workarea={[0,2]}><rect id="a" width={16} height={16} fill="red" end={2}><linearGradientPaint id="g"><colorStop id="c" color="blue" offset={0}><keyframeTrack id="t" property="offset"><keyframe id="k" time={0} value={0}/></keyframeTrack></colorStop><colorStop id="d" color="blue" offset={1}/></linearGradientPaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 1.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        assert_eq!(&render(&editor)[..4], &[0, 0, 255, 255]);
        editor.set_selected_paint_enabled("a", false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.set_selected_paint_enabled("g", false);
        assert_eq!(&render(&editor)[..4], &[255, 0, 0, 255]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("g").unwrap().flag("hidden"));
        assert!(snapshot.document.root.find("k").is_some());
        editor.set_selected_paint_enabled("g", false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        editor.set_selected_paint_enabled("g", true);
        assert_eq!(&render(&editor)[..4], &[0, 0, 255, 255]);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn outline_stack_order_and_bypass_preserve_other_children_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        for tag in ["stroke", "shadow"] {
            let source = format!(
                r#"export default <stage><scene id="s" active width={{32}} height={{32}} workarea={{[0,2]}}><rect id="a" width={{8}} height={{8}} x={{8}} y={{8}} fill="white" end={{2}}><{tag} id="first" color="red" width={{4}} blur={{0}} offsetX={{12}}/><solidPaint id="paint" color="white"/><{tag} id="last" color="blue" width={{4}} blur={{0}} offsetX={{12}}/></rect></scene></stage>;"#
            );
            // Stroke has no blur/offset properties; shadow has no width.
            let source = if tag == "stroke" {
                source.replace(" blur={0} offsetX={12}", "")
            } else {
                source.replace(" width={4}", "")
            };
            std::fs::write(folder.join("index.tsx"), &source).unwrap();
            let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
            editor.history = Some(History::new(snapshot.document.clone()));
            editor.replace_snapshot(snapshot);
            editor.selected = "a".into();
            editor.selection = vec!["a".into()];
            let render = |editor: &Editor| {
                let snapshot = editor.snapshot.as_ref().unwrap();
                let tools = Tools::default();
                let mut renderer =
                    ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false)
                        .unwrap();
                renderer
                    .frame_plan("s", 1.0, 30.0, false)
                    .unwrap()
                    .frame(&tools, &folder)
                    .unwrap()
            };
            let before = render(&editor);
            editor.change_selected_outline_stack("first", None, true);
            let after = render(&editor);
            assert_ne!(before, after);
            let snapshot = editor.snapshot.as_ref().unwrap();
            assert_eq!(
                snapshot
                    .document
                    .root
                    .find("a")
                    .unwrap()
                    .children
                    .iter()
                    .map(|child| child.id.as_str())
                    .collect::<Vec<_>>(),
                ["last", "paint", "first"]
            );
            editor.change_selected_outline_stack("first", Some(false), false);
            assert_eq!(render(&editor), before);
            editor.change_selected_outline_stack("first", Some(false), false);
            editor.change_selected_outline_stack("paint", Some(false), false);
            editor.history(false);
            assert_eq!(render(&editor), after);
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                source
            );
        }
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn stroke_join_and_cap_preserve_properties_render_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" width={12} height={12} x={10} y={10} end={2}><stroke id="line" width={8} color="red" opacity={0.75}/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        let angular = render(&editor);
        editor.edit_selected_stroke_shape("line", "join", "round");
        assert_ne!(render(&editor), angular);
        editor.edit_selected_stroke_shape("line", "cap", "square");
        let line = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("line")
            .unwrap();
        assert_eq!(line.string("join"), Some("round"));
        assert_eq!(line.string("cap"), Some("square"));
        assert_eq!(line.number("width", 0.0).unwrap(), 8.0);
        assert_eq!(line.number("opacity", 0.0).unwrap(), 0.75);
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_stroke_shape("line", "join", "invalid");
        editor.edit_selected_stroke_shape("a", "join", "bevel");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert_eq!(render(&editor), angular);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn stroke_miter_limit_changes_corners_rejects_invalid_values_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" width={12} height={12} x={10} y={10} end={2}><stroke id="line" width={8} color="red"/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        let before = render(&editor);
        let mut draft = StrokeDraft {
            rgb: [255, 0, 0],
            width: 8.0,
            opacity: 1.0,
            miter_limit: 1.0,
        };
        editor.edit_selected_stroke("line", Some(draft.clone()));
        assert_ne!(render(&editor), before);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("line")
                .unwrap()
                .number("miterLimit", 10.0)
                .unwrap(),
            1.0
        );
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        for invalid in [f64::NAN, -1.0, 1_000_001.0] {
            draft.miter_limit = invalid;
            editor.edit_selected_stroke("line", Some(draft.clone()));
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert_eq!(render(&editor), before);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn style_duplication_remaps_gradient_tracks_and_keeps_holder_selected() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" width={12} height={12} end={2}><linearGradientPaint id="g" rotation={30}><colorStop id="c" color="red" offset={0}><keyframeTrack id="track" property="offset"><keyframe id="point" time={0} value={0}/></keyframeTrack></colorStop><colorStop id="d" color="blue" offset={1}/></linearGradientPaint><stroke id="line" width={4} color="white"/><shadow id="shade" blur={4} offsetX={2}/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.duplicate_selected_style("a");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        for (id, tag) in [
            ("g", "linearGradientPaint"),
            ("line", "stroke"),
            ("shade", "shadow"),
        ] {
            let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
            editor.duplicate_selected_style(id);
            let snapshot = editor.snapshot.as_ref().unwrap();
            assert_eq!(
                snapshot.document.root.find(&format!("{id}-2")).unwrap().tag,
                tag
            );
            assert_eq!(editor.selected, "a");
            assert_eq!(editor.selection, ["a"]);
            if id == "g" {
                assert!(snapshot.document.root.find("track-2").is_some());
                assert!(snapshot.document.root.find("point-2").is_some());
                assert_eq!(
                    snapshot
                        .document
                        .root
                        .find("g-2")
                        .unwrap()
                        .number("rotation", 0.0)
                        .unwrap(),
                    30.0
                );
                assert_eq!(
                    snapshot.document.root.find("c-2").unwrap().string("color"),
                    Some("red")
                );
            }
            editor.history(false);
            assert_eq!(
                std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
                before
            );
        }
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn effect_and_animation_duplication_keep_tracks_timing_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={16} height={16} workarea={[0,4]}><rect id="a" width={16} height={16} fill="red" end={4}><effect id="fx" type="invert" value={1}><keyframeTrack id="track" property="value"><keyframe id="point" time={0} value={1}/></keyframeTrack></effect><animation id="motion" type="fade" phase="out" duration={0.5} delay={0.25}/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 1.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        assert_eq!(&render(&editor)[..4], &[0, 255, 255, 255]);
        editor.duplicate_selected_style("fx");
        assert_eq!(&render(&editor)[..4], &[255, 0, 0, 255]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("track-2").is_some());
        assert!(snapshot.document.root.find("point-2").is_some());
        let effects = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.duplicate_selected_style("motion");
        let motion = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("motion-2")
            .unwrap();
        assert_eq!(motion.string("type"), Some("fade"));
        assert_eq!(motion.string("phase"), Some("out"));
        assert_eq!(motion.number("duration", 0.0).unwrap(), 0.5);
        assert_eq!(motion.number("delay", 0.0).unwrap(), 0.25);
        assert_eq!(editor.selected, "a");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            effects
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn animation_and_effect_stack_boundaries_do_not_consume_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_animation("fade");
        editor.move_selected_animation("animation-1", false);
        editor.move_selected_animation("animation-1", true);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.insert_selection_effect("invert", 1.0);
        editor.change_effect_stack("effect-1", None, false);
        editor.change_effect_stack("effect-1", None, true);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn effect_type_switch_preserves_same_units_and_resets_cross_unit_tracks() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><rect id="a" end={4}><effect id="fx" type="invert" value={0.7} hidden><keyframeTrack id="track" property="value"><keyframe id="point" time={0} value={0.2}/></keyframeTrack></effect></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.change_selected_effect_type("fx", "sepia");
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("point").is_some());
        assert_eq!(
            snapshot
                .document
                .root
                .find("fx")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            0.7
        );
        let same_unit = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.change_selected_effect_type("fx", "blur");
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("track").is_none());
        assert!(snapshot.document.root.find("fx").unwrap().flag("hidden"));
        assert_eq!(
            snapshot
                .document
                .root
                .find("fx")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            8.0
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            same_unit
        );
        editor.change_selected_effect_type("fx", "invalid");
        editor.change_selected_effect_type("a", "blur");
        editor.change_selected_effect_type("fx", "sepia");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn animation_menu_matches_holder_media_and_preserves_existing_groups() {
        let (_ctx, editor) = editor();
        let mut node = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("a")
            .unwrap()
            .clone();
        assert!(animation_available(&node, "fade", None));
        assert!(!animation_available(&node, "gain", None));
        assert!(!animation_available(&node, "appearWord", None));
        assert!(animation_available(&node, "gain", Some("gain")));
        assert!(animation_available(&node, "appearChar", Some("scramble")));
        node.tag = "audio".into();
        assert!(animation_available(&node, "gain", None));
        node.tag = "video".into();
        assert!(animation_available(&node, "gain", None));
        node.tag = "text".into();
        assert!(animation_available(&node, "appearChar", None));
        assert!(!animation_available(&node, "gain", None));
        let mut paint = node.clone();
        paint.tag = "videoPaint".into();
        node.children.push(paint);
        assert!(animation_available(&node, "gain", None));
    }

    #[test]
    fn image_paint_import_renders_in_holder_and_undoes_without_removing_asset() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(8, 8).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.save_png(folder.join("red & {one}.png")).unwrap();
        std::fs::write(folder.join("broken.png"), b"invalid image").unwrap();
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.media_path = "missing.png".into();
        editor.insert_selection_media_paint(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.media_path = "red & {one}.png".into();
        editor.insert_selection_media_paint(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.media_path = "broken.png".into();
        editor.insert_selection_media_paint(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.media_path = "red & {one}.png".into();
        editor.insert_selection_media_paint(false);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let paint = snapshot.document.root.find("imagePaint-1").unwrap();
        assert_eq!(paint.string("src"), Some("red & {one}.png"));
        assert_eq!(editor.selected, "a");
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
        drop(renderer);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(folder.join("red & {one}.png").is_file());
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn video_paint_import_advances_frames_and_preserves_holder_on_undo_redo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} fill="white" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let tools = Tools::default();
        let video = folder.join("colors & {two}.mkv");
        let output = ad3_editor::media::command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=16x16:r=2:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=16x16:r=2:d=1",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
            ])
            .arg(&video)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let snapshot = Snapshot::load(&folder, &tools).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.media_path = "colors & {two}.mkv".into();
        editor.insert_selection_media_paint(true);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("videoPaint-1")
                .unwrap()
                .string("src"),
            Some("colors & {two}.mkv")
        );
        assert_eq!(
            snapshot
                .document
                .root
                .find("a")
                .unwrap()
                .number("width", 0.0)
                .unwrap(),
            32.0
        );
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let red = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        let blue = renderer
            .frame_plan("s", 1.5, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(red[0] > 240 && red[2] < 10);
        assert!(blue[2] > 240 && blue[0] < 10);
        drop(renderer);
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(video.is_file());
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.remove_selected_media_paint("videoPaint-1");
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("videoPaint-1").is_none());
        assert!(snapshot.document.root.find("a").is_some());
        assert!(video.is_file());
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn media_paint_fit_changes_rendered_letterbox_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(8, 4).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.save_png(folder.join("wide.png")).unwrap();
        let source = r#"export default <stage><scene id="s" active width={8} height={8} workarea={[0,2]}><rect id="a" width={8} height={8} end={2}><imagePaint id="p" src="wide.png" objectFit="cover"/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        let cover = render(&editor);
        assert_eq!(&cover[..4], &[255, 0, 0, 255]);
        editor.set_selected_media_paint_fit("p", "contain");
        let contain = render(&editor);
        assert_eq!(contain[0], 0);
        assert_eq!(contain[(4 * 8 + 4) * 4], 255);
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.set_selected_media_paint_fit("p", "invalid");
        editor.set_selected_media_paint_fit("a", "fill");
        editor.set_selected_media_paint_fit("p", "contain");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.set_selected_media_paint_fit("p", "fill");
        assert_eq!(render(&editor), cover);
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn media_paint_replacement_preserves_fit_and_tracks_renders_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        for (name, color) in [
            ("red.png", tiny_skia::Color::from_rgba8(255, 0, 0, 255)),
            ("blue.png", tiny_skia::Color::from_rgba8(0, 0, 255, 255)),
        ] {
            let mut image = tiny_skia::Pixmap::new(8, 8).unwrap();
            image.fill(color);
            image.save_png(folder.join(name)).unwrap();
        }
        let source = r#"export default <stage><scene id="s" active width={8} height={8} workarea={[0,2]}><rect id="a" width={8} height={8} end={2}><imagePaint id="p" src="red.png" objectFit="contain" opacity={0.5}><keyframeTrack id="t" property="opacity"><keyframe id="k" time={0} value={0.5}/></keyframeTrack></imagePaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.replace_selected_media_paint("p", "missing.png");
        editor.replace_selected_media_paint("a", "blue.png");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.replace_selected_media_paint("p", "blue.png");
        let snapshot = editor.snapshot.as_ref().unwrap();
        let paint = snapshot.document.root.find("p").unwrap();
        assert_eq!(paint.string("objectFit"), Some("contain"));
        assert!(snapshot.document.root.find("k").is_some());
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert_eq!(pixels[0], 0);
        assert!(pixels[2] >= 127);
        drop(renderer);
        editor.replace_selected_media_paint("p", "blue.png");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(folder.join("red.png").is_file());
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn media_paint_opacity_updates_playhead_keyframe_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(8, 8).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.save_png(folder.join("red.png")).unwrap();
        let source = r#"export default <stage><scene id="s" active width={8} height={8} workarea={[0,4]}><rect id="a" width={8} height={8} end={4}><imagePaint id="p" src="red.png"><keyframeTrack id="track" property="opacity"><keyframe id="first" time={0} value={0}/><keyframe id="last" time={2} value={1}/></keyframeTrack></imagePaint></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.transport.as_mut().unwrap().time = 2.0;
        editor.set_selected_media_paint_opacity("p", 0.75);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("last")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            0.75
        );
        assert_eq!(
            snapshot
                .document
                .root
                .find("first")
                .unwrap()
                .number("value", 1.0)
                .unwrap(),
            0.0
        );
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 2.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!((190..=193).contains(&pixels[0]));
        drop(renderer);
        editor.set_selected_media_paint_opacity("p", f64::NAN);
        editor.set_selected_media_paint_opacity("p", 1.1);
        editor.set_selected_media_paint_opacity("a", 0.5);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn paint_blend_renders_multiply_restores_inheritance_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={8} height={8} workarea={[0,2]}><rect id="a" width={8} height={8} fill="blue" end={2}><solidPaint id="p" color="red"/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        assert_eq!(&render(&editor)[..4], &[255, 0, 0, 255]);
        editor.set_selected_paint_blend("p", Some("multiply"));
        assert_eq!(&render(&editor)[..4], &[0, 0, 0, 255]);
        editor.set_selected_paint_blend("p", None);
        assert_eq!(&render(&editor)[..4], &[255, 0, 0, 255]);
        assert!(
            !editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("p")
                .unwrap()
                .props
                .contains_key("blendMode")
        );
        editor.set_selected_paint_blend("p", Some("invalid"));
        editor.set_selected_paint_blend("a", Some("multiply"));
        editor.history(false);
        assert_eq!(&render(&editor)[..4], &[0, 0, 0, 255]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn stroke_blend_changes_overlapping_pixels_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={16} height={16} workarea={[0,2]}><rect id="a" x={4} y={4} width={8} height={8} fill="blue" end={2}><stroke id="line" color="red" width={4}/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let render = |editor: &Editor| {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
            renderer
                .frame_plan("s", 0.0, 30.0, false)
                .unwrap()
                .frame(&tools, &folder)
                .unwrap()
        };
        let pixel = (8 * 16 + 5) * 4;
        assert_eq!(&render(&editor)[pixel..pixel + 4], &[255, 0, 0, 255]);
        editor.set_selected_paint_blend("line", Some("multiply"));
        assert_eq!(&render(&editor)[pixel..pixel + 4], &[0, 0, 0, 255]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn mask_insertion_selects_rect_clips_holder_and_removal_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={64} height={64} workarea={[0,2]}><rect id="a" width={64} height={64} fill="red" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selected_mask();
        assert_eq!(editor.selected, "rect-1");
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mask = snapshot.document.root.find("rect-1").unwrap();
        assert!(mask.flag("mask"));
        assert_eq!(mask.number("width", 0.0).unwrap(), 64.0);
        assert!(!mask.props.contains_key("fill"));
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
        assert_eq!(
            &pixels[(30 * 64 + 30) * 4..(30 * 64 + 30) * 4 + 4],
            &[255, 0, 0, 255]
        );
        drop(renderer);
        let inserted = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.duplicate_selected_mask("rect-1");
        let snapshot = editor.snapshot.as_ref().unwrap();
        let copy = snapshot.document.root.find("rect-1-2").unwrap();
        assert!(copy.flag("mask"));
        assert_eq!(copy.number("width", 0.0).unwrap(), 64.0);
        assert_eq!(copy.number("x", 0.0).unwrap(), 20.0);
        editor.duplicate_selected_mask("a");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            inserted
        );
        editor.remove_selected_mask("rect-1");
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("rect-1")
                .is_none()
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            inserted
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn unchanged_commit_preserves_redo_drafts_and_runtime_only_edits() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.rename_selected_clip("Novo nome");
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        let unchanged = editor.snapshot.as_ref().unwrap().document.clone();
        assert!(editor.commit(Ok(unchanged)));
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor
            .media_paint_source_drafts
            .insert("draft".into(), "paused.png".into());
        let unchanged = editor.snapshot.as_ref().unwrap().document.clone();
        assert!(editor.commit(Ok(unchanged.clone())));
        assert_eq!(
            editor
                .media_paint_source_drafts
                .get("draft")
                .map(String::as_str),
            Some("paused.png")
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        let mut skews = editor.runtime_skews.clone();
        skews.insert("a".into(), [12.0, 0.0]);
        assert!(editor.commit_with_skews(Ok(document), skews));
        assert_eq!(editor.runtime_skews.get("a"), Some(&[12.0, 0.0]));
        editor.history(false);
        assert!(!editor.runtime_skews.contains_key("a"));
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn solid_paint_insertion_edit_removal_render_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} fill="blue" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_paint();
        editor.edit_selected_paint(
            "solidPaint-1",
            Some(PaintDraft {
                rgb: [255, 0, 0],
                opacity: 1.0,
            }),
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_paint("solidPaint-1", None);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("solidPaint-1")
                .is_none()
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn stroke_inspector_samples_width_color_and_opacity_tracks_at_playhead() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><rect id="a" end={4}><stroke id="line" color="red" width={2} opacity={1}><keyframeTrack property="width"><keyframe value={2}/><keyframe time={2} value={6}/></keyframeTrack><keyframeTrack property="opacity"><keyframe value={1}/><keyframe time={2} value={0.5}/></keyframeTrack><keyframeTrack property="color"><keyframe value="red"/><keyframe time={2} value="blue"/></keyframeTrack></stroke></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        let snapshot = editor.snapshot.clone().unwrap();
        let holder = snapshot.document.root.find("a").unwrap();
        let stroke = snapshot.document.root.find("line").unwrap();
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        editor.refresh_stroke_draft(&snapshot, holder, stroke);
        let draft = &editor.stroke_drafts.get("line").unwrap().1;
        assert_eq!(draft.width, 4.0);
        assert_eq!(draft.opacity, 0.75);
        assert_eq!(draft.rgb, [128, 0, 128]);
        editor.stroke_drafts.get_mut("line").unwrap().1.width = 5.0;
        editor.refresh_stroke_draft(&snapshot, holder, stroke);
        assert_eq!(editor.stroke_drafts.get("line").unwrap().1.width, 5.0);
        editor.transport.as_mut().unwrap().seek(0.0).unwrap();
        editor.refresh_stroke_draft(&snapshot, holder, stroke);
        assert_eq!(editor.stroke_drafts.get("line").unwrap().1.width, 2.0);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn stroke_insertion_edit_and_removal_render_and_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" x={8} y={8} width={16} height={16} fill="blue" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_stroke();
        editor.edit_selected_stroke(
            "stroke-1",
            Some(StrokeDraft {
                rgb: [255, 0, 0],
                width: 4.0,
                opacity: 1.0,
                miter_limit: 10.0,
            }),
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10)
        );
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_stroke("stroke-1", None);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("stroke-1")
                .is_none()
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn shadow_inspector_samples_shape_tracks_and_keeps_text_shadow_authored() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><rect id="a" end={4}><shadow id="shape" color={16711680} offsetX={2}><keyframeTrack property="offsetX"><keyframe value={2}/><keyframe time={2} value={10}/></keyframeTrack></shadow></rect><text id="t" end={4}>Texto<shadow id="text-shadow" offsetX={2}><keyframeTrack property="offsetX"><keyframe value={2}/><keyframe time={2} value={10}/></keyframeTrack></shadow></text></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        let snapshot = editor.snapshot.clone().unwrap();
        let holder = snapshot.document.root.find("a").unwrap();
        let shadow = snapshot.document.root.find("shape").unwrap();
        editor.refresh_shadow_draft(&snapshot, holder, shadow);
        let draft = &editor.shadow_drafts.get("shape").unwrap().1;
        assert_eq!(draft.offset[0], 6.0);
        assert_eq!(draft.rgb, [255, 0, 0]);
        editor.shadow_drafts.get_mut("shape").unwrap().1.offset[0] = 7.0;
        editor.refresh_shadow_draft(&snapshot, holder, shadow);
        assert_eq!(editor.shadow_drafts.get("shape").unwrap().1.offset[0], 7.0);
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.refresh_shadow_draft(&snapshot, holder, shadow);
        assert_eq!(editor.shadow_drafts.get("shape").unwrap().1.offset[0], 10.0);
        editor.refresh_shadow_draft(
            &snapshot,
            snapshot.document.root.find("t").unwrap(),
            snapshot.document.root.find("text-shadow").unwrap(),
        );
        assert_eq!(
            editor.shadow_drafts.get("text-shadow").unwrap().1.offset[0],
            2.0
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn shadow_edit_and_removal_preserve_holder_and_undo_together() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_shadow();
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_shadow(
            "shadow-1",
            Some(ShadowDraft {
                rgb: [255, 0, 0],
                opacity: 0.75,
                blur: 4.0,
                offset: [12.0, -6.0],
            }),
        );
        let shadow = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("shadow-1")
            .unwrap();
        assert_eq!(shadow.string("color"), Some("#ff0000"));
        assert_eq!(shadow.number("opacity", 0.0).unwrap(), 0.75);
        assert_eq!(shadow.number("blur", 0.0).unwrap(), 4.0);
        assert_eq!(shadow.number("offsetX", 0.0).unwrap(), 12.0);
        assert_eq!(shadow.number("offsetY", 0.0).unwrap(), -6.0);
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_shadow("shadow-1", None);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("shadow-1")
                .is_none()
        );
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a")
                .is_some()
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn shadow_insertion_renders_outside_shape_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={32} workarea={[0,2]}><rect id="a" x={8} y={8} width={8} height={8} fill="white" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_shadow();
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert!(snapshot.document.root.find("shadow-1").is_some());
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 && pixel[3] > 0)
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("shadow-1")
                .is_some()
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn effect_stack_order_and_bypass_preserve_parameters_and_undo() {
        fn pixel(editor: &Editor) -> [u8; 4] {
            let snapshot = editor.snapshot.as_ref().unwrap();
            let tools = Tools::default();
            let mut renderer =
                ProjectRenderer::new(&snapshot.document, &snapshot.project, tools.clone(), false)
                    .unwrap();
            let pixels = renderer
                .frame_plan("s", 1.0, 30.0, false)
                .unwrap()
                .frame(&tools, &snapshot.project)
                .unwrap();
            pixels[..4].try_into().unwrap()
        }
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} fill="red" end={2}><effect id="invert" type="invert" value={1}/><animation id="fade" type="fade" duration={0.1}/><effect id="brightness" type="brightness" value={0.5}/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let before_pixel = pixel(&editor);
        assert_eq!(before_pixel[0], 0);
        assert!((127..=129).contains(&before_pixel[1]));
        assert!((127..=129).contains(&before_pixel[2]));
        editor.change_effect_stack("invert", None, true);
        let reordered_pixel = pixel(&editor);
        assert!((127..=129).contains(&reordered_pixel[0]));
        assert_eq!(&reordered_pixel[1..], &[255, 255, 255]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("a")
                .unwrap()
                .children
                .iter()
                .map(|child| child.id.as_str())
                .collect::<Vec<_>>(),
            vec!["brightness", "fade", "invert"]
        );
        editor.change_effect_stack("invert", Some(false), false);
        let bypassed_pixel = pixel(&editor);
        assert!((127..=129).contains(&bypassed_pixel[0]));
        assert_eq!(&bypassed_pixel[1..], &[0, 0, 255]);
        let effect = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("invert")
            .unwrap();
        assert!(effect.flag("hidden"));
        assert_eq!(effect.number("value", 0.0).unwrap(), 1.0);
        editor.history(false);
        assert!(
            !editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("invert")
                .unwrap()
                .flag("hidden")
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a")
                .unwrap()
                .children[0]
                .id,
            "brightness"
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn effect_edit_updates_current_keyframe_and_removal_and_undo_restore_tracks() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><rect id="a" end={4}><effect id="e" type="invert" value={1}><keyframeTrack property="value"><keyframe id="k0" value={0}/><keyframe id="k1" time={2} value={1}/></keyframeTrack></effect></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.edit_selected_effect("e", Some(0.75));
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("k1")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            0.75
        );
        let edited = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.edit_selected_effect("e", Some(f64::NAN));
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.edit_selected_effect("e", None);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("e")
                .is_none()
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            edited
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn effect_insertion_renders_new_pixels_and_undo_restores_exact_source() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect id="a" width={32} height={16} fill="red" end={2}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_effect("invert", 1.0);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("effect-1")
                .unwrap()
                .string("type"),
            Some("invert")
        );
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] < 10 && pixel[1] > 240 && pixel[2] > 240)
        );
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.insert_selection_effect("invalid", 1.0);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn opacity_inspector_samples_animation_edits_keyframe_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><rect id="a" end={4} opacity={1}><keyframeTrack property="opacity"><keyframe id="first" time={0} value={0}/><keyframe id="last" time={2} value={1}/></keyframeTrack></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        let snapshot = editor.snapshot.clone().unwrap();
        editor.refresh_opacity_draft(&snapshot, "a");
        assert_eq!(editor.opacity_draft.as_ref().unwrap().2, 0.5);
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.refresh_opacity_draft(&snapshot, "a");
        assert_eq!(editor.opacity_draft.as_ref().unwrap().2, 1.0);
        editor.set_selection_opacity(0.75);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("last")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            0.75
        );
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.set_selection_opacity(f64::NAN);
        editor.set_selection_opacity(1.1);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn blend_mode_batch_changes_and_restoration_are_undoable() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into(), "b".into()];
        editor.set_selection_blend(Some("multiply"));
        let snapshot = editor.snapshot.as_ref().unwrap();
        for id in ["a", "b"] {
            assert_eq!(
                snapshot.document.root.find(id).unwrap().string("blendMode"),
                Some("multiply")
            );
        }
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.set_selection_blend(Some("invalid"));
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        editor.set_selection_blend(None);
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .string("blendMode")
                    .is_none()
            );
        }
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn changing_animation_preset_preserves_timing_and_identity_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.animation_duration = 0.5;
        editor.animation_delay = 0.25;
        editor.insert_selection_animation("fade");
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.change_selected_animation_preset("animation-1", "appearChar");
        editor.change_selected_animation_preset("animation-1", "invalid");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.change_selected_animation_preset("animation-1", "grow");
        let animation = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("animation-1")
            .unwrap();
        assert_eq!(animation.string("type"), Some("grow"));
        assert_eq!(animation.number("duration", 0.0).unwrap(), 0.5);
        assert_eq!(animation.number("delay", 0.0).unwrap(), 0.25);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("animation-1")
                .unwrap()
                .string("type"),
            Some("grow")
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn animation_order_changes_rendered_motion_preserves_other_children_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active><rect id="a" end={3}><animation id="grow" type="grow"/><solidPaint id="paint" color="red"/><animation id="shrink" type="shrink"/><keyframeTrack id="track" property="opacity"><keyframe id="point" value={0.5}/></keyframeTrack></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.move_selected_animation("grow", true);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let holder = snapshot.document.root.find("a").unwrap();
        assert_eq!(
            holder
                .children
                .iter()
                .map(|child| child.id.as_str())
                .collect::<Vec<_>>(),
            vec!["shrink", "paint", "grow", "track"]
        );
        let sample = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, 0.0, 30.0)
            .unwrap();
        assert_eq!(
            sample.find("a").unwrap().number("scaleX", 1.0).unwrap(),
            0.5
        );
        assert_eq!(
            sample.find("a").unwrap().number("opacity", 1.0).unwrap(),
            0.5
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a")
                .unwrap()
                .children[0]
                .id,
            "shrink"
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn animation_edit_preserves_identity_and_preset_and_undoes_all_timing_together() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.insert_selection_animation("fade");
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.update_selected_animation("animation-1", true, 0.5, 0.25);
        let animation = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("animation-1")
            .unwrap();
        assert_eq!(animation.string("type"), Some("fade"));
        assert_eq!(animation.string("phase"), Some("out"));
        assert_eq!(animation.number("duration", 0.0).unwrap(), 0.5);
        assert_eq!(animation.number("delay", 0.0).unwrap(), 0.25);
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.update_selected_animation("animation-1", false, f64::NAN, 0.0);
        editor.update_selected_animation("b", false, 1.0, 0.0);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn removing_animation_preserves_other_presets_and_tracks_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active><rect id="a" end={3}><animation id="fade" type="fade"/><animation id="spin" type="spin"/><keyframeTrack id="track" property="opacity"><keyframe id="point" value={0.5}/></keyframeTrack></rect><rect id="b"><animation id="other" type="fade"/></rect></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        editor.remove_selected_animation("other");
        editor.remove_selected_animation("track");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.remove_selected_animation("fade");
        let root = &editor.snapshot.as_ref().unwrap().document.root;
        assert!(root.find("fade").is_none());
        for id in ["spin", "track", "point", "other"] {
            assert!(root.find(id).is_some());
        }
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("fade")
                .is_none()
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn animation_presets_insert_in_one_batch_render_and_undo_and_reject_invalid_targets() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into(), "b".into()];
        editor.animation_duration = 0.5;
        editor.insert_selection_animation("fade");
        let snapshot = editor.snapshot.as_ref().unwrap();
        for id in ["a", "b"] {
            assert!(
                snapshot
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .children
                    .iter()
                    .any(|child| child.tag == "animation")
            );
        }
        let sample = ad3_editor::motion::Motion::default()
            .sample(&snapshot.document.root, &snapshot.timeline, 0.0, 30.0)
            .unwrap();
        assert_eq!(
            sample.find("a").unwrap().number("opacity", 1.0).unwrap(),
            0.0
        );
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.insert_selection_animation("appearChar");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn text_case_inheritance_restoration_keeps_authored_characters_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active><text id="t" textCase="upper">Ação e Café<textRange id="r" textCase="lower"/></text></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "r".into();
        editor.selection = vec!["r".into()];
        editor.property = "textCase".into();
        editor.restore_text_inheritance();
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            inherited_text_property(
                &snapshot.document.root,
                "r",
                "textCase",
                &serde_json::json!("original")
            ),
            serde_json::json!("upper")
        );
        assert_eq!(
            snapshot.document.root.find("t").unwrap().text,
            "Ação e Café"
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn text_leading_changes_measured_height_resets_default_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={320} height={320} workarea={[0,4]}><text id="t" fontSize={24} color="white">{"Linha\nOutra"}<textRange id="r" fontWeight="bold"/></text><rect id="a" width={12} height={10}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["t".into()];
        editor.selected = "t".into();
        let measure = |document: &SourceDocument| {
            let mut renderer =
                ProjectRenderer::new(document, &folder, Tools::default(), false).unwrap();
            renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
            renderer.edit_geometries("s").unwrap()["t"].frame.size
        };
        let original_size = measure(&editor.snapshot.as_ref().unwrap().document);
        editor.set_selection_text_leading(200.0);
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert_eq!(
            document
                .root
                .find("t")
                .unwrap()
                .number("leading", 1.0)
                .unwrap(),
            2.0
        );
        assert!(document.root.find("r").is_some());
        let expanded_size = measure(document);
        assert_eq!(expanded_size[0], original_size[0]);
        assert!(expanded_size[1] > original_size[1]);
        let expanded = document.source.clone();
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 1201.0] {
            editor.set_selection_text_leading(invalid);
            assert_eq!(editor.snapshot.as_ref().unwrap().document.source, expanded);
        }
        editor.selection = vec!["t".into(), "a".into()];
        editor.set_selection_text_leading(150.0);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, expanded);
        editor.selection = vec!["t".into()];
        editor.set_selection_text_leading(100.0);
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert!(
            !document
                .root
                .find("t")
                .unwrap()
                .props
                .contains_key("leading")
        );
        assert_eq!(measure(document), original_size);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            expanded
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            expanded
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn text_box_fixed_uses_current_preview_size_and_undoes_as_one_edit() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={320} height={160} workarea={[0,4]}><text id="t" fontSize={24} color="white">Texto</text></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["t".into()];
        editor.selected = "t".into();
        editor.set_selected_text_box(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, Tools::default(), false).unwrap();
        renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        let geometry = renderer.edit_geometries("s").unwrap().clone();
        let size = geometry["t"].frame.size;
        assert!(size[0] > 0.0 && size[1] > 0.0);
        editor.frame_geometry = Some((1.0, geometry.clone()));
        editor.set_selected_text_box(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.frame_geometry = Some((0.0, geometry));
        editor.set_selected_text_box(true);
        let document = &editor.snapshot.as_ref().unwrap().document;
        let text = document.root.find("t").unwrap();
        assert_eq!(text.number("width", 0.0).unwrap(), size[0].round());
        assert_eq!(text.number("height", 0.0).unwrap(), size[1].round());
        assert_eq!(text.text, "Texto");
        let fixed = document.source.clone();
        editor.set_selected_text_box(true);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            fixed
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn text_box_auto_removes_only_size_props_and_tracks_and_restores_them_on_undo() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={320} height={160} workarea={[0,4]}><text id="t" width={80} height={40} fontSize={24}>Texto<keyframeTrack id="w" property="width"><keyframe id="wp" time={0} value={80}/><keyframe id="wp2" time={2} value={120}/></keyframeTrack><keyframeTrack id="h" property="height"><keyframe id="hp" time={0} value={40}/></keyframeTrack><keyframeTrack id="c" property="color"><keyframe id="cp" time={0} value="red"/></keyframeTrack><textRange id="r" fontWeight="bold"/></text><rect id="a" width={12} height={10}/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["t".into()];
        editor.selected = "t".into();
        editor.set_selected_text_box(false);
        let document = &editor.snapshot.as_ref().unwrap().document;
        let text = document.root.find("t").unwrap();
        assert!(!text.props.contains_key("width") && !text.props.contains_key("height"));
        for id in ["w", "wp", "wp2", "h", "hp"] {
            assert!(document.root.find(id).is_none());
        }
        for id in ["c", "cp", "r", "a"] {
            assert!(document.root.find(id).is_some());
        }
        let mut renderer =
            ProjectRenderer::new(document, &folder, Tools::default(), false).unwrap();
        renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        assert!(renderer.edit_geometries("s").unwrap()["t"].frame.size[0] > 0.0);
        let automatic = document.source.clone();
        editor.set_selected_text_box(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            automatic
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn typography_inspector_samples_animated_parent_at_playhead_without_writing_source() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]}><text id="t" color="red">Texto<keyframeTrack property="color"><keyframe time={0} value="red"/><keyframe time={2} value="blue"/></keyframeTrack><textRange id="r"/></text></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        let snapshot = editor.snapshot.clone().unwrap();
        let default = serde_json::json!("white");
        for (time, expected) in [
            (0.0, "#FF0000"),
            (1.0, "#800080"),
            (2.0, "#0000FF"),
            (0.0, "#FF0000"),
        ] {
            editor.transport.as_mut().unwrap().seek(time).unwrap();
            assert_eq!(
                editor
                    .text_property_at_playhead(&snapshot, "r", "color", &default)
                    .as_str(),
                Some(expected)
            );
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn restoring_text_inheritance_removes_only_local_property_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active><text id="t" fontSize={42}>Texto<textRange id="r" fontSize={24} color="red"/></text><rect id="a"/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "r".into();
        editor.selection = vec!["r".into()];
        editor.property = "fontSize".into();
        editor.restore_text_inheritance();
        let snapshot = editor.snapshot.as_ref().unwrap();
        let range = snapshot.document.root.find("r").unwrap();
        assert!(!range.props.contains_key("fontSize"));
        assert_eq!(range.string("color"), Some("red"));
        assert_eq!(
            inherited_text_property(
                &snapshot.document.root,
                "r",
                "fontSize",
                &serde_json::json!(16)
            )
            .as_f64(),
            Some(42.0)
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.selection.push("a".into());
        editor.restore_text_inheritance();
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            !editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("r")
                .unwrap()
                .props
                .contains_key("fontSize")
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn typography_controls_inherit_nested_text_ranges_without_leaking_sibling_styles() {
        let document = SourceDocument::parse(PathBuf::from("text.tsx"), r#"export default <stage><scene id="s" active><text id="t" fontSize={42} fontFamily="Inter" color="red">Texto<textRange id="r" fontWeight="bold"><textRange id="nested" color="blue"/></textRange><textRange id="sibling"/></text><text id="other">Outro</text></scene></stage>;"#.into()).unwrap();
        let root = &document.root;
        assert_eq!(
            inherited_text_property(root, "nested", "fontSize", &serde_json::json!(16)),
            serde_json::json!(42.0)
        );
        assert_eq!(
            inherited_text_property(root, "nested", "fontWeight", &serde_json::json!("normal")),
            serde_json::json!("bold")
        );
        assert_eq!(
            inherited_text_property(root, "nested", "color", &serde_json::json!("white")),
            serde_json::json!("blue")
        );
        assert_eq!(
            inherited_text_property(root, "sibling", "fontWeight", &serde_json::json!("normal")),
            serde_json::json!("normal")
        );
        assert_eq!(
            inherited_text_property(root, "other", "fontSize", &serde_json::json!(16)),
            serde_json::json!(16)
        );
    }

    #[test]
    fn volume_inspector_samples_playhead_preserves_draft_and_edits_current_keyframe() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active workarea={[0,4]} volume={0}><keyframeTrack id="gain" property="volume"><keyframe id="k0" time={0} value={-12}/><keyframe id="k1" time={2} value={0}/></keyframeTrack></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "s".into();
        editor.selection = vec!["s".into()];
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        let snapshot = editor.snapshot.clone().unwrap();
        editor.refresh_volume_draft(&snapshot, "s");
        assert!((editor.volume_draft.as_ref().unwrap().2 + 6.0).abs() < 1e-6);
        editor.volume_draft.as_mut().unwrap().2 = -9.0;
        editor.refresh_volume_draft(&snapshot, "s");
        assert_eq!(editor.volume_draft.as_ref().unwrap().2, -9.0);
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.refresh_volume_draft(&snapshot, "s");
        assert_eq!(editor.volume_draft.as_ref().unwrap().2, 0.0);
        editor.set_selection_volume(-3.0);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("k1")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            -3.0
        );
        assert_eq!(
            snapshot
                .document
                .root
                .find("s")
                .unwrap()
                .number("volume", 0.0)
                .unwrap(),
            -3.0
        );
        assert_eq!(
            snapshot
                .document
                .root
                .find("k0")
                .unwrap()
                .number("value", 0.0)
                .unwrap(),
            -12.0
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn volume_batch_preserves_timing_rejects_invalid_values_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "s".into();
        editor.selection = vec!["s".into(), "a".into()];
        editor.set_selection_volume(-6.0);
        let snapshot = editor.snapshot.as_ref().unwrap();
        for id in ["s", "a"] {
            assert_eq!(
                snapshot
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .number("volume", 0.0)
                    .unwrap(),
                -6.0
            );
        }
        assert_eq!(snapshot.timeline.get("a").unwrap().end, 2.0);
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        for value in [f64::NAN, f64::INFINITY, -61.0, 13.0] {
            editor.set_selection_volume(value);
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn removing_media_materializes_intrinsic_dimensions_preserves_children_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(18, 10).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.save_png(folder.join("native.png")).unwrap();
        let source = r#"export default <stage><scene id="s" active width={64} height={32} workarea={[0,5]}><image id="i" src="native.png" x={4} y={6}><solidPaint id="paint" color="blue"/></image></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selected = "i".into();
        editor.selection = vec!["i".into()];
        editor.remove_selected_intrinsic_media();
        let snapshot = editor.snapshot.as_ref().unwrap();
        let node = snapshot.document.root.find("i").unwrap();
        assert_eq!(node.tag, "rect");
        assert_eq!(node.number("width", 0.0).unwrap(), 18.0);
        assert_eq!(node.number("height", 0.0).unwrap(), 10.0);
        assert_eq!(node.number("x", 0.0).unwrap(), 4.0);
        assert_eq!(node.number("y", 0.0).unwrap(), 6.0);
        assert!(node.find("paint").is_some());
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("i")
                .unwrap()
                .tag,
            "rect"
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn media_replacement_preserves_clip_properties_renders_new_pixels_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.resize_active_scene(64, 32);
        for (name, color) in [
            ("red.png", tiny_skia::Color::from_rgba8(255, 0, 0, 255)),
            (
                "blue & {novo}.png",
                tiny_skia::Color::from_rgba8(0, 0, 255, 255),
            ),
        ] {
            let mut image = tiny_skia::Pixmap::new(8, 4).unwrap();
            image.fill(color);
            image.save_png(folder.join(name)).unwrap();
        }
        editor.media_path = "red.png".into();
        editor.insert_media("image");
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        let mut properties = editor
            .snapshot
            .as_ref()
            .unwrap()
            .document
            .root
            .find("image-1")
            .unwrap()
            .props
            .clone();
        properties.remove("src");
        editor.replace_selected_media("blue & {novo}.png");
        let snapshot = editor.snapshot.as_ref().unwrap();
        let node = snapshot.document.root.find("image-1").unwrap();
        let mut replaced = node.props.clone();
        replaced.remove("src");
        assert_eq!(properties, replaced);
        assert_eq!(node.string("src"), Some("blue & {novo}.png"));
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let pixels = renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .frame(&tools, &folder)
            .unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] < 10 && pixel[1] < 10 && pixel[2] > 240)
        );
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.replace_selected_media("missing.png");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        let span = editor
            .snapshot
            .as_ref()
            .unwrap()
            .timeline
            .get("image-1")
            .unwrap()
            .clone();
        editor.remove_selected_intrinsic_media();
        let snapshot = editor.snapshot.as_ref().unwrap();
        let rect = snapshot.document.root.find("image-1").unwrap();
        assert_eq!(rect.tag, "rect");
        assert!(rect.string("src").is_none());
        assert!(rect.string("objectFit").is_none());
        assert_eq!(rect.number("width", 0.0).unwrap(), 8.0);
        assert_eq!(rect.number("height", 0.0).unwrap(), 4.0);
        assert_eq!(rect.string("name"), Some("red 1"));
        let rect_span = snapshot.timeline.get("image-1").unwrap();
        assert_eq!([rect_span.start, rect_span.end], [span.start, span.end]);
        assert!(folder.join("red.png").is_file());
        assert!(folder.join("blue & {novo}.png").is_file());
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("image-1")
                .unwrap()
                .tag,
            "rect"
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn media_fit_changes_are_atomic_undoable_and_reject_mixed_selection() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(4, 8).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let file = folder.join("fit.png");
        image.save_png(&file).unwrap();
        editor.insert_dropped_files(&[file.clone(), file]);
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.set_media_fit("cover");
        for id in ["image-1", "image-2"] {
            assert_eq!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .string("objectFit"),
                Some("cover")
            );
        }
        let after = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.set_media_fit("invalid");
        editor.selection.push("a".into());
        editor.set_media_fit("fill");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            after
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn clip_rename_preserves_identity_timing_and_undo_and_rejects_invalid_selection() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        editor.selected = "a".into();
        editor.selection = vec!["a".into()];
        let name = "Abertura \"ação\" & {teste}";
        editor.rename_selected_clip(name);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot.document.root.find("a").unwrap().string("name"),
            Some(name)
        );
        assert_eq!(snapshot.timeline.get("a").unwrap().end, 2.0);
        assert_eq!(editor.selected, "a");
        let renamed = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.rename_selected_clip("   ");
        editor.selection.push("b".into());
        editor.rename_selected_clip("Lote");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            renamed
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            renamed
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn local_image_insertion_resolves_relative_path_renders_and_undoes_exactly() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.resize_active_scene(64, 32);
        let before = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        let mut image = tiny_skia::Pixmap::new(4, 4).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        image.save_png(folder.join("asset & {one}.png")).unwrap();
        editor.media_path = "asset & {one}.png".into();
        editor.insert_media("image");
        assert_eq!(editor.selected, "image-1");
        let snapshot = editor.snapshot.as_ref().unwrap();
        let inserted = snapshot.document.root.find("image-1").unwrap();
        assert_eq!(inserted.string("name"), Some("asset & {one} 1"));
        assert_eq!(inserted.number("width", 0.0).unwrap(), 4.0);
        assert_eq!(inserted.number("height", 0.0).unwrap(), 4.0);
        assert_eq!(inserted.number("x", 0.0).unwrap(), 30.0);
        assert_eq!(inserted.number("y", 0.0).unwrap(), 14.0);
        assert_eq!(
            snapshot
                .document
                .root
                .find("image-1")
                .unwrap()
                .string("src"),
            Some("asset & {one}.png")
        );
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        let pixels = plan.frame(&tools, &folder).unwrap();
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10)
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        editor.media_path = "missing.png".into();
        editor.insert_media("image");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            before
        );
        drop(renderer);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn dropped_media_batch_is_one_undo_and_invalid_batch_preserves_source() {
        let (_ctx, mut editor) = editor();
        let (folder, source) = persist(&mut editor);
        let mut image = tiny_skia::Pixmap::new(4, 4).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let file = folder.join("drop.PNG");
        image.save_png(&file).unwrap();
        editor.insert_dropped_files(&[file.clone(), folder.join("missing.png")]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.insert_dropped_files(&[file.clone(), file]);
        assert_eq!(editor.selection, vec!["image-1", "image-2"]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("image-1")
                .unwrap()
                .string("name"),
            Some("drop 1")
        );
        assert_eq!(
            snapshot
                .document
                .root
                .find("image-2")
                .unwrap()
                .string("name"),
            Some("drop 2")
        );
        assert_eq!(editor.selected, "image-2");
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("image-2")
                .is_some()
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn first_dropped_image_creates_matching_scene_and_undo_restores_empty_project() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = "export default <stage/>;";
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        let mut image = tiny_skia::Pixmap::new(16, 8).unwrap();
        image.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let file = folder.join("first.png");
        image.save_png(&file).unwrap();
        editor.insert_dropped_files(&[file]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.timeline.active, "scene-1");
        let scene = snapshot.document.root.find("scene-1").unwrap();
        assert_eq!(scene.number("width", 0.0).unwrap(), 16.0);
        assert_eq!(scene.number("height", 0.0).unwrap(), 8.0);
        assert_eq!(editor.selection, vec!["image-1"]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(editor.transport.is_none());
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("image-1")
                .is_some()
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn empty_project_drop_uses_last_visual_dimensions_and_rejects_partial_batches() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = "export default <stage/>;";
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        let first = folder.join("first.png");
        let last = folder.join("last.png");
        tiny_skia::Pixmap::new(16, 8)
            .unwrap()
            .save_png(&first)
            .unwrap();
        tiny_skia::Pixmap::new(8, 24)
            .unwrap()
            .save_png(&last)
            .unwrap();
        editor.insert_dropped_files(&[first.clone(), folder.join("missing.png")]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(editor.transport.is_none());
        editor.insert_dropped_files(&[first, last]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let scene = snapshot.document.root.find("scene-1").unwrap();
        assert_eq!(scene.number("width", 0.0).unwrap(), 8.0);
        assert_eq!(scene.number("height", 0.0).unwrap(), 24.0);
        assert_eq!(editor.selection, vec!["image-1", "image-2"]);
        let first = snapshot.document.root.find("image-1").unwrap();
        assert_eq!(first.number("width", 0.0).unwrap(), 16.0);
        assert_eq!(first.number("height", 0.0).unwrap(), 8.0);
        assert_eq!(first.number("x", 0.0).unwrap(), -4.0);
        assert_eq!(first.number("y", 0.0).unwrap(), 8.0);
        let last = snapshot.document.root.find("image-2").unwrap();
        assert_eq!(last.number("width", 0.0).unwrap(), 8.0);
        assert_eq!(last.number("height", 0.0).unwrap(), 24.0);
        assert_eq!(last.number("x", 1.0).unwrap(), 0.0);
        assert_eq!(last.number("y", 1.0).unwrap(), 0.0);
        assert_eq!(editor.transport.as_ref().unwrap().workarea, [0.0, 5.0]);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(editor.transport.is_none());
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("image-2")
                .is_some()
        );
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn current_frame_png_uses_playhead_and_authored_resolution_and_preserves_existing_file() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><rect width={32} height={16} fill="red" end={1}/><rect width={32} height={16} fill="blue" start={1} end={2}/></scene></stage>;"#;
        let document = SourceDocument::parse(folder.join("index.tsx"), source.into()).unwrap();
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        // The end boundary is exclusive; capture must match the preview's
        // final visible frame instead of an empty frame at the boundary.
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.export_path = folder.join("selected.mov").to_string_lossy().into_owned();
        editor.capture_frame(ctx.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while editor.capturing && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.capturing, "{}", editor.status);
        let output = folder.join("selected.png");
        let bytes = std::fs::read(&output).unwrap();
        let image = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (32, 16));
        let pixel = image.pixel(16, 8).unwrap();
        assert!(pixel.blue() > 240 && pixel.red() < 10);
        editor.transport.as_mut().unwrap().seek(0.0).unwrap();
        editor.capture_frame(ctx.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while editor.capturing && Instant::now() < deadline {
            editor.poll(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!editor.capturing);
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        let snapshot = editor.snapshot.as_ref().unwrap();
        let tools = Tools::default();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, tools.clone(), false).unwrap();
        let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        let cancelled_output = folder.join("cancelled.png");
        let cancel = AtomicBool::new(true);
        assert!(
            plan.capture_cancellable(&tools, &folder, &cancelled_output, Some(&cancel))
                .is_err()
        );
        assert!(!cancelled_output.exists());
        cancel.store(false, Ordering::Relaxed);
        plan.capture_cancellable(&tools, &folder, &cancelled_output, Some(&cancel))
            .unwrap();
        assert!(cancelled_output.exists());
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, source);
        drop(editor);
        cleanup(folder);
    }

    #[test]
    fn export_preferences_load_per_scene_and_survive_composition_edits() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r#"{"diffusion":{"export":{"s":{"format":"mov","video":{"fps":23.976,"resolution":720,"enabled":false,"codec":"hevc"},"audio":{"codec":"aac"}}}}}"#,
        )
        .unwrap();
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        editor.snapshot = None;
        editor.replace_snapshot(
            Snapshot::from_document(document.clone(), folder.clone(), &Tools::default()).unwrap(),
        );
        assert_eq!(editor.export_fps, 23.976);
        assert_eq!(editor.export_resolution, 720);
        assert!(!editor.export_video);
        assert_eq!(editor.export_video_codec, "hevc");
        assert_eq!(editor.export_audio_codec, "aac");
        assert!(editor.export_path.ends_with("export.mov"));
        editor.export_fps = 60.0;
        editor.export_resolution = 540;
        editor.export_video_bitrate = 3_000_000;
        editor.export_video = true;
        editor.export_video_codec = "avc".into();
        editor.export_audio_codec = "mp3".into();
        editor.export_path = folder.join("custom.mov").to_string_lossy().into_owned();
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        assert_eq!(editor.export_fps, 60.0);
        assert_eq!(editor.export_resolution, 540);
        assert!(editor.export_video);
        assert_eq!(editor.export_video_codec, "avc");
        assert_eq!(editor.export_audio_codec, "mp3");
        assert!(editor.export_path.ends_with("custom.mov"));
        editor.export_audio = false;
        editor.export_audio_channels = 1;
        editor.export_audio_sample_rate = 44_100;
        editor.export_audio_bitrate = 96_000;
        editor.save_export_preferences();
        // The audio bitrate is saved independently of whether the track is enabled.
        let settings = ad3_editor::project_config::ProjectConfig::load(&folder)
            .unwrap()
            .export_settings("s");
        assert_eq!(settings["format"], "mov");
        assert_eq!(settings["video"]["fps"], 60.0);
        assert_eq!(settings["video"]["resolution"], 540.0);
        assert_eq!(settings["video"]["bitrate"], 3_000_000.0);
        assert_eq!(settings["video"]["enabled"], true);
        assert_eq!(settings["video"]["codec"], "avc");
        assert_eq!(settings["audio"]["enabled"], false);
        assert_eq!(settings["audio"]["codec"], "mp3");
        assert_eq!(settings["audio"]["numberOfChannels"], 1.0);
        assert_eq!(settings["audio"]["sampleRate"], 44100.0);
        assert_eq!(settings["audio"]["bitrate"], 96_000.0);
        editor.export_fps = f64::NAN;
        editor.export(egui::Context::default());
        assert!(!editor.exporting);
        assert!(editor.status.contains("FPS"));
        std::fs::write(
            folder.join("package.json"),
            r#"{"diffusion":{"export":{"s":{"format":"avi","video":{"fps":30}}}}}"#,
        )
        .unwrap();
        let document = editor.snapshot.as_ref().unwrap().document.clone();
        editor.snapshot = None;
        editor.replace_snapshot(
            Snapshot::from_document(document, folder.clone(), &Tools::default()).unwrap(),
        );
        assert!(editor.export_path.ends_with("export.avi"));
        editor.export(egui::Context::default());
        assert!(!editor.exporting);
        assert!(editor.status.contains("WebM"));
        cleanup(folder);
    }

    #[test]
    fn marker_quote_tap_and_hold_have_distinct_persistent_actions() {
        let (_ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("package.json"), r#"{"name":"test"}"#).unwrap();
        editor.marker_gesture(0.0, true, false, true, true);
        editor.marker_gesture(0.2, false, true, false, true);
        assert_eq!(editor.marker_config.as_ref().unwrap().markers("s").len(), 1);
        editor.marker_gesture(1.0, true, false, true, true);
        editor.marker_gesture(3.01, false, false, true, true);
        assert!(editor.marker_picker);
        editor.marker_gesture(3.1, false, true, false, true);
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")
                .len(),
            1
        );
        editor.marker_gesture(4.0, true, false, true, true);
        editor.marker_gesture(4.1, false, false, true, false);
        editor.marker_gesture(4.2, false, true, false, true);
        assert_eq!(editor.marker_config.as_ref().unwrap().markers("s").len(), 1);
        cleanup(folder);
    }

    #[test]
    fn individual_marker_removal_preserves_other_points_and_rejects_stale_edits() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        std::fs::write(
            folder.join("package.json"),
            r##"{"name":"test","diffusion":{"markers":{"other":[{"time":4,"color":"#ffffff"}]}}}"##,
        )
        .unwrap();
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        editor.edit_markers(false);
        editor.transport.as_mut().unwrap().seek(2.0).unwrap();
        editor.edit_markers(false);
        let before = editor.marker_config.as_ref().unwrap().markers("s");
        editor.replace_markers(&before, &before[1..]);
        let config = ad3_editor::project_config::ProjectConfig::load(&folder).unwrap();
        assert_eq!(config.markers("s"), before[1..]);
        assert_eq!(config.markers("other").len(), 1);
        editor.replace_markers(&before, &[]);
        assert!(editor.status.contains("externally"));
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s"),
            before[1..]
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn marker_click_seeks_exact_frame_without_editing_markers() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        let package = folder.join("package.json");
        std::fs::write(&package, r#"{"name":"test"}"#).unwrap();
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        editor.edit_markers(false);
        let saved = std::fs::read(&package).unwrap();
        editor.transport.as_mut().unwrap().seek(0.0).unwrap();
        frame(&ctx, &mut editor, Vec::new(), Default::default());
        let target = editor.marker_hits[0].center() + Vec2::new(4.0, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(target, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert_eq!(editor.transport.as_ref().unwrap().time, 1.5);
        assert_eq!(std::fs::read(&package).unwrap(), saved);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn marker_mouse_drag_snaps_and_saves_only_on_release() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        std::fs::write(folder.join("package.json"), r#"{"name":"test"}"#).unwrap();
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        editor.edit_markers(false);
        frame(&ctx, &mut editor, Vec::new(), Default::default());
        let from = editor.marker_hits[0].center();
        let to = from + Vec2::new(editor.zoom * 0.7, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(from, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(to)],
            Default::default(),
        );
        assert!(editor.marker_drag.is_some());
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")[0]
                .time,
            1.5
        );
        frame(
            &ctx,
            &mut editor,
            button(to, false, Default::default()),
            Default::default(),
        );
        assert!(editor.marker_drag.is_none());
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")[0]
                .time,
            2.2
        );
        frame(&ctx, &mut editor, Vec::new(), Default::default());
        let from = editor.marker_hits[0].center();
        let target = from + Vec2::new(editor.zoom * 0.5, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(from, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(target)],
            Default::default(),
        );
        assert!(editor.marker_drag.is_some());
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            Default::default(),
        );
        assert!(editor.marker_drag.is_none());
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(target + Vec2::new(10.0, 0.0))],
            Default::default(),
        );
        assert!(editor.marker_drag.is_none());
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")[0]
                .time,
            2.2
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn marker_keys_persist_toggle_and_clear_without_changing_composition() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        let package = folder.join("package.json");
        std::fs::write(
            &package,
            r#"{"name":"test","diffusion":{"export":{"s":{"format":"mov"}}}}"#,
        )
        .unwrap();
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        editor.marker_color = "#3B82F6".into();
        key(&ctx, &mut editor, egui::Key::M, Default::default());
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")[0]
                .time,
            1.5
        );
        assert_eq!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")[0]
                .color,
            "#3B82F6"
        );
        editor.marker_color = "#10B981".into();
        key(&ctx, &mut editor, egui::Key::M, Default::default());
        assert!(
            editor
                .marker_config
                .as_ref()
                .unwrap()
                .markers("s")
                .is_empty()
        );
        key(&ctx, &mut editor, egui::Key::M, Default::default());
        key(&ctx, &mut editor, egui::Key::M, egui::Modifiers::ALT);
        assert!(
            ad3_editor::project_config::ProjectConfig::load(&folder)
                .unwrap()
                .markers("s")
                .is_empty()
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(package).unwrap()).unwrap();
        assert_eq!(metadata["diffusion"]["export"]["s"]["format"], "mov");
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn playback_shortcuts_ignore_modified_shuttle_and_cut_keys() {
        let (ctx, mut editor) = editor();
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        let original = editor.snapshot.as_ref().unwrap().document.source.clone();
        let generation = editor.transport.as_ref().unwrap().generation;
        for modifiers in [
            egui::Modifiers::COMMAND,
            egui::Modifiers::SHIFT,
            egui::Modifiers::ALT,
        ] {
            for shortcut in [
                egui::Key::J,
                egui::Key::K,
                egui::Key::L,
                egui::Key::W,
                egui::Key::Q,
                egui::Key::E,
            ] {
                key(&ctx, &mut editor, shortcut, modifiers);
            }
        }
        assert_eq!(editor.transport.as_ref().unwrap().generation, generation);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, original);
        key(&ctx, &mut editor, egui::Key::L, Default::default());
        assert_eq!(editor.transport.as_ref().unwrap().speed, 2.0);
        assert!(editor.transport.as_ref().unwrap().playing);
        key(&ctx, &mut editor, egui::Key::J, Default::default());
        assert_eq!(editor.transport.as_ref().unwrap().speed, -2.0);
        key(&ctx, &mut editor, egui::Key::K, Default::default());
        assert_eq!(editor.transport.as_ref().unwrap().speed, 1.0);
        key(&ctx, &mut editor, egui::Key::K, Default::default());
        assert!(!editor.transport.as_ref().unwrap().playing);
    }

    #[test]
    fn keyboard_transport_steps_frames_and_seconds_without_editing_source() {
        let (ctx, mut editor) = editor();
        let original = editor.snapshot.as_ref().unwrap().document.source.clone();
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        let generation = editor.transport.as_ref().unwrap().generation;
        key(&ctx, &mut editor, egui::Key::D, Default::default());
        assert!((editor.transport.as_ref().unwrap().time - 46.0 / 30.0).abs() < 1e-9);
        assert!(editor.transport.as_ref().unwrap().generation > generation);
        key(&ctx, &mut editor, egui::Key::S, Default::default());
        assert!((editor.transport.as_ref().unwrap().time - 16.0 / 30.0).abs() < 1e-9);
        key(&ctx, &mut editor, egui::Key::S, Default::default());
        assert_eq!(editor.transport.as_ref().unwrap().time, 0.0);
        key(&ctx, &mut editor, egui::Key::D, egui::Modifiers::COMMAND);
        assert_eq!(editor.transport.as_ref().unwrap().time, 0.0);
        key(&ctx, &mut editor, egui::Key::D, egui::Modifiers::SHIFT);
        assert!((editor.transport.as_ref().unwrap().time - 1.0 / 30.0).abs() < 1e-9);
        key(&ctx, &mut editor, egui::Key::S, egui::Modifiers::ALT);
        assert_eq!(editor.transport.as_ref().unwrap().time, 0.0);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, original);
    }

    #[test]
    fn keyboard_parent_selection_skips_sequences_and_stops_at_stage() {
        let (ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active workarea={[0,12]}><group id="g" end={10}><sequence id="outer"><sequence id="inner"><rect id="a" end={3}/><rect id="b" end={3}/></sequence></sequence></group><group id="g2" end={10}><rect id="c" end={3}/></group></scene><rect id="loose" end={10}/></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        let original = editor.snapshot.as_ref().unwrap().document.source.clone();
        editor.selection = vec!["a".into(), "b".into(), "c".into()];
        key(&ctx, &mut editor, egui::Key::Backslash, Default::default());
        assert_eq!(editor.selection, vec!["g", "g2"]);
        key(&ctx, &mut editor, egui::Key::Backslash, Default::default());
        assert_eq!(editor.selection, vec!["s"]);
        key(&ctx, &mut editor, egui::Key::Backslash, Default::default());
        assert_eq!(editor.selection, vec!["s"]);
        editor.selection = vec!["loose".into()];
        key(&ctx, &mut editor, egui::Key::Backslash, Default::default());
        assert_eq!(editor.selection, vec!["loose"]);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, original);
    }

    #[test]
    fn keyboard_enter_selects_visible_children_through_sequences() {
        let (ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active workarea={[0,12]}><group id="g" end={10}><sequence id="track"><rect id="a" end={3}/><rect id="late" start={4} end={8}/></sequence><rect id="hidden" end={10} hidden/><group id="nested" end={10}><rect id="leaf" end={10}/></group></group></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        editor.selection = vec!["g".into(), "track".into()];
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        let original = editor.snapshot.as_ref().unwrap().document.source.clone();
        key(&ctx, &mut editor, egui::Key::Enter, Default::default());
        assert_eq!(editor.selection, vec!["a", "nested"]);
        key(&ctx, &mut editor, egui::Key::Enter, Default::default());
        assert_eq!(editor.selection, vec!["leaf"]);
        key(&ctx, &mut editor, egui::Key::Enter, Default::default());
        assert_eq!(editor.selection, vec!["leaf"]);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, original);
    }

    #[test]
    fn keyboard_forward_selection_opens_sequences_and_preserves_groups() {
        let (ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active workarea={[0,12]}><sequence id="track"><rect id="early" end={2}/><sequence id="nested" start={4}><rect id="later" end={2}/></sequence></sequence><group id="g" start={6} end={9}><rect id="inside" end={3}/></group><rect id="crossing" start={1} end={10}/><rect id="hidden" start={8} end={10} hidden/></scene><scene id="other"><rect id="outside" end={12}/></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        editor.transport.as_mut().unwrap().seek(4.0).unwrap();
        key(&ctx, &mut editor, egui::Key::A, Default::default());
        assert_eq!(editor.selection, vec!["later", "g", "hidden"]);
        editor.selection = vec!["g".into(), "later".into()];
        editor.transport.as_mut().unwrap().seek(11.0).unwrap();
        key(&ctx, &mut editor, egui::Key::A, Default::default());
        assert_eq!(editor.selection, vec!["later", "g", "hidden"]);
        editor.selection.clear();
        key(&ctx, &mut editor, egui::Key::A, Default::default());
        assert!(editor.selection.is_empty());
    }

    #[test]
    fn keyboard_selection_delete_and_undo_redo_share_one_source_transaction() {
        let (ctx, mut editor) = editor();
        let document = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active workarea={[0,8]}><rect id="a" end={2}/><rect id="b" start={1} end={4}/></scene><scene id="spare" hidden workarea={[0,8]}><rect id="spare-child" end={8}/></scene></stage>;"#.into()).unwrap();
        let timeline = Timeline::resolve(&document.root, &Default::default()).unwrap();
        editor.replace_snapshot(Snapshot {
            document,
            timeline,
            project: PathBuf::from("."),
            controls: Vec::new(),
        });
        let (folder, original) = persist(&mut editor);
        let entry = folder.join("index.tsx");
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        key(&ctx, &mut editor, egui::Key::A, command);
        assert_eq!(editor.selection, vec!["s"]);
        key(&ctx, &mut editor, egui::Key::Delete, Default::default());
        assert!(editor.selection.is_empty());
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a")
                .is_none()
        );
        let deleted = std::fs::read_to_string(&entry).unwrap();
        assert_ne!(deleted, original);
        key(&ctx, &mut editor, egui::Key::Z, command);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), original);
        editor.selection = vec!["s".into()];
        key(&ctx, &mut editor, egui::Key::Backspace, Default::default());
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), deleted);
        key(&ctx, &mut editor, egui::Key::Z, command);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), original);
        key(
            &ctx,
            &mut editor,
            egui::Key::Z,
            egui::Modifiers {
                shift: true,
                ..command
            },
        );
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), deleted);
        key(&ctx, &mut editor, egui::Key::Z, command);
        key(&ctx, &mut editor, egui::Key::Y, command);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), deleted);
        cleanup(folder);
    }

    #[test]
    fn text_field_focus_protects_document_shortcuts() {
        let (ctx, mut editor) = editor();
        let command = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        let input_id = egui::Id::new("focused-edit");
        let mut text = "Property text".to_string();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id(input_id));
            ui.ctx().memory_mut(|memory| memory.request_focus(input_id));
        });
        output.textures_delta.clear();
        assert!(ctx.egui_wants_keyboard_input());
        key(&ctx, &mut editor, egui::Key::A, command);
        assert_eq!(editor.selection, vec!["a"]);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a")
                .is_some()
        );
    }

    #[test]
    fn desktop_playhead_cut_saves_one_transaction_and_undo_redo_restore_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        let entry = folder.join("index.tsx");
        editor.transport.as_mut().unwrap().seek(1.5).unwrap();
        key(&ctx, &mut editor, egui::Key::B, egui::Modifiers::COMMAND);
        assert_eq!(editor.selection, vec!["a", "b"]);
        let cut = std::fs::read_to_string(&entry).unwrap();
        assert_ne!(cut, original);
        let snapshot = editor.snapshot.as_ref().unwrap();
        for id in ["a", "b"] {
            assert_eq!(snapshot.timeline.get(id).unwrap().end, 1.5);
            assert_eq!(
                snapshot.timeline.get(&format!("{id}-2")).unwrap().start,
                1.5
            );
        }
        editor.history(false);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), original);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a-2")
                .is_none()
        );
        editor.history(true);
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), cut);
        cleanup(folder);
    }

    #[test]
    fn escape_deselects_and_closes_pending_marker_gestures() {
        let (ctx, mut editor) = editor();
        editor.selection = vec!["a".into(), "b".into()];
        editor.selected = "a".into();
        editor.marker_picker = true;
        editor.marker_hold = Some(("scene".into(), 0.0));
        editor.marker_drag = Some((0, 1.0));
        editor.gap_selection = Some(("track".into(), [1.0, 2.0]));
        key(&ctx, &mut editor, egui::Key::Escape, Default::default());
        assert!(editor.selection.is_empty());
        assert!(editor.selected.is_empty());
        assert!(!editor.marker_picker);
        assert!(editor.marker_hold.is_none());
        assert!(editor.marker_drag.is_none());
        assert!(editor.gap_selection.is_none());
    }

    #[test]
    fn sequence_wrap_shortcut_resolves_overlap_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        key(
            &ctx,
            &mut editor,
            egui::Key::Enter,
            egui::Modifiers {
                command: true,
                alt: true,
                ..Default::default()
            },
        );
        assert_eq!(editor.selection, vec!["sequence-1"]);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .timeline
                .get("b")
                .unwrap()
                .start,
            2.0
        );
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .timeline
                .get("b")
                .unwrap()
                .end,
            4.0
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn preview_mode_switch_changes_worker_resolution() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("index.tsx"),r#"export default <stage><scene id="s" active width={64} height={48} workarea={[0,2]}><rect width={32} height={24} fill="red"/></scene></stage>;"#).unwrap();
        editor.replace_snapshot(Snapshot::load(&folder, &Tools::default()).unwrap());
        for (shortcut, height) in [(egui::Key::Num0, 48), (egui::Key::Num1, 360)] {
            key(&ctx, &mut editor, shortcut, egui::Modifiers::COMMAND);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                editor.poll(&ctx);
                if editor
                    .texture
                    .as_ref()
                    .is_some_and(|texture| texture.size()[1] == height)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "preview frame did not reach height {height}: {}",
                    editor.status
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        cleanup(folder);
    }

    #[test]
    fn timeline_zoom_shortcuts_scale_and_clamp_without_source_changes() {
        let (ctx, mut editor) = editor();
        let source = editor.snapshot.as_ref().unwrap().document.source.clone();
        key(&ctx, &mut editor, egui::Key::Num0, egui::Modifiers::COMMAND);
        assert!(editor.preview_actual_size);
        key(&ctx, &mut editor, egui::Key::Num1, egui::Modifiers::COMMAND);
        assert!(!editor.preview_actual_size);
        key(
            &ctx,
            &mut editor,
            egui::Key::Equals,
            egui::Modifiers::COMMAND,
        );
        assert_eq!(editor.zoom, 100.0);
        key(&ctx, &mut editor, egui::Key::Plus, egui::Modifiers::COMMAND);
        assert_eq!(editor.zoom, 125.0);
        key(
            &ctx,
            &mut editor,
            egui::Key::Minus,
            egui::Modifiers::COMMAND,
        );
        assert_eq!(editor.zoom, 100.0);
        editor.zoom = 399.0;
        key(
            &ctx,
            &mut editor,
            egui::Key::Equals,
            egui::Modifiers::COMMAND,
        );
        assert_eq!(editor.zoom, 400.0);
        editor.zoom = 10.0;
        key(
            &ctx,
            &mut editor,
            egui::Key::Minus,
            egui::Modifiers::COMMAND,
        );
        assert_eq!(editor.zoom, 10.0);
        assert_eq!(editor.snapshot.as_ref().unwrap().document.source, source);
    }

    #[test]
    fn clipboard_native_events_and_project_reload_follow_session_scope() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            |ui| editor.edit_shortcuts(ui.ctx()),
        );
        output.textures_delta.clear();
        assert!(editor.clipboard.is_some());
        editor.selection = vec!["b".into()];
        editor.selected = "b".into();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste(String::new())],
                ..Default::default()
            },
            |ui| editor.edit_shortcuts(ui.ctx()),
        );
        output.textures_delta.clear();
        assert_eq!(editor.selection, vec!["a-2"]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("s")
                .unwrap()
                .children
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "a-2"]
        );
        let same = (**snapshot).clone();
        editor
            .event_sender
            .send(Event::Opened(Box::new(Ok(same.clone()))))
            .unwrap();
        editor.poll(&ctx);
        assert!(editor.clipboard.is_some());
        let mut other = same;
        other.project = folder.join("other-project");
        editor
            .event_sender
            .send(Event::Opened(Box::new(Ok(other))))
            .unwrap();
        editor.poll(&ctx);
        assert!(editor.clipboard.is_none());
        cleanup(folder);
    }

    #[test]
    fn clipboard_shortcuts_cut_paste_select_and_undo_without_losing_snapshot() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        let selected = editor.selection[0].clone();
        key(&ctx, &mut editor, egui::Key::X, egui::Modifiers::COMMAND);
        assert!(editor.clipboard.is_some());
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find(&selected)
                .is_none()
        );
        let cut_source = editor.snapshot.as_ref().unwrap().document.source.clone();
        editor.selection.clear();
        editor.selected.clear();
        key(&ctx, &mut editor, egui::Key::V, egui::Modifiers::COMMAND);
        assert_eq!(editor.selection.len(), 1);
        assert_ne!(editor.selected, selected);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find(&editor.selected)
                .is_some()
        );
        editor.history(false);
        assert_eq!(
            editor.snapshot.as_ref().unwrap().document.source,
            cut_source
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        assert!(editor.clipboard.is_some());
        cleanup(folder);
    }

    #[test]
    fn wrap_scene_shortcut_measures_selection_saves_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={64} height={48} workarea={[0,4]}><rect id="a" x={8} y={6} width={12} height={10} fill="red"/></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        key(
            &ctx,
            &mut editor,
            egui::Key::Enter,
            egui::Modifiers::COMMAND,
        );
        assert!(editor.pending_wrap_scene.is_some());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, Tools::default(), false).unwrap();
        renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        editor.frame_geometry = Some((0.0, renderer.edit_geometries("s").unwrap().clone()));
        key(
            &ctx,
            &mut editor,
            egui::Key::Enter,
            egui::Modifiers::COMMAND,
        );
        let document = &editor.snapshot.as_ref().unwrap().document;
        let scene = document.root.find(&editor.selected).unwrap();
        assert_eq!(scene.tag, "scene");
        assert_eq!(scene.number("width", 0.0).unwrap(), 12.0);
        assert_eq!(
            document.root.find("a").unwrap().number("x", 0.0).unwrap(),
            0.0
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            document.source
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("scene-1")
                .unwrap()
                .tag,
            "scene"
        );
        cleanup(folder);
    }

    #[test]
    fn unwrap_sequence_shortcut_keeps_other_selected_containers() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={64} height={48} workarea={[0,4]}><sequence id="seq"><rect id="a" end={1}/><rect id="b" start={1} end={2}/></sequence><group id="g"><rect id="c"/></group></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["seq".into(), "g".into()];
        editor.selected = "seq".into();
        let modifiers = egui::Modifiers {
            command: true,
            alt: true,
            shift: true,
            ..Default::default()
        };
        key(&ctx, &mut editor, egui::Key::Enter, modifiers);
        assert_eq!(editor.pending_ungroup.as_ref().unwrap().1, true);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, Tools::default(), false).unwrap();
        renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        editor.frame_geometry = Some((0.0, renderer.edit_geometries("s").unwrap().clone()));
        let (selection, sequences_only) = editor.pending_ungroup.take().unwrap();
        assert_eq!(selection, editor.selection);
        editor.dissolve_selection(sequences_only);
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert!(document.root.find("seq").is_none());
        assert!(document.root.find("g").is_some());
        assert_eq!(editor.selection, vec!["a", "b"]);
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .timeline
                .get("b")
                .unwrap()
                .start,
            1.0
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        cleanup(folder);
    }

    #[test]
    fn nested_scene_ungroup_shortcut_saves_and_undo_restores_container() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={64} height={48} workarea={[0,4]}><scene id="inner" x={8} y={4} width={40} height={32}><rect id="a" x={4} y={4} width={12} height={10} fill="red"/></scene></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["inner".into()];
        editor.selected = "inner".into();
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, Tools::default(), false).unwrap();
        renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        editor.frame_geometry = Some((0.0, renderer.edit_geometries("s").unwrap().clone()));
        key(
            &ctx,
            &mut editor,
            egui::Key::G,
            egui::Modifiers {
                command: true,
                shift: true,
                ..Default::default()
            },
        );
        assert_eq!(editor.selection, vec!["a"]);
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert!(document.root.find("inner").is_none());
        assert_eq!(
            document.root.find("a").unwrap().number("x", 0.0).unwrap(),
            12.0
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            document.source
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("inner")
                .is_none()
        );
        cleanup(folder);
    }

    #[test]
    fn ungroup_shortcut_bakes_skew_selects_children_and_undo_redo_restores_state() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        let source = r#"export default <stage><scene id="s" active width={64} height={48} workarea={[0,4]}><group id="g" scaleX={2} scaleY={1}><rect id="a" width={12} height={10} rotation={30} fill="red"/></group></scene></stage>;"#;
        std::fs::write(folder.join("index.tsx"), source).unwrap();
        let snapshot = Snapshot::load(&folder, &Tools::default()).unwrap();
        editor.history = Some(History::new(snapshot.document.clone()));
        editor.replace_snapshot(snapshot);
        editor.selection = vec!["g".into()];
        editor.selected = "g".into();
        editor.ungroup_selection();
        assert!(editor.pending_ungroup.is_some());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        let snapshot = editor.snapshot.as_ref().unwrap();
        let mut renderer =
            ProjectRenderer::new(&snapshot.document, &folder, Tools::default(), false).unwrap();
        for _ in 0..2 {
            renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
        }
        editor.frame_geometry = Some((0.0, renderer.edit_geometries("s").unwrap().clone()));
        key(
            &ctx,
            &mut editor,
            egui::Key::G,
            egui::Modifiers {
                command: true,
                shift: true,
                ..Default::default()
            },
        );
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("g")
                .is_none()
        );
        assert_eq!(editor.selection, vec!["a"]);
        assert!(editor.runtime_skews["a"][0].abs() > 1.0);
        let skew = editor.runtime_skews.clone();
        editor.history(false);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("g")
                .is_some()
        );
        assert!(editor.runtime_skews.is_empty());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            source
        );
        editor.history(true);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("g")
                .is_none()
        );
        assert_eq!(editor.runtime_skews, skew);
        cleanup(folder);
    }

    #[test]
    fn transient_skew_follows_successful_edits_undo_and_redo() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        let next =
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .set("a", "x", Some(serde_json::json!(10)));
        assert!(editor.commit_with_skews(next, BTreeMap::from([("a".into(), [30.0, 0.0])])));
        let baked = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        assert_eq!(editor.runtime_skews["a"], [30.0, 0.0]);
        editor.history(false);
        assert!(editor.runtime_skews.is_empty());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        editor.history(true);
        assert_eq!(editor.runtime_skews["a"], [30.0, 0.0]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            baked
        );
        let next = editor.snapshot.as_ref().unwrap().document.remove("a");
        assert!(editor.commit(next));
        assert!(editor.runtime_skews.is_empty());
        editor.history(false);
        assert_eq!(editor.runtime_skews["a"], [30.0, 0.0]);
        let next =
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .set("a", "x", Some(serde_json::json!(11)));
        assert!(!editor.commit_with_skews(next, BTreeMap::from([("a".into(), [f64::NAN, 0.0])])));
        assert_eq!(editor.runtime_skews["a"], [30.0, 0.0]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            baked
        );
        cleanup(folder);
    }

    #[test]
    fn selected_keyframes_drag_together_and_undo_as_one_edit() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.property = "x".into();
        editor.value = "16".into();
        for at in [0.5, 1.0] {
            editor.transport.as_mut().unwrap().seek(at).unwrap();
            editor.toggle_selection_keyframe();
        }
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        frame(&ctx, &mut editor, vec![], Default::default());
        editor.selected_keyframes = editor
            .keyframe_hits
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let ids = editor.selected_keyframes.clone();
        let from = editor.keyframe_hits[0].1.center();
        let to = from + Vec2::new(editor.zoom * 0.5, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(from, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(to)],
            Default::default(),
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        frame(
            &ctx,
            &mut editor,
            button(to, false, Default::default()),
            Default::default(),
        );
        for (id, at) in ids.iter().zip([1.0, 1.5]) {
            assert_eq!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .number("time", 0.0)
                    .unwrap(),
                at
            );
        }
        assert_eq!(editor.selected_keyframes, ids);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        cleanup(folder);
    }

    #[test]
    fn keyframe_multiselection_toggles_and_deletes_in_one_undo() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.property = "x".into();
        editor.value = "16".into();
        for at in [1.0, 2.0] {
            editor.transport.as_mut().unwrap().seek(at).unwrap();
            editor.toggle_selection_keyframe();
        }
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        frame(&ctx, &mut editor, vec![], Default::default());
        let first = editor.keyframe_hits[0].1.center();
        let second = editor.keyframe_hits[1].1.center();
        for (point, modifiers) in [
            (first, egui::Modifiers::default()),
            (second, egui::Modifiers::SHIFT),
        ] {
            frame(&ctx, &mut editor, button(point, true, modifiers), modifiers);
            frame(
                &ctx,
                &mut editor,
                button(point, false, modifiers),
                modifiers,
            );
        }
        assert_eq!(editor.selected_keyframes.len(), 2);
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut editor,
                button(second, pressed, egui::Modifiers::SHIFT),
                egui::Modifiers::SHIFT,
            );
        }
        assert_eq!(editor.selected_keyframes.len(), 1);
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut editor,
                button(second, pressed, egui::Modifiers::COMMAND),
                egui::Modifiers::COMMAND,
            );
        }
        assert_eq!(editor.selected_keyframes.len(), 2);
        key(&ctx, &mut editor, egui::Key::Delete, Default::default());
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert!(document.root.find("a").is_some());
        assert!(
            !document
                .root
                .nodes()
                .iter()
                .any(|node| node.tag == "keyframeTrack")
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        cleanup(folder);
    }

    #[test]
    fn nested_paint_keyframe_click_selects_paint_and_uses_clip_clock() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        std::fs::write(folder.join("index.tsx"), r##"export default <stage><scene id="s" active workarea={[0,5]}><rect id="a" width={16} height={16} start={1} sourceIn={2} playbackRate={2} end={3}><solidPaint id="fill" color="#ff0000"><keyframeTrack property="color"><keyframe id="p" time={3} value="#00ff00"/></keyframeTrack></solidPaint></rect></scene></stage>;"##).unwrap();
        editor.replace_snapshot(Snapshot::load(&folder, &Tools::default()).unwrap());
        frame(&ctx, &mut editor, vec![], Default::default());
        let target = editor
            .keyframe_hits
            .iter()
            .find(|(id, _)| id == "p")
            .unwrap()
            .1
            .center();
        frame(
            &ctx,
            &mut editor,
            button(target, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert_eq!(editor.selection, vec!["fill"]);
        assert_eq!(editor.property, "color");
        assert_eq!(editor.transport.as_ref().unwrap().time, 1.5);
        assert_eq!(editor.value, "\"#00ff00\"");
        cleanup(folder);
    }

    #[test]
    fn timeline_keyframe_drag_saves_on_release_and_escape_cancels() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.property = "x".into();
        editor.value = "16".into();
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        editor.toggle_selection_keyframe();
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        frame(&ctx, &mut editor, vec![], Default::default());
        let (id, hit) = editor.keyframe_hits[0].clone();
        let from = hit.center();
        let to = from + Vec2::new(editor.zoom * 0.7, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(from, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(to)],
            Default::default(),
        );
        assert!(editor.keyframe_drag.is_some());
        assert!(editor.drag_start.is_none());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        frame(
            &ctx,
            &mut editor,
            button(to, false, Default::default()),
            Default::default(),
        );
        assert_eq!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find(&id)
                .unwrap()
                .number("time", 0.0)
                .unwrap(),
            1.7
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        frame(&ctx, &mut editor, vec![], Default::default());
        let from = editor.keyframe_hits[0].1.center();
        let to = from + Vec2::new(editor.zoom * 0.5, 0.0);
        frame(
            &ctx,
            &mut editor,
            button(from, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(to)],
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            Default::default(),
        );
        assert!(editor.keyframe_drag.is_none());
        frame(
            &ctx,
            &mut editor,
            button(to, false, Default::default()),
            Default::default(),
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        cleanup(folder);
    }

    #[test]
    fn timeline_keyframe_click_seeks_and_loads_property_without_editing() {
        let (ctx, mut editor) = editor();
        let (folder, _) = persist(&mut editor);
        editor.selection = vec!["a".into()];
        editor.selected = "a".into();
        editor.property = "x".into();
        editor.value = "16".into();
        editor.transport.as_mut().unwrap().seek(1.0).unwrap();
        editor.toggle_selection_keyframe();
        let saved = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.transport.as_mut().unwrap().seek(0.0).unwrap();
        editor.selection.clear();
        editor.selected.clear();
        editor.property = "y".into();
        frame(&ctx, &mut editor, vec![], Default::default());
        let target = editor.keyframe_hits[0].1.center();
        frame(
            &ctx,
            &mut editor,
            button(target, true, Default::default()),
            Default::default(),
        );
        assert!(editor.marquee.is_none());
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        assert_eq!(editor.selection, vec!["a"]);
        assert_eq!(editor.property, "x");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&editor.value)
                .unwrap()
                .as_f64(),
            Some(16.0)
        );
        assert_eq!(editor.transport.as_ref().unwrap().time, 1.0);
        assert!(editor.drag_start.is_none());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        let empty = Pos2::new(950.0, editor.hit_regions[0].1.center().y);
        frame(
            &ctx,
            &mut editor,
            button(empty, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(empty, false, Default::default()),
            Default::default(),
        );
        assert!(editor.selected_keyframes.is_empty());
        assert!(editor.selection.is_empty());
        key(&ctx, &mut editor, egui::Key::Delete, Default::default());
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        frame(
            &ctx,
            &mut editor,
            button(target, true, Default::default()),
            Default::default(),
        );
        frame(
            &ctx,
            &mut editor,
            button(target, false, Default::default()),
            Default::default(),
        );
        key(&ctx, &mut editor, egui::Key::Delete, Default::default());
        let document = &editor.snapshot.as_ref().unwrap().document;
        assert!(document.root.find("a").is_some());
        assert!(
            !document
                .root
                .nodes()
                .iter()
                .any(|node| node.tag == "keyframeTrack")
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            saved
        );
        cleanup(folder);
    }

    #[test]
    fn inspector_remove_animation_is_one_undoable_batch() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        editor.property = "x".into();
        editor.value = "16".into();
        editor.toggle_selection_keyframe();
        let animated = std::fs::read_to_string(folder.join("index.tsx")).unwrap();
        editor.remove_selection_keyframe_tracks();
        assert!(
            !editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .nodes()
                .iter()
                .any(|node| node.tag == "keyframeTrack")
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            animated
        );
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn inspector_keyframe_toggle_saves_one_transaction_and_undoes() {
        let (_ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        editor.property = "x".into();
        editor.value = "16".into();
        editor.toggle_selection_keyframe();
        for id in ["a", "b"] {
            assert!(
                editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .children
                    .iter()
                    .any(|child| child.tag == "keyframeTrack")
            );
        }
        editor.toggle_selection_keyframe();
        for id in ["a", "b"] {
            assert!(
                !editor
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .document
                    .root
                    .find(id)
                    .unwrap()
                    .children
                    .iter()
                    .any(|child| child.tag == "keyframeTrack")
            );
        }
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn arrow_shortcuts_nudge_selection_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        key(&ctx, &mut editor, egui::Key::ArrowRight, Default::default());
        key(
            &ctx,
            &mut editor,
            egui::Key::ArrowUp,
            egui::Modifiers::SHIFT,
        );
        for id in ["a", "b"] {
            let node = editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find(id)
                .unwrap();
            assert_eq!(node.number("x", 0.0).unwrap(), 1.0);
            assert_eq!(node.number("y", 0.0).unwrap(), -10.0);
        }
        assert_eq!(editor.selection, vec!["a", "b"]);
        editor.history(false);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn grouping_shortcut_selects_group_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        key(&ctx, &mut editor, egui::Key::G, egui::Modifiers::COMMAND);
        assert_eq!(editor.selection, vec!["group-1"]);
        let snapshot = editor.snapshot.as_ref().unwrap();
        assert_eq!(
            snapshot
                .document
                .root
                .find("group-1")
                .unwrap()
                .children
                .len(),
            2
        );
        assert_eq!(snapshot.timeline.get("a").unwrap().start, 0.0);
        editor.history(false);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }

    #[test]
    fn duplicate_shortcut_selects_copies_and_undo_restores_source() {
        let (ctx, mut editor) = editor();
        let (folder, original) = persist(&mut editor);
        editor.selection = vec!["a".into(), "b".into()];
        editor.runtime_skews.insert("a".into(), [20.0, -10.0]);
        key(&ctx, &mut editor, egui::Key::D, egui::Modifiers::COMMAND);
        assert_eq!(editor.selection, vec!["a-2", "b-2"]);
        assert_eq!(editor.runtime_skews["a-2"], [20.0, -10.0]);
        assert!(
            editor
                .snapshot
                .as_ref()
                .unwrap()
                .document
                .root
                .find("a-2")
                .is_some()
        );
        editor.history(false);
        assert!(!editor.runtime_skews.contains_key("a-2"));
        assert_eq!(editor.runtime_skews["a"], [20.0, -10.0]);
        assert_eq!(
            std::fs::read_to_string(folder.join("index.tsx")).unwrap(),
            original
        );
        cleanup(folder);
    }
}

fn main() -> eframe::Result {
    let path = std::env::args().nth(1).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "AD3 Editing — Rust",
        options,
        Box::new(move |context| Ok(Box::new(Editor::new(context, path)))),
    )
}

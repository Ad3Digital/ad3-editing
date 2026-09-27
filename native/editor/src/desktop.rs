// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use ad3_editor::{
    Result,
    media::{MediaLibrary, Tools},
    render::RenderPlan,
    source::{History, SourceDocument},
    timing::Timeline,
    transport::Transport,
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Snapshot {
    document: SourceDocument,
    library: MediaLibrary,
    timeline: Timeline,
    project: PathBuf,
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
        let timeline = Timeline::resolve(&document.root, &library.durations)?;
        Ok(Self {
            document,
            library,
            timeline,
            project,
        })
    }
}
struct FrameRequest {
    snapshot: Arc<Snapshot>,
    generation: u64,
    time: f64,
}
struct FrameReply {
    generation: u64,
    result: Result<egui::ColorImage>,
}
enum Event {
    Opened(Box<Result<Snapshot>>),
    Exported(Result<PathBuf>),
}

#[cfg(debug_assertions)]
struct Smoke {
    output: PathBuf,
    started: Instant,
    requested: bool,
}

struct Editor {
    project_path: String,
    export_path: String,
    snapshot: Option<Arc<Snapshot>>,
    history: Option<History>,
    transport: Option<Transport>,
    selected: String,
    property: String,
    value: String,
    status: String,
    texture: Option<egui::TextureHandle>,
    frames: mpsc::Receiver<FrameReply>,
    request: mpsc::SyncSender<FrameRequest>,
    events: mpsc::Receiver<Event>,
    event_sender: mpsc::Sender<Event>,
    busy: bool,
    loading: bool,
    exporting: bool,
    export_cancel: Arc<AtomicBool>,
    export_job: Option<thread::JoinHandle<()>>,
    dirty_preview: bool,
    generation: u64,
    last_tick: Instant,
    zoom: f32,
    drag_start: Option<f64>,
    #[cfg(debug_assertions)]
    smoke: Option<Smoke>,
}

impl Editor {
    fn new(context: &eframe::CreationContext<'_>, path: String) -> Self {
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = mpsc::sync_channel::<FrameRequest>(1);
        let (result_tx, result_rx) = mpsc::channel();
        let repaint = context.egui_ctx.clone();
        thread::spawn(move || {
            let tools = Tools::default();
            while let Ok(request) = rx.recv() {
                let snapshot = &request.snapshot;
                let result = (|| {
                    let mut plan = RenderPlan::build(
                        &snapshot.document.root,
                        &snapshot.timeline,
                        &snapshot.library,
                        &snapshot.timeline.active,
                        30.0,
                        Some([request.time, request.time + 1.0 / 30.0]),
                    )?;
                    plan.resize_output(360)?;
                    let bytes = plan.frame(&tools, &snapshot.project)?;
                    Ok(egui::ColorImage::from_rgba_unmultiplied(
                        [plan.width as usize, plan.height as usize],
                        &bytes,
                    ))
                })();
                if result_tx
                    .send(FrameReply {
                        generation: request.generation,
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
            snapshot: None,
            history: None,
            transport: None,
            selected: String::new(),
            property: "volume".into(),
            value: "0".into(),
            status: "Abra a pasta de um projeto AD3.".into(),
            texture: None,
            frames: result_rx,
            request: tx,
            events,
            event_sender,
            busy: false,
            loading: false,
            exporting: false,
            export_cancel: Arc::new(AtomicBool::new(false)),
            export_job: None,
            dirty_preview: false,
            generation: 0,
            last_tick: Instant::now(),
            zoom: 80.0,
            drag_start: None,
            #[cfg(debug_assertions)]
            smoke: std::env::var_os("AD3_NATIVE_SMOKE_SCREENSHOT").map(|path| Smoke {
                output: path.into(),
                started: Instant::now(),
                requested: false,
            }),
        };
        if !editor.project_path.is_empty() {
            editor.open(context.egui_ctx.clone());
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
        self.generation += 1;
        self.dirty_preview = true;
    }

    fn replace_snapshot(&mut self, snapshot: Snapshot) {
        let same_scene = self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.timeline.active == snapshot.timeline.active);
        let previous = self.transport.take();
        self.transport = snapshot
            .timeline
            .workareas
            .get(&snapshot.timeline.active)
            .and_then(|area| Transport::new(*area).ok());
        if same_scene && let (Some(previous), Some(current)) = (previous, &mut self.transport) {
            let _ = current.seek(
                previous
                    .time
                    .clamp(current.workarea[0], current.workarea[1]),
            );
            current.looping = previous.looping;
        }
        self.snapshot = Some(Arc::new(snapshot));
    }

    fn commit(&mut self, next: Result<SourceDocument>) {
        let result = (|| {
            let next = next?;
            let previous = self.snapshot.as_ref().ok_or("No project")?;
            let snapshot =
                Snapshot::from_document(next.clone(), previous.project.clone(), &Tools::default())?;
            self.history.as_mut().ok_or("No history")?.commit(next)?;
            self.replace_snapshot(snapshot);
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.status = "Salvo no projeto.".into();
                self.invalidate();
            }
            Err(error) => self.status = error,
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
            let next = snapshot
                .document
                .split(&snapshot.timeline, &self.selected, transport.time);
            self.commit(next);
        }
    }

    fn export(&mut self, context: egui::Context) {
        if self.exporting {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let destination = PathBuf::from(&self.export_path);
        let tx = self.event_sender.clone();
        self.exporting = true;
        self.export_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.export_cancel.clone();
        self.status = "Exportando…".into();
        self.export_job = Some(thread::spawn(move || {
            let result = (|| {
                let plan = RenderPlan::build(
                    &snapshot.document.root,
                    &snapshot.timeline,
                    &snapshot.library,
                    &snapshot.timeline.active,
                    30.0,
                    None,
                )?;
                plan.export_cancellable(
                    &Tools::default(),
                    &snapshot.project,
                    &destination,
                    Some(&cancel),
                )?;
                Ok(destination)
            })();
            let _ = tx.send(Event::Exported(result));
            context.request_repaint();
        }));
    }

    fn poll(&mut self, context: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Opened(result) => {
                    self.loading = false;
                    match *result {
                        Ok(snapshot) => {
                            self.history = Some(History::new(snapshot.document.clone()));
                            self.export_path = snapshot
                                .project
                                .join("export.mp4")
                                .to_string_lossy()
                                .into_owned();
                            self.replace_snapshot(snapshot);
                            self.selected.clear();
                            self.texture = None;
                            self.status = if self
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
                    Ok(image) => {
                        self.texture = Some(context.load_texture(
                            "scene-preview",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                    Err(error) => {
                        self.texture = None;
                        self.status = error;
                        if let Some(t) = &mut self.transport {
                            t.playing = false;
                        }
                    }
                }
            }
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if let Some(transport) = &mut self.transport
            && transport.playing
        {
            let _ = transport.advance(elapsed);
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
                })
                .is_ok()
            {
                self.busy = true;
                self.dirty_preview = false;
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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
        });
        ui.horizontal(|ui| {
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
            ui.add(
                egui::TextEdit::singleline(&mut self.export_path)
                    .hint_text("Arquivo de exportação.mp4")
                    .desired_width(230.0),
            );
            if ui
                .add_enabled(
                    !self.exporting && self.snapshot.is_some(),
                    egui::Button::new("Exportar"),
                )
                .clicked()
            {
                self.export(ui.ctx().clone());
            }
            if self.exporting && ui.button("Cancelar exportação").clicked() {
                self.export_cancel.store(true, Ordering::Relaxed);
            }
        });
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
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
        });
        egui::ScrollArea::both()
            .id_salt("timeline-scroll")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ((end as f32 + 2.0) * self.zoom + 160.0).max(ui.available_width());
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click_and_drag());
                let origin = rect.left() + 155.0;
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
                if (response.clicked() || response.dragged())
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
                    let clip = Rect::from_min_max(
                        Pos2::new(origin + span.start as f32 * self.zoom, row.top() + 3.0),
                        Pos2::new(origin + span.end as f32 * self.zoom, row.bottom() - 3.0),
                    );
                    let response =
                        ui.interact(clip, ui.id().with(&node.id), Sense::click_and_drag());
                    let selected = self.selected == node.id;
                    let color = if selected {
                        Color32::from_rgb(30, 142, 128)
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
                        self.selected = node.id.clone();
                    }
                    if response.drag_started() {
                        self.selected = node.id.clone();
                        self.drag_start = Some(span.start);
                    }
                    if response.drag_stopped()
                        && let Some(start) = self.drag_start.take()
                    {
                        self.commit(snapshot.document.move_clip(
                            &snapshot.timeline,
                            &node.id,
                            start + f64::from(response.drag_delta().x / self.zoom),
                        ));
                    }
                }
            });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
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
        let Some(node) = snapshot.document.root.find(&self.selected) else {
            ui.label("Selecione um clipe.");
            return;
        };
        ui.label(format!("{} · {}", node.id, node.tag));
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
        ui.label("Valor JSON");
        ui.text_edit_singleline(&mut self.value);
        if ui.button("Aplicar").clicked() {
            let result = serde_json::from_str(&self.value)
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    snapshot
                        .document
                        .set(&self.selected, &self.property, Some(value))
                });
            self.commit(result);
        }
        if ui.button("Remover propriedade").clicked() {
            self.commit(snapshot.document.set(&self.selected, &self.property, None));
        }
        if ui.button("Excluir clipe").clicked() {
            self.commit(snapshot.document.remove(&self.selected));
        }
    }
}

impl eframe::App for Editor {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.export_cancel.store(true, Ordering::Relaxed);
        if let Some(job) = self.export_job.take() {
            let _ = job.join();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
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
            let keys = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::J),
                    i.key_pressed(egui::Key::K),
                    i.key_pressed(egui::Key::L),
                    i.key_pressed(egui::Key::W),
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
            .show(ui, |ui| self.inspector(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(texture) = &self.texture {
                    let available = ui.available_size();
                    let size = texture.size_vec2();
                    let scale = (available.x / size.x).min(available.y / size.y).min(2.0);
                    ui.image((texture.id(), size * scale));
                } else {
                    ui.label(if self.busy {
                        "Preparando prévia…"
                    } else {
                        "Abra um projeto para visualizar a cena."
                    });
                }
            });
        });
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

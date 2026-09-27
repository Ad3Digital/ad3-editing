// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native CPU composition. Preview, capture and export share the draw list;
//! FFmpeg supplies decoded video pixels, while Rust draws and combines them.
use crate::{
    Result,
    document::Node,
    encoder::cancelled,
    geometry::{Frame, GroupChild, Mat2D, TransformValues, group_frame, is_culled},
    media::MediaLibrary,
    paints::{self, Gradient, LineStyle},
    texture::{Fit, TextureFrame, TextureRequest, TextureStore},
    timing::{Timeline, round_frame},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, Mask, Paint, Path, PathBuilder, Pattern, Pixmap,
    Rect, SpreadMode, Stroke, Transform,
};

#[derive(Debug)]
pub(crate) struct FrameError {
    pub message: String,
    pub transient: bool,
}
impl FrameError {
    fn transient(message: String) -> Self {
        Self {
            message,
            transient: true,
        }
    }
}
impl From<String> for FrameError {
    fn from(message: String) -> Self {
        Self {
            message,
            transient: false,
        }
    }
}
impl From<&str> for FrameError {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

pub(crate) struct MediaContext<'a> {
    pub library: &'a MediaLibrary,
    pub store: &'a Arc<TextureStore>,
}

#[derive(Clone, Copy, Default)]
struct GeometryState {
    local: Mat2D,
    frame: Frame,
    text: Option<TextGeometry>,
}

#[derive(Clone, Copy)]
struct TextGeometry {
    authored: [Option<f64>; 2],
    size: Option<[f64; 2]>,
}

#[derive(Default)]
pub struct Compositor {
    scene: String,
    previous: BTreeMap<String, GeometryState>,
}

struct Layer<'a> {
    node: &'a Node,
    parent: Option<usize>,
    children: Vec<usize>,
    values: TransformValues,
    state: GeometryState,
    world: Mat2D,
    visible: bool,
    opacity: f64,
    culled: bool,
    blend: BlendMode,
    line: LineStyle,
}

struct Draw {
    path: Arc<Path>,
    world: Mat2D,
    ink: Ink,
    opacity: f64,
    clipped: bool,
    blend: BlendMode,
    stroke: Option<Stroke>,
}
enum Ink {
    Solid(u32),
    Gradient {
        gradient: Arc<Gradient>,
        origin: [f64; 2],
    },
    Media {
        request: Box<TextureRequest>,
        store: Arc<TextureStore>,
        size: [f64; 2],
        fit: Fit,
    },
}

pub struct NativeFrame {
    pub width: u32,
    pub height: u32,
    canvas: [f64; 2],
    resolution: f64,
    background: Option<u32>,
    // A scene clips descendants to its own (possibly rounded) path.
    clip: Option<Path>,
    draws: Vec<Draw>,
}

pub(crate) fn dimension(value: f64) -> Result<u32> {
    if !value.is_finite() || !(1.0..=8192.0).contains(&value) || value.fract() != 0.0 {
        return Err("Render dimensions must be integers from 1 to 8192".into());
    }
    Ok(value as u32)
}

pub(crate) fn packed_color(value: &serde_json::Value) -> Result<u32> {
    match value {
        serde_json::Value::String(text) => crate::color::parse(text),
        serde_json::Value::Number(number) => number.as_f64().and_then(crate::color::number),
        _ => None,
    }
    .ok_or_else(|| format!("Invalid color: {value}"))
}

fn alpha(parent: f64, own: f64) -> f64 {
    let requested = parent * own;
    // Canvas ignores out-of-range globalAlpha assignments.
    if (0.0..=1.0).contains(&requested) {
        requested
    } else {
        parent
    }
}

fn validate(node: &Node, root: bool) -> Result<()> {
    if node.tag == "scene" && !root {
        return Err(format!(
            "{}: nested scene clipping is not ported yet",
            node.id
        ));
    }
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
        "rotation",
        "scale",
        "scaleX",
        "scaleY",
        "opacity",
        "blendMode",
        "selected",
        "expanded",
        "clipHeight",
        "clipColor",
        "keepAspectRatio",
    ];
    let specific: &[&str] = match node.tag.as_str() {
        "scene" => &[
            "active",
            "workarea",
            "fill",
            "color",
            "cornerRadius",
            "cornerRadiusTopLeft",
            "cornerRadiusTopRight",
            "cornerRadiusBottomRight",
            "cornerRadiusBottomLeft",
        ],
        "rect" => &[
            "fill",
            "color",
            "cornerRadius",
            "cornerRadiusTopLeft",
            "cornerRadiusTopRight",
            "cornerRadiusBottomRight",
            "cornerRadiusBottomLeft",
        ],
        "audio" => &["src"],
        "text" | "textRange" => &[
            "fill",
            "color",
            "fontFamily",
            "fontSize",
            "fontWeight",
            "fontStyle",
            "letterSpacing",
            "textCase",
            "textAlign",
            "textBaseline",
            "leading",
        ],
        "image" | "video" => &[
            "src",
            "objectFit",
            "fill",
            "color",
            "cornerRadius",
            "cornerRadiusTopLeft",
            "cornerRadiusTopRight",
            "cornerRadiusBottomRight",
            "cornerRadiusBottomLeft",
        ],
        "group" | "sequence" => &[],
        "solidPaint" => &["color", "fill"],
        "linearGradientPaint" | "radialGradientPaint" => &[],
        "colorStop" => &["color", "fill", "offset"],
        "stroke" => &["color", "fill", "join", "cap", "miterLimit"],
        tag => {
            return Err(format!(
                "{}: native vector rendering of <{tag}> is not ported yet",
                node.id
            ));
        }
    };
    for key in node.props.keys() {
        if !common.contains(&key.as_str()) && !specific.contains(&key.as_str()) {
            return Err(format!(
                "{}: render property {key:?} is not ported yet; refusing an incomplete render",
                node.id
            ));
        }
    }
    if node.flag("keepAspectRatio") {
        return Err(format!(
            "{}: native aspect constraints are not ported yet",
            node.id
        ));
    }
    Ok(())
}

fn checked_nodes<'a>(
    root: &'a Node,
    cancel: Option<&AtomicBool>,
) -> std::result::Result<Vec<&'a Node>, FrameError> {
    let mut nodes = Vec::new();
    let mut pending = vec![(root, 0usize)];
    while let Some((node, depth)) = pending.pop() {
        cancelled(cancel).map_err(FrameError::transient)?;
        if depth > 128 || nodes.len() + pending.len() + node.children.len() + 1 > 100_000 {
            return Err("Native scene exceeds 100000 nodes or 128 levels".into());
        }
        nodes.push(node);
        pending.extend(node.children.iter().map(|n| (n, depth + 1)));
    }
    Ok(nodes)
}
fn validate_children(
    scene: &Node,
    cancel: Option<&AtomicBool>,
) -> std::result::Result<(), FrameError> {
    let mut pending = vec![(scene, None::<&Node>)];
    while let Some((node, parent)) = pending.pop() {
        cancelled(cancel).map_err(FrameError::transient)?;
        validate(node, parent.is_none())?;
        let valid = match node.tag.as_str() {
            "textRange" => parent.is_some_and(|p| p.tag == "text"),
            "colorStop" => parent.is_some_and(|p| {
                matches!(
                    p.tag.as_str(),
                    "linearGradientPaint" | "radialGradientPaint"
                )
            }),
            tag if paints::is_fill(tag) || tag == "stroke" => parent.is_some_and(|p| {
                matches!(
                    p.tag.as_str(),
                    "scene" | "rect" | "image" | "video" | "text" | "textRange"
                )
            }),
            _ => parent.is_none_or(|p| !paints::is_style(&p.tag)),
        };
        if !valid {
            return Err(format!("{}: invalid parent for <{}>", node.id, node.tag).into());
        }
        if matches!(node.tag.as_str(), "solidPaint" | "stroke" | "colorStop")
            && !node.children.is_empty()
        {
            return Err(
                format!("{}: this paint/style cannot have render children", node.id).into(),
            );
        }
        if node
            .children
            .iter()
            .filter(|n| paints::is_fill(&n.tag) || n.tag == "stroke")
            .count()
            > 1024
        {
            return Err("A holder accepts at most 1024 native paints/strokes".into());
        }
        pending.extend(node.children.iter().map(|n| (n, Some(node))));
    }
    Ok(())
}

struct Brushes<'a> {
    original: BTreeMap<&'a str, &'a Node>,
    gradients: BTreeMap<String, Arc<Gradient>>,
    points: usize,
    stops: usize,
}
impl Brushes<'_> {
    fn authored<'a>(&'a self, node: &'a Node) -> &'a Node {
        self.original.get(node.id.as_str()).copied().unwrap_or(node)
    }
    fn ink(&mut self, node: &Node, size: [f64; 2], origin: [f64; 2], text: bool) -> Result<Ink> {
        if matches!(node.tag.as_str(), "solidPaint" | "stroke") {
            Ok(Ink::Solid(paints::color(if text {
                self.authored(node)
            } else {
                node
            })))
        } else {
            if !self.gradients.contains_key(&node.id) {
                let gradient = Gradient::from_node(node, self.authored(node), size)?;
                self.gradients.insert(node.id.clone(), Arc::new(gradient));
            }
            self.stops = self
                .stops
                .saturating_add(self.gradients[&node.id].stops.len());
            if self.stops > 1_000_000 {
                return Err("Native frame exceeds one million gradient stop applications".into());
            }
            Ok(Ink::Gradient {
                gradient: self.gradients[&node.id].clone(),
                origin,
            })
        }
    }
}
fn push_draw(draws: &mut Vec<Draw>, draw: Draw) -> Result<()> {
    if draws.len() >= 65536 {
        return Err("Native frame exceeds 65536 draws".into());
    }
    draws.push(draw);
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn draw_style(
    draws: &mut Vec<Draw>,
    brushes: &mut Brushes<'_>,
    layer: &Layer<'_>,
    node: &Node,
    path: Option<&Arc<Path>>,
    origin: [f64; 2],
    text: bool,
    line: &mut LineStyle,
    current_alpha: &mut f64,
) -> Result<()> {
    if node.flag("hidden") {
        return Ok(());
    }
    let stroke = if node.tag == "stroke" {
        Some(line.apply(node)?)
    } else {
        None
    };
    let own = paints::number(
        if text { brushes.authored(node) } else { node },
        "opacity",
        1.0,
    );
    *current_alpha = paints::assigned_alpha(*current_alpha, layer.opacity * own);
    if let Some(path) = path {
        brushes.points = brushes.points.saturating_add(path.points().len());
        if brushes.points > 8_000_000 {
            return Err("Native frame exceeds eight million styled path points".into());
        }
        let ink = brushes.ink(node, layer.state.frame.size, origin, text)?;
        push_draw(
            draws,
            Draw {
                path: path.clone(),
                world: layer.world.multiply(Mat2D::translate(origin[0], origin[1])),
                ink,
                opacity: *current_alpha,
                clipped: layer.parent.is_some(),
                blend: paints::blend(node, layer.blend),
                stroke,
            },
        )?;
    }
    Ok(())
}

impl Compositor {
    /// Selection of the vector path depends on every materialized node, including
    /// hidden/offscreen ones. Projects containing other node types use the older
    /// renderer, which has its own supported subset. Unsupported properties are
    /// explicit planning errors in either backend; this is not a style fallback.
    pub fn supports(scene: &Node) -> bool {
        let mut pending = vec![scene];
        while let Some(node) = pending.pop() {
            if !matches!(
                node.tag.as_str(),
                "scene"
                    | "rect"
                    | "group"
                    | "sequence"
                    | "audio"
                    | "image"
                    | "video"
                    | "text"
                    | "textRange"
                    | "solidPaint"
                    | "linearGradientPaint"
                    | "radialGradientPaint"
                    | "colorStop"
                    | "stroke"
            ) {
                return false;
            }
            pending.extend(&node.children);
        }
        true
    }
    pub fn reset(&mut self) {
        self.previous.clear();
        self.scene.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        scene: &Node,
        timeline: &Timeline,
        time: f64,
        fps: f64,
        sizes_resolved_by_motion: bool,
        background: Option<u32>,
    ) -> Result<NativeFrame> {
        checked_nodes(scene, None).map_err(|e| e.message)?;
        let mut texts = crate::text::prepare(scene, scene, &Default::default());
        self.frame_on_canvas(
            scene,
            scene,
            timeline,
            time,
            fps,
            sizes_resolved_by_motion,
            background,
            None,
            None,
            &mut texts,
            &mut Default::default(),
            None,
        )
        .map_err(|error| error.message)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_on_canvas(
        &mut self,
        scene: &Node,
        authored: &Node,
        timeline: &Timeline,
        time: f64,
        fps: f64,
        sizes_resolved_by_motion: bool,
        background: Option<u32>,
        canvas: Option<[f64; 2]>,
        media: Option<MediaContext<'_>>,
        texts: &mut crate::text::PreparedTexts,
        fonts: &mut crate::fonts::FontBook,
        cancel: Option<&AtomicBool>,
    ) -> std::result::Result<NativeFrame, FrameError> {
        if !time.is_finite() || !fps.is_finite() || !(0.0..=240.0).contains(&fps) || fps == 0.0 {
            return Err("Invalid native frame clock".into());
        }
        let mut layers: Vec<Layer<'_>> = Vec::new();
        cancelled(cancel).map_err(FrameError::transient)?;
        let original_nodes = checked_nodes(authored, cancel)?
            .into_iter()
            .map(|n| (n.id.as_str(), n))
            .collect::<BTreeMap<_, _>>();
        let mut brushes = Brushes {
            original: original_nodes,
            gradients: BTreeMap::new(),
            points: 0,
            stops: 0,
        };
        let sampled_nodes = checked_nodes(scene, cancel)?
            .into_iter()
            .map(|n| (n.id.as_str(), n))
            .collect::<BTreeMap<_, _>>();
        validate_children(scene, cancel)?;
        let mut pending: Vec<(&Node, Option<usize>)> = vec![(scene, None)];
        let global_frame = round_frame(time * fps);
        while let Some((node, parent)) = pending.pop() {
            cancelled(cancel).map_err(FrameError::transient)?;
            validate(node, parent.is_none())?;
            if paints::is_style(&node.tag) {
                continue;
            }
            let index = layers.len();
            let old = if self.scene == scene.id {
                self.previous.get(&node.id).copied().unwrap_or_default()
            } else {
                GeometryState::default()
            };
            let group = matches!(node.tag.as_str(), "group" | "sequence");
            let defaults = match node.tag.as_str() {
                "scene" | "image" | "video" => [1920.0, 1080.0],
                "audio" => [500.0, 150.0],
                _ => [100.0; 2],
            };
            let mut text_state = None;
            let size = if node.tag == "text" {
                let prepared = texts.get(&node.id).ok_or("Missing prepared text")?;
                let mut state = old.text.unwrap_or(TextGeometry {
                    authored: [None; 2],
                    size: None,
                });
                let mut computed = old.frame.size;
                if old.text.is_none() || state.authored != prepared.authored_size {
                    let previous_authored = state.authored;
                    state.authored = prepared.authored_size;
                    if state.authored.iter().any(Option::is_some) {
                        let mut size = state.size.unwrap_or([0.0; 2]);
                        let mut changed = false;
                        for (i, value) in state.authored.iter().enumerate() {
                            if let Some(value) = value
                                && (old.text.is_none() || previous_authored[i] != Some(*value))
                            {
                                size[i] = round_frame(*value);
                                changed = true;
                            }
                        }
                        state.size = Some(size);
                        if changed {
                            computed = size;
                        }
                    } else {
                        state.size = None;
                    }
                }
                for (i, value) in prepared.animated_size.iter().enumerate() {
                    if let Some(value) = value {
                        computed[i] = *value;
                    }
                }
                text_state = Some(state);
                computed
            } else if group {
                old.frame.size
            } else {
                let raw = [
                    node.number("width", defaults[0])?,
                    node.number("height", defaults[1])?,
                ];
                if sizes_resolved_by_motion {
                    raw
                } else {
                    raw.map(round_frame)
                }
            };
            let scalar = node.number("scale", 1.0)?;
            let scale = if node.props.contains_key("scale") {
                [scalar; 2]
            } else {
                [node.number("scaleX", 1.0)?, node.number("scaleY", 1.0)?]
            };
            let mut values = TransformValues {
                size,
                position: [node.number("x", 0.0)?, node.number("y", 0.0)?],
                offset: [node.number("offsetX", 0.0)?, node.number("offsetY", 0.0)?],
                rotation: node.number("rotation", 0.0)?,
                scale,
                ..Default::default()
            };
            // Offline capture normalizes the scene every frame after motion.
            // Sequences have no spatial identity (the JSX reconciler ignores it).
            if parent.is_none() || node.tag == "sequence" {
                values = TransformValues {
                    size,
                    ..Default::default()
                };
            }
            let span = timeline.get(&node.id)?;
            let visible = !span.hidden
                && (parent.is_none()
                    || (global_frame >= span.start * fps && global_frame < span.end * fps));
            layers.push(Layer {
                node,
                parent,
                children: Vec::new(),
                values,
                state: GeometryState {
                    text: text_state,
                    local: old.local,
                    frame: Frame {
                        origin: old.frame.origin,
                        size,
                    },
                },
                world: Mat2D::IDENTITY,
                visible,
                opacity: 1.0,
                culled: false,
                blend: BlendMode::SourceOver,
                line: LineStyle::default(),
            });
            if let Some(parent) = parent {
                layers[parent].children.push(index);
            }
            pending.extend(node.children.iter().rev().map(|child| (child, Some(index))));
        }
        let canvas = canvas.unwrap_or(layers[0].state.frame.size);
        let [width, height] = canvas_dimensions(canvas, 1.0)?;
        // Match the original parent-first traversal: a group's children still
        // carry their previous local matrices when its box is recomputed.
        for index in 0..layers.len() {
            cancelled(cancel).map_err(FrameError::transient)?;
            let parent = layers[index].parent;
            if layers[index].node.tag == "sequence"
                && let Some(parent) = parent
            {
                layers[index].state.frame = layers[parent].state.frame;
            } else if matches!(layers[index].node.tag.as_str(), "group" | "sequence") {
                let children = layers[index]
                    .children
                    .iter()
                    .map(|&child| GroupChild {
                        local: layers[child].state.local,
                        size: layers[child].state.frame.size,
                        hidden: layers[child].node.flag("hidden"),
                        mask: false,
                    })
                    .collect::<Vec<_>>();
                layers[index].state.frame =
                    group_frame(&children, layers[index].state.frame.origin);
            }
            layers[index].values.size = layers[index].state.frame.size;
            let local = layers[index].values.local_matrix();
            let world = parent
                .map_or(Mat2D::IDENTITY, |p| layers[p].world)
                .multiply(local);
            let bounds = layers[index].state.frame.bounds(world);
            let culled = is_culled(
                bounds,
                parent.map(|p| layers[p].culled),
                Some([width as f64, height as f64]),
            );
            let own_opacity = layers[index].node.number("opacity", 1.0)?;
            let opacity = alpha(parent.map_or(1.0, |p| layers[p].opacity), own_opacity);
            let visible = layers[index].visible && parent.is_none_or(|p| layers[p].visible);
            layers[index].state.local = local;
            layers[index].world = world;
            layers[index].culled = culled;
            layers[index].opacity = opacity;
            layers[index].visible = visible;
            layers[index].blend = paints::blend(
                layers[index].node,
                parent.map_or(BlendMode::SourceOver, |p| layers[p].blend),
            );
            raster_transform(world)?;
        }
        let mut draws = Vec::new();
        let mut text_work = crate::text::TextWork::default();
        fonts.begin_frame();
        let clip = shape_path(scene, layers[0].state.frame.size)?;
        for index in 0..layers.len() {
            cancelled(cancel).map_err(FrameError::transient)?;
            let inherited_line = layers[index]
                .parent
                .map_or_else(LineStyle::default, |p| layers[p].line.clone());
            let layer = &mut layers[index];
            layer.line = inherited_line.clone();
            if !layer.visible
                || layer.culled
                || !matches!(
                    layer.node.tag.as_str(),
                    "scene" | "rect" | "image" | "video" | "text"
                )
            {
                continue;
            }
            if layer.node.tag == "text" {
                let prepared = texts
                    .get_mut(&layer.node.id)
                    .ok_or("Missing prepared text")?;
                crate::text::shape(layer.node, prepared, fonts, &mut text_work, cancel)
                    .map_err(FrameError::transient)?;
                let layout = prepared.layout(
                    layer.state.frame.size,
                    layer.state.text.and_then(|t| t.size),
                )?;
                // Render-time measurement changes Computed size after this
                // frame's transforms and parent-first group bounds were made.
                layer.state.frame.size = layout.size;
                let paths = layout
                    .tokens
                    .iter()
                    .map(|token| {
                        prepared.words[token.piece]
                            .shaped
                            .path
                            .clone()
                            .map(Arc::new)
                    })
                    .collect::<Vec<_>>();
                let mut line = inherited_line.clone();
                let mut current_alpha = layer.opacity;
                // Canvas completes every word's stroke pass before any fills.
                for (token, path) in layout.tokens.iter().zip(&paths) {
                    cancelled(cancel).map_err(FrameError::transient)?;
                    let word = &prepared.words[token.piece];
                    if let Some(holder) =
                        word.strokes.as_deref().and_then(|id| sampled_nodes.get(id))
                    {
                        for stroke in holder.children.iter().filter(|n| n.tag == "stroke") {
                            draw_style(
                                &mut draws,
                                &mut brushes,
                                layer,
                                stroke,
                                path.as_ref(),
                                [token.x as f64, token.y as f64],
                                true,
                                &mut line,
                                &mut current_alpha,
                            )?;
                        }
                    }
                }
                current_alpha = layer.opacity;
                for (token, path) in layout.tokens.iter().zip(&paths) {
                    cancelled(cancel).map_err(FrameError::transient)?;
                    let word = &prepared.words[token.piece];
                    if let Some(color) = word.color {
                        current_alpha = layer.opacity;
                        if let Some(path) = path {
                            push_draw(
                                &mut draws,
                                Draw {
                                    path: path.clone(),
                                    world: layer
                                        .world
                                        .multiply(Mat2D::translate(token.x as f64, token.y as f64)),
                                    ink: Ink::Solid(color),
                                    opacity: layer.opacity,
                                    clipped: layer.parent.is_some(),
                                    blend: layer.blend,
                                    stroke: None,
                                },
                            )?;
                        }
                    }
                    if let Some(holder) = word.fills.as_deref().and_then(|id| sampled_nodes.get(id))
                    {
                        for fill in holder.children.iter().filter(|n| paints::is_fill(&n.tag)) {
                            draw_style(
                                &mut draws,
                                &mut brushes,
                                layer,
                                fill,
                                path.as_ref(),
                                [token.x as f64, token.y as f64],
                                true,
                                &mut line,
                                &mut current_alpha,
                            )?;
                        }
                    }
                }
                continue;
            }
            // Validate shape values even when unpainted.
            let path = shape_path(layer.node, layer.state.frame.size)?.map(Arc::new);
            if let Some(rgb) = crate::text::node_color(layer.node)
                && let Some(path) = &path
            {
                push_draw(
                    &mut draws,
                    Draw {
                        path: path.clone(),
                        world: layer.world,
                        ink: Ink::Solid(rgb),
                        opacity: layer.opacity,
                        clipped: layer.parent.is_some(),
                        blend: layer.blend,
                        stroke: None,
                    },
                )?;
            }
            if matches!(layer.node.tag.as_str(), "image" | "video")
                && let Some(path) = &path
            {
                let media = media
                    .as_ref()
                    .ok_or("Media composition needs its source library")?;
                let probe = media
                    .library
                    .probes
                    .get(&layer.node.id)
                    .ok_or("Unprobed texture source")?
                    .clone();
                let source = media
                    .library
                    .sources
                    .get(&layer.node.id)
                    .ok_or("Unresolved texture source")?
                    .clone();
                let frame = if layer.node.tag == "image" {
                    TextureFrame::Image
                } else if probe.still {
                    TextureFrame::Video(0.0)
                } else {
                    let span = timeline.get(&layer.node.id)?;
                    TextureFrame::VideoClock {
                        time,
                        origin: span.origin,
                        rate: span.playback_rate,
                        fps,
                    }
                };
                push_draw(
                    &mut draws,
                    Draw {
                        path: path.clone(),
                        world: layer.world,
                        opacity: layer.opacity,
                        clipped: layer.parent.is_some(),
                        blend: layer.blend,
                        stroke: None,
                        ink: Ink::Media {
                            request: Box::new(TextureRequest {
                                path: source,
                                probe,
                                frame,
                            }),
                            store: media.store.clone(),
                            size: layer.state.frame.size,
                            fit: Fit::from_prop(layer.node.string("objectFit")),
                        },
                    },
                )?;
            }
            let mut line = inherited_line;
            for style in layer
                .node
                .children
                .iter()
                .filter(|n| paints::is_fill(&n.tag))
                .chain(layer.node.children.iter().filter(|n| n.tag == "stroke"))
            {
                cancelled(cancel).map_err(FrameError::transient)?;
                let mut opacity = layer.opacity;
                draw_style(
                    &mut draws,
                    &mut brushes,
                    layer,
                    style,
                    path.as_ref(),
                    [0.0; 2],
                    false,
                    &mut line,
                    &mut opacity,
                )?;
            }
            layer.line = line;
        }
        let mut previous = BTreeMap::new();
        for layer in &layers {
            cancelled(cancel).map_err(FrameError::transient)?;
            previous.insert(layer.node.id.clone(), layer.state);
        }
        cancelled(cancel).map_err(FrameError::transient)?;
        self.previous = previous;
        self.scene = scene.id.clone();
        Ok(NativeFrame {
            width,
            height,
            canvas,
            resolution: 1.0,
            background,
            clip,
            draws,
        })
    }
}

fn canvas_dimensions(canvas: [f64; 2], resolution: f64) -> Result<[u32; 2]> {
    if canvas.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err("Capture canvas dimensions must be finite and positive".into());
    }
    Ok([
        dimension(round_frame(canvas[0] * resolution / 2.0) * 2.0)?,
        dimension(round_frame(canvas[1] * resolution / 2.0) * 2.0)?,
    ])
}

fn raster_transform(matrix: Mat2D) -> Result<Transform> {
    let values = [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f].map(|v| v as f32);
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Native transform exceeds the finite raster coordinate range".into());
    }
    Ok(Transform::from_row(
        values[0], values[1], values[2], values[3], values[4], values[5],
    ))
}

fn shape_path(node: &Node, size: [f64; 2]) -> Result<Option<Path>> {
    if size
        .iter()
        .any(|v| !v.is_finite() || !(*v as f32).is_finite())
    {
        return Err(format!("{}: non-finite shape size", node.id));
    }
    let uniform = node.number("cornerRadius", 0.0)?;
    let mut radii = [
        "cornerRadiusTopLeft",
        "cornerRadiusTopRight",
        "cornerRadiusBottomRight",
        "cornerRadiusBottomLeft",
    ]
    .map(|key| node.number(key, uniform))
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    if radii.iter().any(|v| *v < 0.0 || !(*v as f32).is_finite()) {
        return Err(format!(
            "{}: native corner radii must be finite and nonnegative",
            node.id
        ));
    }
    let [w, h] = size;
    if w == 0.0 || h == 0.0 {
        return Ok(None);
    }
    if radii.iter().all(|v| *v == 0.0) {
        return Ok(Rect::from_ltrb(
            w.min(0.0) as f32,
            h.min(0.0) as f32,
            w.max(0.0) as f32,
            h.max(0.0) as f32,
        )
        .map(PathBuilder::from_rect));
    }
    if w < 0.0 || h < 0.0 {
        return Err(format!(
            "{}: rounded shapes with negative size are not ported yet",
            node.id
        ));
    }
    let edge = |sum: f64| if sum == 0.0 { 1.0 } else { sum };
    let scale = (w / edge(radii[0] + radii[1]))
        .min(h / edge(radii[1] + radii[2]))
        .min(w / edge(radii[2] + radii[3]))
        .min(h / edge(radii[3] + radii[0]))
        .min(1.0);
    for r in &mut radii {
        *r *= scale;
    }
    let [tl, tr, br, bl] = [
        radii[0] as f32,
        radii[1] as f32,
        radii[2] as f32,
        radii[3] as f32,
    ];
    let (w, h) = (w as f32, h as f32);
    // Four cubic arcs per corner keep the geometric approximation much smaller
    // than a pixel for ordinary scene sizes. Backend antialiasing is not assumed
    // byte-identical to the browser's Canvas rasterizer.
    let mut p = PathBuilder::new();
    p.move_to(tl, 0.0);
    p.line_to(w - tr, 0.0);
    corner(&mut p, [w - tr, tr], tr, -90.0);
    p.line_to(w, h - br);
    corner(&mut p, [w - br, h - br], br, 0.0);
    p.line_to(bl, h);
    corner(&mut p, [bl, h - bl], bl, 90.0);
    p.line_to(0.0, tl);
    corner(&mut p, [tl, tl], tl, 180.0);
    p.close();
    Ok(p.finish())
}

fn corner(path: &mut PathBuilder, center: [f32; 2], radius: f32, start: f64) {
    if radius == 0.0 {
        return;
    }
    let step = std::f64::consts::FRAC_PI_2 / 4.0;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |x: f64, y: f64| {
        [
            (center[0] as f64 + radius as f64 * x) as f32,
            (center[1] as f64 + radius as f64 * y) as f32,
        ]
    };
    for i in 0..4 {
        let a = start.to_radians() + i as f64 * step;
        let b = a + step;
        let p1 = point(a.cos() - k * a.sin(), a.sin() + k * a.cos());
        let p2 = point(b.cos() + k * b.sin(), b.sin() - k * b.cos());
        let end = point(b.cos(), b.sin());
        path.cubic_to(p1[0], p1[1], p2[0], p2[1], end[0], end[1]);
    }
}

fn raster_bytes(len: usize) -> Result<Vec<u8>> {
    if len > 256 * 1024 * 1024 {
        return Err("Native raster buffer exceeds 256 MiB".into());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| "Cannot allocate native raster buffer")?;
    bytes.resize(len, 0);
    Ok(bytes)
}
fn raster_mask(width: u32, height: u32) -> Result<Mask> {
    let size = tiny_skia::IntSize::from_wh(width, height).ok_or("Invalid native mask size")?;
    let len = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Native mask size overflows")?;
    if len > 64 * 1024 * 1024 {
        return Err("Native mask exceeds 64 MiB".into());
    }
    Mask::from_vec(raster_bytes(len)?, size).ok_or_else(|| "Invalid native mask storage".into())
}
impl NativeFrame {
    pub(crate) fn resize_output(&mut self, height: u32) -> Result<()> {
        dimension(height as f64)?;
        let resolution = round_frame(height as f64 * 1e6 / self.canvas[1]) / 1e6;
        let [width, height] = canvas_dimensions(self.canvas, resolution)?;
        self.width = width;
        self.height = height;
        self.resolution = resolution;
        Ok(())
    }
    pub fn pixmap(&self, width: u32, height: u32, cancel: Option<&AtomicBool>) -> Result<Pixmap> {
        cancelled(cancel)?;
        dimension(width as f64)?;
        dimension(height as f64)?;
        let size = tiny_skia::IntSize::from_wh(width, height).ok_or("Invalid native canvas")?;
        let mut pixels =
            Pixmap::from_vec(raster_bytes(width as usize * height as usize * 4)?, size)
                .ok_or("Invalid native frame storage")?;
        if let Some(rgb) = self.background {
            pixels.fill(Color::from_rgba8(
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
                255,
            ));
        }
        let scale = if [width, height] == [self.width, self.height] {
            self.resolution
        } else {
            round_frame(height as f64 * 1e6 / self.canvas[1]) / 1e6
        };
        let transform = raster_transform(Mat2D::scale(scale, scale))?;
        let mut mask = raster_mask(width, height)?;
        if let Some(clip) = &self.clip {
            mask.fill_path(clip, FillRule::Winding, true, transform);
        }
        for draw in &self.draws {
            cancelled(cancel)?;
            match &draw.ink {
                Ink::Solid(_) | Ink::Gradient { .. } => {
                    let shader = match &draw.ink {
                        Ink::Solid(rgb) => Some(paints::solid_shader(*rgb, draw.opacity)),
                        Ink::Gradient { gradient, origin } => {
                            gradient.shader(draw.opacity, *origin)?
                        }
                        _ => unreachable!(),
                    };
                    let Some(shader) = shader else { continue };
                    let paint = Paint {
                        shader,
                        blend_mode: draw.blend,
                        force_hq_pipeline: draw.blend != BlendMode::SourceOver
                            || matches!(&draw.ink, Ink::Gradient { .. }),
                        ..Paint::default()
                    };
                    if let Some(stroke) = &draw.stroke {
                        pixels.stroke_path(
                            &draw.path,
                            &paint,
                            stroke,
                            raster_transform(draw.world.view(scale))?,
                            draw.clipped.then_some(&mask),
                        );
                    } else {
                        pixels.fill_path(
                            &draw.path,
                            &paint,
                            FillRule::Winding,
                            raster_transform(draw.world.view(scale))?,
                            draw.clipped.then_some(&mask),
                        );
                    }
                }
                Ink::Media {
                    request,
                    store,
                    size,
                    fit,
                } => {
                    let bitmap = store.frame(request, cancel)?;
                    let [mut x, mut y, w, h] =
                        fit.rectangle([bitmap.width() as f64, bitmap.height() as f64], *size);
                    if w == 0.0 || h == 0.0 {
                        continue;
                    }
                    // Canvas drawImage with a negative destination size grows
                    // backwards without flipping source pixels.
                    if w < 0.0 {
                        x += w;
                    }
                    if h < 0.0 {
                        y += h;
                    }
                    let local = Mat2D::translate(x, y).multiply(Mat2D::scale(
                        w.abs() / bitmap.width() as f64,
                        h.abs() / bitmap.height() as f64,
                    ));
                    // Fill the media mask directly, then intersect in place.
                    // Mask::clone/intersect_path allocate infallibly and would
                    // need an additional full-size temporary mask.
                    let mut clip = raster_mask(width, height)?;
                    clip.fill_path(
                        &draw.path,
                        FillRule::Winding,
                        true,
                        raster_transform(draw.world.view(scale))?,
                    );
                    if draw.clipped {
                        for (part, scene) in clip
                            .data_mut()
                            .chunks_mut(65536)
                            .zip(mask.data().chunks(65536))
                        {
                            cancelled(cancel)?;
                            for (a, b) in part.iter_mut().zip(scene) {
                                let product = u32::from(*a) * u32::from(*b) + 128;
                                *a = (((product >> 8) + product) >> 8) as u8;
                            }
                        }
                    }
                    let paint = Paint {
                        blend_mode: draw.blend,
                        force_hq_pipeline: true,
                        shader: Pattern::new(
                            bitmap.as_ref().as_ref(),
                            SpreadMode::Pad,
                            FilterQuality::Bilinear,
                            draw.opacity as f32,
                            Transform::identity(),
                        ),
                        ..Paint::default()
                    };
                    let path = PathBuilder::from_rect(
                        Rect::from_xywh(0.0, 0.0, bitmap.width() as f32, bitmap.height() as f32)
                            .ok_or("Invalid bitmap bounds")?,
                    );
                    pixels.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        raster_transform(draw.world.multiply(local).view(scale))?,
                        Some(&clip),
                    );
                }
            }
        }
        cancelled(cancel)?;
        Ok(pixels)
    }
    pub fn rgba(
        &self,
        width: u32,
        height: u32,
        flatten_black: bool,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>> {
        let mut pixels = self.pixmap(width, height, cancel)?;
        if flatten_black {
            // tiny-skia holds premultiplied RGB: these are already the colors
            // over black. H.264 drops alpha, so make that compositing explicit.
            for pixel in pixels.data_mut().as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            cancelled(cancel)?;
            Ok(pixels.take())
        } else {
            let bytes = pixels.take_demultiplied();
            cancelled(cancel)?;
            Ok(bytes)
        }
    }
}

#[cfg(test)]
mod text_geometry_tests {
    use super::*;
    #[test]
    fn raster_buffers_reject_excessive_sizes_before_allocation() {
        assert!(raster_bytes(usize::MAX).is_err());
        assert!(raster_mask(8193, 8192).is_err());
        assert_eq!(raster_bytes(16).unwrap(), vec![0; 16]);
        assert_eq!(raster_mask(4, 4).unwrap().data(), &[0; 16]);
    }
    #[test]
    fn nontext_frame_cancellation_is_transient_and_does_not_commit_history() {
        let doc=crate::source::SourceDocument::parse("cancel-paints.tsx".into(),"export default <stage><scene id=\"s\" width={40} height={40}><rect id=\"r\" width={20} height={20}><solidPaint color=\"red\"/></rect></scene></stage>".into()).unwrap();
        let scene = doc.root.find("s").unwrap();
        let timeline = Timeline::resolve(&doc.root, &Default::default()).unwrap();
        let mut compositor = Compositor::default();
        let mut texts = crate::text::prepare(scene, scene, &Default::default());
        let cancel = AtomicBool::new(true);
        let error = compositor
            .frame_on_canvas(
                scene,
                scene,
                &timeline,
                0.0,
                30.0,
                false,
                None,
                None,
                None,
                &mut texts,
                &mut Default::default(),
                Some(&cancel),
            )
            .err()
            .unwrap();
        assert!(error.transient);
        assert!(compositor.previous.is_empty());
        assert!(compositor.scene.is_empty());
        compositor
            .frame(scene, &timeline, 0.0, 30.0, false, None)
            .unwrap();
        assert_eq!(compositor.previous.len(), 2);
    }
    #[test]
    fn native_scene_node_and_depth_budgets_also_cover_nondrawing_nodes() {
        let doc = crate::source::SourceDocument::parse(
            "bounds.tsx".into(),
            "export default <stage><scene id=\"s\"><group/></scene></stage>".into(),
        )
        .unwrap();
        let mut scene = doc.root.find("s").unwrap().clone();
        scene.children = vec![scene.children[0].clone(); 100_000];
        assert!(
            checked_nodes(&scene, None)
                .err()
                .unwrap()
                .message
                .contains("100000")
        );
        scene.children.clear();
        for _ in 0..129 {
            let child = scene.clone();
            scene.children = vec![child];
        }
        assert!(
            checked_nodes(&scene, None)
                .err()
                .unwrap()
                .message
                .contains("128")
        );
    }
    #[test]
    fn adding_the_first_size_axis_starts_the_other_axis_at_trait_default_zero() {
        let doc=crate::source::SourceDocument::parse("size.tsx".into(),"export default <stage><scene id=\"s\" width={240} height={160}><text id=\"t\" x={40} y={40} color=\"red\" fontSize={32} rotation={30}>Hello</text></scene></stage>".into()).unwrap();
        let mut root = doc.root;
        let timeline = Timeline::resolve(&root, &Default::default()).unwrap();
        let mut compositor = Compositor::default();
        compositor
            .frame(root.find("s").unwrap(), &timeline, 0.0, 30.0, false, None)
            .unwrap();
        assert_eq!(compositor.previous["t"].frame.size, [71.0, 39.0]);
        root.children[0].children[0]
            .props
            .insert("width".into(), 120.into());
        compositor
            .frame(root.find("s").unwrap(), &timeline, 0.0, 30.0, false, None)
            .unwrap();
        let state = compositor.previous["t"];
        assert_eq!(state.text.unwrap().size, Some([120.0, 0.0]));
        let values = TransformValues {
            position: [40.0, 40.0],
            size: [120.0, 0.0],
            rotation: 30.0,
            ..Default::default()
        };
        assert_eq!(state.local, values.local_matrix());
    }
    #[test]
    fn size_tracks_keep_authored_wrap_width_while_changing_the_computed_container() {
        let doc=crate::source::SourceDocument::parse("size-track.tsx".into(),"export default <stage><scene id=\"s\" width={240} height={160}><text id=\"t\" width={120} height={40} color=\"red\">Hello world</text></scene></stage>".into()).unwrap();
        let scene = doc.root.find("s").unwrap();
        let timeline = Timeline::resolve(&doc.root, &Default::default()).unwrap();
        let mut texts = crate::text::prepare(
            scene,
            scene,
            &BTreeMap::from([("t".into(), [Some(10.0), None])]),
        );
        let mut compositor = Compositor::default();
        compositor
            .frame_on_canvas(
                scene,
                scene,
                &timeline,
                0.0,
                30.0,
                true,
                None,
                None,
                None,
                &mut texts,
                &mut Default::default(),
                None,
            )
            .unwrap();
        let state = compositor.previous["t"];
        assert_eq!(state.frame.size, [10.0, 40.0]);
        assert_eq!(state.text.unwrap().size, Some([120.0, 40.0]));
        let layout = texts["t"]
            .layout(state.frame.size, state.text.unwrap().size)
            .unwrap();
        assert!(layout.tokens.iter().all(|token| token.line == 0));
    }
}

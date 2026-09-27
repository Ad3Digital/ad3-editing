// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Derived per-frame motion. Authored trees are immutable; presets are applied
//! in child order, then keyframe tracks override their respective properties.
use crate::{
    Result, color,
    document::Node,
    easing::{Easing, cubic_bezier},
    text_motion,
    timing::{Span, Timeline, time_value},
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Motion {
    easings: BTreeMap<String, Easing>,
}
const PROPERTIES: &[&str] = &[
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
    "cornerRadius",
    "cornerRadiusTopLeft",
    "cornerRadiusTopRight",
    "cornerRadiusBottomRight",
    "cornerRadiusBottomLeft",
    "volume",
    "color",
    "offset",
    "blur",
    "value",
];

pub fn has_motion(root: &Node) -> bool {
    root.nodes()
        .iter()
        .any(|n| matches!(n.tag.as_str(), "animation" | "keyframeTrack"))
}
fn frame(seconds: f64, fps: f64) -> f64 {
    crate::timing::round_frame(seconds * fps)
}
fn seconds(node: &Node, property: &str, fallback: f64) -> Result<f64> {
    node.props
        .get(property)
        .map(time_value)
        .transpose()
        .map(|n| n.unwrap_or(fallback))
}
fn scalar(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64().filter(|n| n.is_finite()),
        Some(Value::String(s)) => {
            let n = boa_engine::js_string!(s.as_str()).to_number();
            n.is_finite().then_some(n)
        }
        _ => None,
    }
}
fn value(node: &Node) -> f64 {
    scalar(node.props.get("value"))
        .or_else(|| node.string("value").and_then(color::parse).map(f64::from))
        .unwrap_or(0.0)
}

impl Motion {
    pub fn sample(
        &mut self,
        root: &Node,
        timeline: &Timeline,
        time: f64,
        fps: f64,
    ) -> Result<Node> {
        let scene_frame = frame(time, fps);
        if !time.is_finite()
            || !scene_frame.is_finite()
            || !fps.is_finite()
            || fps <= 0.0
            || fps > 240.0
        {
            return Err("Invalid native motion clock".into());
        }
        crate::stack::runtime(|| self.node(root, timeline, scene_frame, fps, None))
    }
    fn node(
        &mut self,
        original: &Node,
        timeline: &Timeline,
        scene_frame: f64,
        fps: f64,
        parent_clock: Option<&Span>,
    ) -> Result<Node> {
        let clock = timeline.spans.get(&original.id).or(parent_clock);
        let local = clock.map_or(0.0, |span| {
            if span.id == span.scene_id {
                scene_frame
            } else {
                crate::timing::round_frame((scene_frame - span.origin * fps) * span.playback_rate)
            }
        });
        let mut output = Node {
            id: original.id.clone(),
            tag: original.tag.clone(),
            props: original.props.clone(),
            text: original.text.clone(),
            location: original.location.clone(),
            children: Vec::with_capacity(original.children.len()),
        };
        // Size reaches Computed through resizeEntity/propagateSize, rounded
        // before motion. Width/height keyframes may then assign fractional sizes.
        if matches!(
            original.tag.as_str(),
            "scene" | "rect" | "image" | "video" | "audio" | "adjustmentLayer" | "html" | "surface"
        ) {
            for property in ["width", "height"] {
                if let Some(value) = output
                    .props
                    .get(property)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                {
                    output
                        .props
                        .insert(property.into(), crate::timing::round_frame(value).into());
                }
            }
        }
        let corners = [
            "cornerRadiusTopLeft",
            "cornerRadiusTopRight",
            "cornerRadiusBottomRight",
            "cornerRadiusBottomLeft",
        ];
        let vector = matches!(original.tag.as_str(), "rect" | "scene");
        let mixed = corners.iter().any(|key| original.props.contains_key(*key));
        if vector && mixed {
            // MixedCornerRadius is established from authored props, before
            // motion. An animated uniform radius cannot change its fallbacks.
            let uniform = original.number("cornerRadius", 0.0)?;
            for key in corners {
                output
                    .props
                    .insert(key.into(), original.number(key, uniform)?.into());
            }
        }
        let mut numbers = BTreeMap::<String, f64>::new();
        // The runtime mirrors uniform scale only on the enclosing geometry,
        // group or adjustment layer, never on a nested paint/style target.
        let mut uniform = original.is_timed() && original.props.contains_key("scale");
        let visible = clock.is_some_and(|span| {
            !span.hidden
                && (span.id == span.scene_id
                    || (scene_frame >= span.start * fps && scene_frame < span.end * fps))
        });
        for animation in original.children.iter().filter(|n| n.tag == "animation") {
            if !original.is_timed() || matches!(original.tag.as_str(), "stage" | "sequence") {
                return Err(format!(
                    "{}: animation needs a geometry or group holder",
                    animation.id
                ));
            }
            if !animation.children.is_empty() {
                return Err("Preset animations cannot have children".into());
            }
            Self::validate_props(animation, &["type", "phase", "duration", "delay"])?;
            let duration = frame(seconds(animation, "duration", 1.0)?, fps);
            let delay = frame(seconds(animation, "delay", 0.0)?, fps);
            if !duration.is_finite() || !delay.is_finite() {
                return Err("Animation frame timing overflows".into());
            }
            if !visible || duration <= 0.0 {
                continue;
            }
            let span = clock.ok_or("Animation holder has no source clock")?;
            let out = animation.string("phase") == Some("out");
            let edge = if out { span.end } else { span.start };
            let edge = frame((edge - span.origin) * span.playback_rate, fps);
            let start = if out {
                edge - duration - delay
            } else {
                edge + delay
            };
            if (out && local < start) || (!out && local >= start + duration) {
                continue;
            }
            let progress = ((local - start) / (duration - 1.0).max(1.0)).clamp(0.0, 1.0);
            Self::preset(
                original,
                &mut output.text,
                &mut numbers,
                animation.string("type").unwrap_or("fade"),
                out,
                progress,
            )?;
        }
        for track in original
            .children
            .iter()
            .filter(|n| n.tag == "keyframeTrack")
        {
            if original.tag == "sequence" {
                return Err("Sequence keyframe targeting is not implemented".into());
            }
            if clock.is_none() {
                return Err(format!(
                    "{}: keyframe track has no enclosing timed node",
                    track.id
                ));
            }
            Self::validate_props(track, &["property"])?;
            let property = track
                .string("property")
                .ok_or("Keyframe track needs a property")?;
            if !PROPERTIES.contains(&property) {
                return Err(format!("Unsupported keyframe property: {property}"));
            }
            let mut points = Vec::new();
            for point in &track.children {
                if point.tag != "keyframe" {
                    return Err("A keyframe track may contain only keyframes".into());
                }
                Self::validate_props(point, &["time", "value", "easing"])?;
                if !point.children.is_empty() {
                    return Err("Keyframes cannot have children".into());
                }
                let time = frame(seconds(point, "time", 0.0)?, fps);
                if !time.is_finite() {
                    return Err("Keyframe frame timing overflows".into());
                }
                points.push((
                    time,
                    value(point),
                    point.string("easing").unwrap_or("linear"),
                ));
            }
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            if !visible {
                continue;
            }
            if let Some(sample) = self.track(&points, local, property == "color")? {
                let key = if property == "scale" {
                    uniform = original.is_timed();
                    "scaleX"
                } else {
                    property
                };
                numbers.insert(key.into(), sample);
            }
        }
        if uniform && !numbers.is_empty() {
            let x = *numbers
                .get("scaleX")
                .unwrap_or(&original.number("scale", 1.0)?);
            numbers.insert("scaleX".into(), x);
            numbers.insert("scaleY".into(), x);
            output.props.remove("scale");
        }
        if vector {
            // Tracks update Computed stores, not trait membership. Without
            // Color or MixedCornerRadius these values do not become ink.
            if !original.props.contains_key("fill") {
                numbers.remove("color");
            }
            if !mixed {
                for key in corners {
                    numbers.remove(key);
                }
            }
        }
        Self::write_values(&mut output, &numbers)?;
        for child in &original.children {
            if matches!(child.tag.as_str(), "animation" | "keyframeTrack") {
                continue;
            }
            if child.tag == "keyframe" {
                return Err("Keyframe needs a keyframeTrack parent".into());
            }
            output
                .children
                .push(self.node(child, timeline, scene_frame, fps, clock)?);
        }
        Ok(output)
    }

    fn validate_props(node: &Node, supported: &[&str]) -> Result<()> {
        for name in node.props.keys() {
            if !supported.contains(&name.as_str())
                && !["id", "name", "selected", "expanded"].contains(&name.as_str())
            {
                return Err(format!("{}: unsupported motion property {name}", node.id));
            }
        }
        Ok(())
    }
    fn track(
        &mut self,
        points: &[(f64, f64, &str)],
        local: f64,
        color_track: bool,
    ) -> Result<Option<f64>> {
        let Some(first) = points.first() else {
            return Ok(None);
        };
        let last = points.last().unwrap();
        if points.len() == 1 || local <= first.0 {
            return Ok(Some(first.1));
        }
        if local >= last.0 {
            return Ok(Some(last.1));
        }
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if local < a.0 || local > b.0 || b.0 <= a.0 {
                continue;
            }
            let progress = ((local - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
            if !self.easings.contains_key(a.2) {
                if self.easings.len() >= 256 {
                    self.easings.clear();
                }
                self.easings.insert(a.2.into(), Easing::parse(a.2)?);
            }
            let progress = self.easings[a.2].sample(progress);
            return Ok(Some(if color_track {
                f64::from(color::lerp(
                    color::number(a.1).unwrap_or(0),
                    color::number(b.1).unwrap_or(0),
                    progress,
                ))
            } else {
                a.1 + (b.1 - a.1) * progress
            }));
        }
        Ok(Some(last.1))
    }

    fn write_values(node: &mut Node, values: &BTreeMap<String, f64>) -> Result<()> {
        for (key, &value) in values {
            if key == "volume" && value == f64::NEG_INFINITY {
                node.props.insert("muted".into(), Value::Bool(true));
                continue;
            }
            if !value.is_finite() {
                return Err(format!("{}: motion {key} is non-finite", node.id));
            }
            if key == "color" {
                let key = if matches!(node.tag.as_str(), "rect" | "scene") {
                    "fill"
                } else {
                    "color"
                };
                node.props.insert(
                    key.into(),
                    Value::String(format!("#{:06X}", color::number(value).unwrap_or(0))),
                );
            } else {
                node.props.insert(key.clone(), Value::from(value));
            }
        }
        Ok(())
    }
    fn preset(
        original: &Node,
        text: &mut String,
        numbers: &mut BTreeMap<String, f64>,
        kind: &str,
        out: bool,
        progress: f64,
    ) -> Result<()> {
        let common = cubic_bezier([0.1, 0.7, 0.5, 1.0], progress).clamp(0.0, 1.0);
        let t = if out { common } else { 1.0 - common };
        let mut set = |name: &str, value: f64| {
            numbers.insert(name.into(), value);
        };
        match kind {
            "gain" => {
                let eased = cubic_bezier([0.4, 0.095, 0.546, 0.875], progress).clamp(0.0, 1.0);
                let amplitude = if out { 1.0 - eased } else { eased };
                let delta = if amplitude <= 0.0 {
                    f64::NEG_INFINITY
                } else {
                    20.0 * amplitude.log10()
                };
                let volume = numbers
                    .get("volume")
                    .copied()
                    .unwrap_or(original.number("volume", 0.0)?);
                numbers.insert("volume".into(), volume + delta);
            }
            "grow" | "shrink" => {
                let scale = if kind == "grow" {
                    1.0 - 0.5 * t
                } else {
                    1.0 + 0.5 * t
                };
                set("scaleX", scale);
                set("scaleY", scale);
            }
            "blur" => {
                let curve = if out {
                    [0.4, 0.0, 1.0, 1.0]
                } else {
                    [0.33, 0.0, 0.2, 1.0]
                };
                let eased = cubic_bezier(curve, progress).clamp(0.0, 1.0);
                set(
                    "blur",
                    if out {
                        24.0 * eased
                    } else {
                        24.0 + (0.0 - 24.0) * eased
                    },
                );
            }
            "slideLeft" | "slideRight" | "slideUp" | "slideDown" => {
                let sign = if out { -1.0 } else { 1.0 };
                let direction = if matches!(kind, "slideLeft" | "slideUp") {
                    100.0
                } else {
                    -100.0
                };
                let key = if matches!(kind, "slideLeft" | "slideRight") {
                    "offsetX"
                } else {
                    "offsetY"
                };
                set(key, sign * direction * t);
                set("opacity", 1.0 - t);
            }
            "spin" => {
                let eased = cubic_bezier([0.44, 0.02, 0.252, 0.992], progress).clamp(0.0, 1.0);
                let t = if out { eased } else { 1.0 - eased };
                set("scaleX", 1.0 - t);
                set("scaleY", 1.0 - t);
                set("rotation", -45.0 * t);
            }
            "twist" => {
                set("scaleX", 1.0 + t);
                set("scaleY", 1.0 + t);
                set("rotation", -10.0 * t);
                set("offsetX", -30.0 * t);
                set("offsetY", -30.0 * t);
            }
            "appearWord" | "appearChar" | "scramble" => {
                if matches!(original.tag.as_str(), "text" | "captions") {
                    // Keep the original subtraction order: at rational frame
                    // boundaries, 1-(1-progress) can differ by one ULP and
                    // changes a reveal floor or the deterministic scramble step.
                    let t = if out { progress } else { 1.0 - progress };
                    let ratio = 1.0 - t;
                    *text = match kind {
                        "appearWord" => text_motion::reveal_words(&original.text, ratio),
                        "appearChar" => text_motion::reveal_chars(&original.text, ratio),
                        _ => text_motion::scramble_chars(&original.text, ratio),
                    };
                }
            }
            _ => set("opacity", if out { 1.0 - common } else { common }),
        }
        Ok(())
    }
}

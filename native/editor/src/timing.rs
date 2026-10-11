// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native timing contract, derived from reference/jsx/timing.md and runtime/actions/timing.ts.
use crate::{Result, document::Node};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub const TIME_FPS: f64 = 30.0;
pub const DEFAULT_DURATION: f64 = 16.0;

/// Match JavaScript's frame rounding, including negative half-frame values.
pub fn snap(seconds: f64) -> f64 {
    snap_at_rate(seconds, TIME_FPS)
}

pub(crate) fn round_frame(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

fn snap_at_rate(seconds: f64, fps: f64) -> f64 {
    round_frame(seconds * fps) / fps
}

pub fn parse_time(value: &str) -> Result<f64> {
    let text = value.trim();
    let invalid = || format!("Invalid time: {value:?}; use seconds, 45f or HH:MM:SS");
    if text.is_empty() {
        return Err(invalid());
    }
    let number = if text.ends_with(['f', 'F']) {
        text[..text.len() - 1]
            .parse::<f64>()
            .map_err(|_| invalid())?
            / TIME_FPS
    } else if text.contains(':') {
        let (sign, unsigned) = if let Some(s) = text.strip_prefix('-') {
            (-1.0, s)
        } else {
            (1.0, text)
        };
        let parts = unsigned.split(':').collect::<Vec<_>>();
        if !(2..=3).contains(&parts.len()) {
            return Err(invalid());
        }
        let mut seconds = 0.0;
        for part in parts {
            let n = part.parse::<f64>().map_err(|_| invalid())?;
            if !n.is_finite() || n < 0.0 {
                return Err(invalid());
            }
            seconds = seconds * 60.0 + n;
        }
        sign * seconds
    } else {
        text.parse::<f64>().map_err(|_| invalid())?
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(invalid())
    }
}

pub fn time_value(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => n
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| "Invalid time number".into()),
        Value::String(s) => parse_time(s),
        _ => Err("Time must be a number or time string".into()),
    }
}

fn time(node: &Node, name: &str) -> Result<Option<f64>> {
    node.props
        .get(name)
        .map(time_value)
        .transpose()
        .map_err(|error| format!("{}.{name}: {error}", node.id))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub id: String,
    pub parent_id: Option<String>,
    pub scene_id: String,
    /// Source second zero in scene seconds. Keep fractional audio precision.
    pub origin: f64,
    pub start: f64,
    pub end: f64,
    pub playback_rate: f64,
    pub source_in: f64,
    pub source_out: f64,
    pub hidden: bool,
    pub muted: bool,
    pub volume_db: f64,
}

impl Span {
    pub fn source_at(&self, scene_time: f64) -> f64 {
        (scene_time - self.origin) * self.playback_rate
    }
    pub fn visible(&self, scene_time: f64) -> bool {
        !self.hidden && scene_time >= self.start && scene_time < self.end
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Timeline {
    pub spans: BTreeMap<String, Span>,
    pub workareas: BTreeMap<String, [f64; 2]>,
    pub active: String,
}

impl Timeline {
    /// Durations are keyed by node id, in seconds at the source's natural speed.
    pub fn resolve(stage: &Node, durations: &BTreeMap<String, f64>) -> Result<Self> {
        Self::resolve_at_rate(stage, durations, TIME_FPS)
    }

    /// An offline capture world sets output FPS before mounting JSX, so every
    /// authored second is quantized into that world's frames on construction.
    /// Frame-string parsing still uses the language's fixed 30 fps unit.
    pub fn resolve_at_rate(
        stage: &Node,
        durations: &BTreeMap<String, f64>,
        fps: f64,
    ) -> Result<Self> {
        Self::resolve_known(stage, durations, None, fps)
    }
    /// A still asset bound through a video tag has no natural time cap. Keep
    /// the authored tag/identity while using the same default window as IMAGE.
    pub fn resolve_with_media(
        stage: &Node,
        library: &crate::media::MediaLibrary,
        fps: f64,
    ) -> Result<Self> {
        Self::resolve_known(stage, &library.durations, Some(&library.probes), fps)
    }
    fn resolve_known(
        stage: &Node,
        durations: &BTreeMap<String, f64>,
        probes: Option<&BTreeMap<String, crate::media::Probe>>,
        fps: f64,
    ) -> Result<Self> {
        Self::resolve_placed(stage, durations, probes, fps, &BTreeMap::new())
    }

    /// Offsets are measured recording-to-reference seconds, keyed by linked node ID.
    pub fn resolve_with_sync_offsets(
        stage: &Node,
        durations: &BTreeMap<String, f64>,
        fps: f64,
        offsets: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        Self::resolve_linked(stage, durations, None, fps, offsets)
    }

    pub fn resolve_with_media_sync(
        stage: &Node,
        library: &crate::media::MediaLibrary,
        fps: f64,
        offsets: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        if offsets.is_empty()
            && !stage
                .nodes()
                .iter()
                .any(|node| node.props.contains_key("syncTo"))
        {
            return Self::resolve_with_media(stage, library, fps);
        }
        Self::resolve_linked(
            stage,
            &library.durations,
            Some(&library.probes),
            fps,
            offsets,
        )
    }

    fn resolve_linked(
        stage: &Node,
        durations: &BTreeMap<String, f64>,
        probes: Option<&BTreeMap<String, crate::media::Probe>>,
        fps: f64,
        offsets: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        let mut plain = stage.clone();
        fn strip(node: &mut Node) {
            node.props.remove("syncTo");
            for child in &mut node.children {
                strip(child);
            }
        }
        strip(&mut plain);
        let nodes = stage.nodes();
        let mut pending = nodes
            .iter()
            .filter(|node| node.props.contains_key("syncTo"))
            .copied()
            .collect::<Vec<_>>();
        let mut placements = BTreeMap::new();
        let mut timeline = Self::resolve_placed(&plain, durations, probes, fps, &placements)?;
        while !pending.is_empty() {
            let mut progressed = false;
            let mut remaining = Vec::new();
            for node in &pending {
                let target = node
                    .string("syncTo")
                    .ok_or("syncTo requires a reference ID string")?;
                let reference = stage.find(target).ok_or_else(|| {
                    format!("{}: syncTo reference {target:?} does not exist", node.id)
                })?;
                if !matches!(node.tag.as_str(), "audio" | "video")
                    || !matches!(reference.tag.as_str(), "audio" | "video")
                {
                    return Err("syncTo requires audio/video recordings on both ends".into());
                }
                if pending.iter().any(|other| other.id == target) {
                    remaining.push(*node);
                    continue;
                }
                let recording = timeline.get(&node.id)?;
                let reference = timeline.get(target)?;
                if recording.scene_id != reference.scene_id {
                    return Err("syncTo reference must belong to the same root scene".into());
                }
                let offset = *offsets
                    .get(&node.id)
                    .ok_or_else(|| format!("{}: missing measured syncTo offset", node.id))?;
                let placed = crate::audio_sync::place_recording(
                    recording,
                    reference,
                    offset,
                    time(node, "sourceIn")?.map(|value| snap_at_rate(value, fps)),
                    time(node, "sourceOut")?.map(|value| snap_at_rate(value, fps)),
                    durations.get(&node.id).copied(),
                )?;
                placements.insert(node.id.clone(), placed);
                timeline = Self::resolve_placed(&plain, durations, probes, fps, &placements)?;
                progressed = true;
            }
            if !progressed {
                return Err("syncTo links cannot form a cycle".into());
            }
            pending = remaining;
        }
        Ok(timeline)
    }

    fn resolve_placed(
        stage: &Node,
        durations: &BTreeMap<String, f64>,
        probes: Option<&BTreeMap<String, crate::media::Probe>>,
        fps: f64,
        placements: &BTreeMap<String, Span>,
    ) -> Result<Self> {
        if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
            return Err("Timeline FPS must be in (0, 240]".into());
        }
        let snap = |seconds| snap_at_rate(seconds, fps);
        if stage.tag != "stage" {
            return Err("The document root must be <stage>".into());
        }
        let scenes = stage
            .children
            .iter()
            .filter(|node| node.tag == "scene")
            .collect::<Vec<_>>();
        if scenes.is_empty() {
            return Err("The stage has no scene".into());
        }
        // Existing reconciler semantics: the last active root wins; no flag
        // means no active timeline, rather than silently selecting a scene.
        let active = scenes
            .iter()
            .rfind(|node| node.flag("active"))
            .map(|node| node.id.clone())
            .unwrap_or_default();
        let mut timeline = Self {
            spans: BTreeMap::new(),
            workareas: BTreeMap::new(),
            active,
        };
        for scene in scenes {
            let span = timeline
                .resolve_node(scene, None, &scene.id, durations, probes, fps, placements)?;
            let area =
                if let Some(value) = scene.props.get("workarea").filter(|value| !value.is_null()) {
                    let array = value
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or_else(|| format!("{}: workarea must be [start, end]", scene.id))?;
                    [snap(time_value(&array[0])?), snap(time_value(&array[1])?)]
                } else {
                    [0.0, span.end.max(0.0)]
                };
            if area[0] < 0.0 || area[1] <= area[0] {
                return Err(format!("{}: workarea must have 0 <= start < end", scene.id));
            }
            timeline.workareas.insert(scene.id.clone(), area);
        }
        Ok(timeline)
    }

    fn resolve_node(
        &mut self,
        node: &Node,
        parent: Option<&Span>,
        scene: &str,
        durations: &BTreeMap<String, f64>,
        probes: Option<&BTreeMap<String, crate::media::Probe>>,
        fps: f64,
        placements: &BTreeMap<String, Span>,
    ) -> Result<Span> {
        let snap = |seconds| snap_at_rate(seconds, fps);
        if node.props.contains_key("syncTo") {
            return Err(format!(
                "{}: native audio syncTo resolution is not implemented yet",
                node.id
            ));
        }
        let rate = node.number("playbackRate", 1.0)?;
        if rate <= 0.0 {
            return Err(format!("{}: playbackRate must be positive", node.id));
        }
        let parent_rate = parent.map_or(1.0, |p| p.playback_rate);
        let parent_origin = parent.map_or(0.0, |p| p.origin);
        let start = snap(time(node, "start")?.unwrap_or(0.0));
        let source_in = snap(time(node, "sourceIn")?.unwrap_or(0.0));
        let authored_out = time(node, "sourceOut")?.map(snap);
        let timeline_end = time(node, "end")?.map(snap);
        let mut source_out = authored_out.unwrap_or(f64::INFINITY);
        if let Some(end) = timeline_end {
            source_out = source_out.min(source_in + (end - start) * rate);
        }
        // Caption transcript length is a fallback, not a cap on authored timing.
        if let Some(duration) = durations.get(&node.id) {
            if !duration.is_finite() || *duration < 0.0 {
                return Err(format!("{}: invalid source duration", node.id));
            }
            if node.tag != "captions" || source_out.is_infinite() {
                source_out = source_out.min(snap(*duration));
            }
        } else if matches!(node.tag.as_str(), "video" | "audio")
            && !probes
                .and_then(|p| p.get(&node.id))
                .is_some_and(|p| p.still)
        {
            return Err(format!(
                "{}: source duration must be probed before resolving media timing",
                node.id
            ));
        }
        let fits = node.is_container()
            && !durations.contains_key(&node.id)
            && authored_out.is_none()
            && timeline_end.is_none();
        if source_out.is_infinite() {
            // The original runtime's fallback is 480 frames, including in a
            // capture world with a different frame rate.
            source_out = DEFAULT_DURATION * TIME_FPS / fps;
        }
        source_out = source_out.max(source_in);
        let origin = parent_origin + (start - source_in / rate) / parent_rate;
        let absolute_rate = rate * parent_rate;
        if !origin.is_finite() || !absolute_rate.is_finite() || absolute_rate <= 0.0 {
            return Err(format!("{}: timing overflows", node.id));
        }
        let mut span = Span {
            id: node.id.clone(),
            parent_id: parent.map(|p| p.id.clone()),
            scene_id: scene.into(),
            origin,
            start: snap(origin + source_in / absolute_rate),
            end: snap(origin + source_out / absolute_rate),
            playback_rate: absolute_rate,
            source_in,
            source_out,
            hidden: node.flag("hidden") || parent.is_some_and(|p| p.hidden),
            muted: node.flag("muted") || parent.is_some_and(|p| p.muted),
            volume_db: node.number("volume", 0.0)? + parent.map_or(0.0, |p| p.volume_db),
        };
        let mut children = Vec::new();
        if let Some(placed) = placements.get(&node.id) {
            span.origin = placed.origin;
            span.start = snap(placed.start);
            span.end = snap(placed.end);
            span.source_in = placed.source_in;
            span.source_out = placed.source_out;
        }
        for child in node.children.iter().filter(|child| child.is_timed()) {
            let resolved = self.resolve_node(
                child,
                Some(&span),
                scene,
                durations,
                probes,
                fps,
                placements,
            )?;
            // Original Cache.children excludes IsMask; masks have their own
            // timing, but do not extend a container fitted to its children.
            if !child.flag("mask") {
                children.push(resolved);
            }
        }
        if fits && !children.is_empty() {
            span.start = children
                .iter()
                .map(|c| c.start)
                .fold(f64::INFINITY, f64::min);
            span.end = children
                .iter()
                .map(|c| c.end)
                .fold(f64::NEG_INFINITY, f64::max);
            span.source_in = span.source_at(span.start);
            span.source_out = span.source_at(span.end);
        }
        self.spans.insert(node.id.clone(), span.clone());
        // Explicit container trims gate all descendants, including nested groups.
        if !fits && node.is_container() {
            for descendant in node.nodes().into_iter().skip(1) {
                if let Some(child) = self.spans.get_mut(&descendant.id) {
                    child.start = child.start.max(span.start);
                    child.end = child.end.min(span.end).max(child.start);
                    child.source_in = child.source_at(child.start);
                    child.source_out = child.source_at(child.end);
                }
            }
        }
        Ok(span)
    }

    pub fn get(&self, id: &str) -> Result<&Span> {
        self.spans
            .get(id)
            .ok_or_else(|| format!("No timed node with id {id:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn times_are_30fps_and_reject_non_finite_or_partial_input() {
        assert_eq!(parse_time("45f").unwrap(), 1.5);
        assert_eq!(parse_time("-01:02:03.5").unwrap(), -3723.5);
        assert_eq!(snap(-0.5 / 30.0), 0.0);
        for value in ["", "NaN", "inf", "1garbage", "1::2", "1:NaN", "1e999"] {
            assert!(parse_time(value).is_err());
        }
    }
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Authored text styles and UTF-16 ranges, followed by native shaping/layout.
use crate::{
    Result,
    document::Node,
    encoder::cancelled,
    fonts::{FontBook, FontSpec, Shaped},
    text_layout::{self, Align, Baseline, Layout, Piece},
    timing::round_frame,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

pub(crate) type TextSizes = BTreeMap<String, [Option<f64>; 2]>;
pub(crate) type PreparedTexts = BTreeMap<String, PreparedText>;
pub(crate) struct Word {
    pub shaped: Arc<Shaped>,
    pub color: Option<u32>,
    pub fills: Option<String>,
    pub strokes: Option<String>,
    newline: bool,
}
pub(crate) struct PreparedText {
    pub authored_size: [Option<f64>; 2],
    pub animated_size: [Option<f64>; 2],
    pub words: Vec<Word>,
    leading: f64,
    align: Align,
    baseline: Baseline,
}
impl PreparedText {
    pub fn layout(&self, size: [f64; 2], authored_size: Option<[f64; 2]>) -> Result<Layout> {
        let pieces = self
            .words
            .iter()
            .map(|word| Piece {
                metrics: word.shaped.metrics,
                newline_after: word.newline,
            })
            .collect::<Vec<_>>();
        text_layout::layout(
            &pieces,
            authored_size.map_or(f64::INFINITY, |s| s[0]),
            authored_size.is_some(),
            size,
            self.leading,
            self.align,
            self.baseline,
        )
    }
}

#[derive(Clone, Default)]
struct Style {
    font: FontSpec,
    color: Option<u32>,
    case: Case,
    baseline: Option<Baseline>,
    fills: Option<String>,
    strokes: Option<String>,
}
#[derive(Clone, Copy, Default)]
enum Case {
    #[default]
    Original,
    Upper,
    Lower,
}

pub(crate) fn scalar(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64().filter(|n| n.is_finite()),
        Some(Value::String(s)) => {
            let n = boa_engine::js_string!(s.as_str()).to_number();
            n.is_finite().then_some(n)
        }
        _ => None,
    }
}
pub(crate) fn node_color(node: &Node) -> Option<u32> {
    // Both aliases write the same Color trait in authored property order.
    node.props
        .iter()
        .rfind(|(key, _)| matches!(key.as_str(), "fill" | "color"))
        .and_then(|(_, value)| crate::compositor::packed_color(value).ok())
}
fn baseline(value: Option<&str>) -> Option<Baseline> {
    match value {
        Some("top") => Some(Baseline::Top),
        Some("middle") => Some(Baseline::Middle),
        Some("bottom") => Some(Baseline::Bottom),
        Some("alphabetic") => Some(Baseline::Alphabetic),
        _ => None,
    }
}
impl Style {
    fn apply(&mut self, node: &Node) {
        if node.children.iter().any(|n| crate::paints::is_fill(&n.tag)) {
            self.fills = Some(node.id.clone());
        }
        if node.children.iter().any(|n| n.tag == "stroke") {
            self.strokes = Some(node.id.clone());
        }
        if let Some(value) = node
            .string("fontFamily")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.font.family = value.into();
        }
        if let Some(value) = scalar(node.props.get("fontSize")).filter(|n| *n > 0.0) {
            self.font.size = round_frame(value);
        }
        let weight = match node.string("fontWeight").map(str::trim) {
            Some("") => None,
            Some("normal") => Some(400.0),
            Some("bold") => Some(700.0),
            _ => scalar(node.props.get("fontWeight")),
        };
        if let Some(value) = weight {
            self.font.weight = value;
        }
        if let Some(value) = match node.string("fontStyle") {
            Some("normal") => Some(fontdb::Style::Normal),
            Some("italic") => Some(fontdb::Style::Italic),
            Some("oblique") => Some(fontdb::Style::Oblique),
            _ => None,
        } {
            self.font.style = value;
        }
        if let Some(value) = scalar(node.props.get("letterSpacing")) {
            self.font.spacing = value;
        }
        if let Some(value) = match node.string("textCase") {
            Some("original") => Some(Case::Original),
            Some("upper") => Some(Case::Upper),
            Some("lower") => Some(Case::Lower),
            _ => None,
        } {
            self.case = value;
        }
        if let Some(value) = node_color(node) {
            self.color = Some(value);
        }
        if let Some(value) = baseline(node.string("textBaseline")) {
            self.baseline = Some(value);
        }
    }
}

/// Authored and computed size descriptors do not load fonts or measure text.
pub(crate) fn prepare(scene: &Node, authored: &Node, animated: &TextSizes) -> PreparedTexts {
    let originals = authored
        .nodes()
        .into_iter()
        .map(|n| (n.id.as_str(), n))
        .collect::<BTreeMap<_, _>>();
    scene
        .nodes()
        .into_iter()
        .filter(|node| node.tag == "text")
        .map(|node| {
            let original = originals.get(node.id.as_str()).copied().unwrap_or(node);
            (
                node.id.clone(),
                PreparedText {
                    authored_size: [
                        scalar(original.props.get("width")),
                        scalar(original.props.get("height")),
                    ],
                    animated_size: animated.get(&node.id).copied().unwrap_or([None; 2]),
                    words: Vec::new(),
                    leading: scalar(node.props.get("leading"))
                        .filter(|n| *n > 0.0)
                        .unwrap_or(1.0),
                    align: match node.string("textAlign") {
                        Some("center") => Align::Center,
                        Some("right") => Align::Right,
                        _ => Align::Left,
                    },
                    baseline: baseline(node.string("textBaseline")).unwrap_or(Baseline::Top),
                },
            )
        })
        .collect()
}
pub(crate) struct TextWork {
    points: usize,
    pieces: usize,
    text_bytes: usize,
    font_budget: usize,
}
impl Default for TextWork {
    fn default() -> Self {
        Self {
            points: 0,
            pieces: 0,
            text_bytes: 0,
            font_budget: 65536,
        }
    }
}
/// Called only after traversal has resolved visibility and culling. Failures
/// remain transient and no compositor history is committed until every draw is ready.
pub(crate) fn shape(
    node: &Node,
    prepared: &mut PreparedText,
    fonts: &mut FontBook,
    work: &mut TextWork,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let mut base = Style::default();
    base.apply(node);
    prepared.words.clear();
    for (chars, style) in splits(node, &base)? {
        for token in text_layout::tokenize(&chars) {
            let mut parts = token.split('\n').peekable();
            while let Some(part) = parts.next() {
                cancelled(cancel)?;
                let chars = match style.case {
                    Case::Original => part.to_owned(),
                    Case::Upper => part.to_uppercase(),
                    Case::Lower => part.to_lowercase(),
                };
                work.text_bytes = work.text_bytes.saturating_add(chars.len());
                if work.text_bytes > 4 * 1024 * 1024 {
                    return Err("Scene text exceeds 4 MiB".into());
                }
                let shaped = fonts
                    .shape_bounded(
                        &chars,
                        &style.font,
                        style.baseline.unwrap_or(Baseline::Top),
                        cancel,
                        &mut work.font_budget,
                    )
                    .map_err(|e| format!("{}: {e}", node.id))?;
                work.points = work
                    .points
                    .saturating_add(shaped.path.as_ref().map_or(0, |p| p.points().len()));
                work.pieces += 1;
                if work.points > 2_000_000 || work.pieces > 100_000 {
                    return Err(
                        "Scene text exceeds two million outline points or 100000 pieces".into(),
                    );
                }
                prepared.words.push(Word {
                    shaped,
                    color: style.color,
                    fills: style.fills.clone(),
                    strokes: style.strokes.clone(),
                    newline: parts.peek().is_some(),
                });
            }
        }
    }
    Ok(())
}

fn splits(node: &Node, base: &Style) -> Result<Vec<(String, Style)>> {
    if node.text.len() > 1024 * 1024 {
        return Err("Text content exceeds 1 MiB".into());
    }
    let ranges = node
        .children
        .iter()
        .filter(|n| n.tag == "textRange")
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return Ok(vec![(node.text.clone(), base.clone())]);
    }
    if ranges.len() > 2048 {
        return Err("Text accepts at most 2048 style ranges".into());
    }
    let utf16 = node.text.encode_utf16().collect::<Vec<_>>();
    let len = utf16.len() as f64;
    let ranges = ranges
        .into_iter()
        .map(|n| {
            (
                n,
                scalar(n.props.get("start"))
                    .unwrap_or(0.0)
                    .trunc()
                    .clamp(0.0, len),
                scalar(n.props.get("end"))
                    .unwrap_or(len)
                    .trunc()
                    .clamp(0.0, len),
            )
        })
        .collect::<Vec<_>>();
    let mut boundaries = vec![0.0, len];
    for (_, start, end) in &ranges {
        boundaries.extend([*start, *end]);
    }
    boundaries.sort_by(f64::total_cmp);
    boundaries.dedup();
    let mut result = Vec::new();
    for pair in boundaries.windows(2) {
        let [start, end] = [pair[0], pair[1]];
        if start >= end {
            continue;
        }
        let mut style = base.clone();
        for (range, a, b) in &ranges {
            if *a <= start && *b > start {
                style.apply(range);
            }
        }
        result.push((
            String::from_utf16_lossy(&utf16[start as usize..end as usize]),
            style,
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_range_indices_truncate_in_the_reconciler_before_segmentation() {
        let node = text(
            "AB<textRange start={0.8} end={1.8} color=\"blue\"/><textRange start={1.2} end={1.9} color=\"lime\"/>",
        );
        let mut base = Style::default();
        base.apply(&node);
        let parts = splits(&node, &base).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "A");
        assert_eq!(parts[0].1.color, Some(0x0000ff));
        assert_eq!(parts[1].0, "B");
        assert_eq!(parts[1].1.color, Some(0xff0000));
    }
    #[test]
    fn nonscalar_colors_remove_the_trait_and_ranges_keep_previous_intrinsic_ink() {
        let mut node = text("A<textRange color=\"blue\" fill={[1,2]}/>");
        let mut base = Style::default();
        base.apply(&node);
        assert_eq!(splits(&node, &base).unwrap()[0].1.color, Some(0xff0000));
        node.props
            .insert("fill".into(), serde_json::json!({"color":"blue"}));
        assert_eq!(node_color(&node), None);
        let mut style = Style::default();
        style.apply(&node);
        assert_eq!(style.color, None);
    }
    fn text(body: &str) -> Node {
        let doc=crate::source::SourceDocument::parse("text.tsx".into(),format!("export default <stage><scene><text color=\"red\" fontSize={{32}}>{body}</text></scene></stage>")).unwrap();
        doc.root.children[0].children[0].clone()
    }
    #[test]
    fn overlapping_ranges_use_utf16_boundaries_and_document_order() {
        let node = text(
            "A😀BC<textRange start={1} end={4} color=\"blue\"/><textRange start={3} end={5} color=\"lime\" fontWeight=\"bold\"/>",
        );
        let mut base = Style::default();
        base.apply(&node);
        let parts = splits(&node, &base).unwrap();
        assert_eq!(
            parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
            ["A", "😀", "B", "C"]
        );
        assert_eq!(
            parts.iter().map(|p| p.1.color).collect::<Vec<_>>(),
            [
                Some(0xff0000),
                Some(0x0000ff),
                Some(0x00ff00),
                Some(0x00ff00)
            ]
        );
        assert_eq!(parts[2].1.font.weight, 700.0);
        assert_eq!(parts[3].1.font.size, 32.0);
    }
    #[test]
    fn style_ranges_can_split_surrogates_and_empty_styled_text_stays_finite() {
        let node = text("😀<textRange start={1} end={2}/>");
        let parts = splits(&node, &Style::default()).unwrap();
        assert_eq!(
            parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
            ["�", "�"]
        );
        let node = text("<textRange start={0}/>");
        assert!(splits(&node, &Style::default()).unwrap().is_empty());
    }
    #[test]
    fn style_numbers_aliases_and_invalid_range_overrides_follow_reconciler() {
        let node = text(
            "A<textRange fontSize=\"12.6\" fontWeight=\"normal\" color={null} letterSpacing=\"-2\"/>",
        );
        let mut base = Style::default();
        base.apply(&node);
        let parts = splits(&node, &base).unwrap();
        assert_eq!(parts[0].1.font.size, 13.0);
        assert_eq!(parts[0].1.font.spacing, -2.0);
        assert_eq!(parts[0].1.color, Some(0xff0000));
    }
}

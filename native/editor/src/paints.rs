// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Paint sub-entities share geometry with their holder, never its child layers.
use crate::{Result, document::Node, text::scalar};
use tiny_skia::{
    BlendMode, Color, GradientStop, LineCap, LineJoin, Point, Shader, SpreadMode, Stroke, Transform,
};

pub(crate) fn is_fill(tag: &str) -> bool {
    matches!(
        tag,
        "solidPaint" | "linearGradientPaint" | "radialGradientPaint"
    )
}
pub(crate) fn is_style(tag: &str) -> bool {
    is_fill(tag) || matches!(tag, "stroke" | "colorStop" | "textRange")
}
pub(crate) fn number(node: &Node, key: &str, default: f64) -> f64 {
    scalar(node.props.get(key)).unwrap_or(default)
}
pub(crate) fn color(node: &Node) -> u32 {
    crate::text::node_color(node).unwrap_or(0)
}
pub(crate) fn base_scale(node: &Node) -> [f64; 2] {
    let mut axis = [1.0; 2];
    let mut computed = axis;
    for (key, value) in &node.props {
        match key.as_str() {
            "scale" => computed = scalar(Some(value)).map_or(axis, |v| [v; 2]),
            "scaleX" | "scaleY" => {
                axis[usize::from(key == "scaleY")] = scalar(Some(value)).unwrap_or(1.0);
                computed = axis;
            }
            _ => {}
        }
    }
    computed
}
pub(crate) fn blend(node: &Node, inherited: BlendMode) -> BlendMode {
    match node.string("blendMode") {
        Some("multiply") => BlendMode::Multiply,
        Some("screen") => BlendMode::Screen,
        Some("overlay") => BlendMode::Overlay,
        Some("darken") => BlendMode::Darken,
        Some("lighten") => BlendMode::Lighten,
        Some("colorDodge") => BlendMode::ColorDodge,
        Some("colorBurn") => BlendMode::ColorBurn,
        Some("hardLight") => BlendMode::HardLight,
        Some("softLight") => BlendMode::SoftLight,
        Some("difference") => BlendMode::Difference,
        Some("exclusion") => BlendMode::Exclusion,
        Some("hue") => BlendMode::Hue,
        Some("saturation") => BlendMode::Saturation,
        Some("color") => BlendMode::Color,
        Some("luminosity") => BlendMode::Luminosity,
        // The reconciler removes sourceOver/unknown BlendMode traits, so the
        // saved Canvas mode remains in effect rather than explicitly resetting.
        _ => inherited,
    }
}
pub(crate) fn assigned_alpha(current: f64, requested: f64) -> f64 {
    if (0.0..=1.0).contains(&requested) {
        requested
    } else {
        current
    }
}
fn rgba(rgb: u32, opacity: f64) -> Color {
    Color::from_rgba(
        (rgb >> 16) as u8 as f32 / 255.0,
        (rgb >> 8) as u8 as f32 / 255.0,
        rgb as u8 as f32 / 255.0,
        opacity.clamp(0.0, 1.0) as f32,
    )
    .expect("finite normalized paint color")
}

#[derive(Clone, Debug)]
pub(crate) struct Stop {
    pub offset: f64,
    pub color: u32,
    pub opacity: f64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum GradientGeometry {
    Linear { start: [f64; 2], end: [f64; 2] },
    Radial { center: [f64; 2], radius: f64 },
}
#[derive(Clone, Debug)]
pub(crate) struct Gradient {
    pub geometry: GradientGeometry,
    pub stops: Vec<Stop>,
}
impl Gradient {
    pub fn from_node(node: &Node, authored: &Node, size: [f64; 2]) -> Result<Self> {
        let positioned = authored.props.contains_key("x") || authored.props.contains_key("y");
        let px = if positioned {
            number(node, "x", 0.0)
        } else {
            0.5
        };
        let py = if positioned {
            number(node, "y", 0.0)
        } else {
            0.5
        };
        let [sx, sy] = base_scale(node);
        let angle = number(node, "rotation", 0.0).to_radians();
        let (sin, cos) = angle.sin_cos();
        let point = |x: f64, y: f64| {
            [
                (px + x * sx * cos - y * sy * sin) * size[0],
                (py + x * sx * sin + y * sy * cos) * size[1],
            ]
        };
        let geometry = if node.tag == "linearGradientPaint" {
            GradientGeometry::Linear {
                start: point(-0.5, 0.0),
                end: point(0.5, 0.0),
            }
        } else {
            let center = [px * size[0], py * size[1]];
            let x = point(0.5, 0.0);
            let y = point(0.0, 0.5);
            GradientGeometry::Radial {
                center,
                radius: (x[0] - center[0])
                    .hypot(x[1] - center[1])
                    .max((y[0] - center[0]).hypot(y[1] - center[1]))
                    .max(0.0001),
            }
        };
        let values = match geometry {
            GradientGeometry::Linear { start, end } => vec![start[0], start[1], end[0], end[1]],
            GradientGeometry::Radial { center, radius } => vec![center[0], center[1], radius],
        };
        if values
            .iter()
            .any(|n| !n.is_finite() || !(*n as f32).is_finite())
        {
            return Err(format!(
                "{}: gradient exceeds the finite raster coordinate range",
                node.id
            ));
        }
        let mut stops = Vec::new();
        for child in &node.children {
            if child.tag != "colorStop" {
                return Err(format!(
                    "{}: gradient accepts only colorStop children",
                    node.id
                ));
            }
            if stops.len() >= 4096 {
                return Err("A gradient accepts at most 4096 color stops".into());
            }
            let raw = number(child, "offset", 0.0);
            stops.push(Stop {
                offset: if raw <= 1.0 { raw.max(0.0) } else { raw % 1.0 },
                color: color(child),
                opacity: number(child, "opacity", 1.0).clamp(0.0, 1.0),
            });
        }
        // Stable ordering matters when multiple stops occupy the same offset.
        stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        Ok(Self { geometry, stops })
    }
    pub fn shader(&self, opacity: f64, origin: [f64; 2]) -> Result<Option<Shader<'static>>> {
        if self.stops.is_empty() {
            return Ok(None);
        }
        // Canvas paints nothing for a zero-length linear gradient, even with
        // one stop. tiny-skia's degenerate fallback paints its final color.
        if let GradientGeometry::Linear { start, end } = self.geometry
            && start == end
        {
            return Ok(None);
        }
        let stops = self
            .stops
            .iter()
            .map(|s| GradientStop::new(s.offset as f32, rgba(s.color, s.opacity * opacity)))
            .collect();
        let transform = Transform::from_translate(-origin[0] as f32, -origin[1] as f32);
        let point = |p: [f64; 2]| Point::from_xy(p[0] as f32, p[1] as f32);
        let shader = match self.geometry {
            GradientGeometry::Linear { start, end } => tiny_skia::LinearGradient::new(
                point(start),
                point(end),
                stops,
                SpreadMode::Pad,
                transform,
            ),
            GradientGeometry::Radial { center, radius } => tiny_skia::RadialGradient::new(
                point(center),
                0.0,
                point(center),
                radius as f32,
                stops,
                SpreadMode::Pad,
                transform,
            ),
        };
        shader
            .map(Some)
            .ok_or_else(|| "Native gradient could not be represented".into())
    }
}

/// Canvas ignores nonpositive width/miter assignments, retaining its previous
/// line state. Shapes pass that state to children; text saves/restores its pass.
#[derive(Clone, Debug)]
pub(crate) struct LineStyle(Stroke);
impl Default for LineStyle {
    fn default() -> Self {
        Self(Stroke {
            miter_limit: 10.0,
            ..Stroke::default()
        })
    }
}
impl LineStyle {
    pub fn apply(&mut self, node: &Node) -> Result<Stroke> {
        let width = number(node, "width", 1.0);
        let miter = number(node, "miterLimit", 10.0);
        if width > 8192.0 || miter > 1_000_000.0 {
            return Err("Stroke width/miter exceeds native rendering limits".into());
        }
        if width > 0.0 {
            // Zero means a device-space hairline to tiny-skia, whereas Canvas
            // retains an arbitrarily thin positive requested line width.
            self.0.width = (width as f32).max(f32::MIN_POSITIVE);
        }
        if miter > 0.0 {
            self.0.miter_limit = (miter as f32).max(f32::MIN_POSITIVE);
        }
        self.0.line_join = match node.string("join") {
            Some("round") => LineJoin::Round,
            Some("bevel") => LineJoin::Bevel,
            _ => LineJoin::Miter,
        };
        self.0.line_cap = match node.string("cap") {
            Some("round") => LineCap::Round,
            Some("square") => LineCap::Square,
            _ => LineCap::Butt,
        };
        Ok(self.0.clone())
    }
}

pub(crate) fn solid_shader(color: u32, opacity: f64) -> Shader<'static> {
    Shader::SolidColor(rgba(color, opacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn node(tag: &str, props: Value) -> Node {
        Node {
            id: tag.into(),
            tag: tag.into(),
            props: props.as_object().unwrap().clone(),
            text: String::new(),
            children: Vec::new(),
            location: Default::default(),
        }
    }
    #[test]
    fn gradient_geometry_and_stops_match_original_runtime_functions() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gradients.json")).unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 288);
        for case in cases {
            let mut source = node(
                if case["kind"] == "linear" {
                    "linearGradientPaint"
                } else {
                    "radialGradientPaint"
                },
                case["props"].clone(),
            );
            source.children = case["stops"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| node("colorStop", v.clone()))
                .collect();
            let gradient = Gradient::from_node(
                &source,
                &source,
                [
                    case["size"][0].as_f64().unwrap(),
                    case["size"][1].as_f64().unwrap(),
                ],
            )
            .unwrap();
            let actual = match gradient.geometry {
                GradientGeometry::Linear { start, end } => vec![start[0], start[1], end[0], end[1]],
                GradientGeometry::Radial { center, radius } => {
                    vec![center[0], center[1], 0.0, center[0], center[1], radius]
                }
            };
            for (actual, expected) in actual
                .iter()
                .zip(case["result"]["points"].as_array().unwrap())
            {
                let expected = expected.as_f64().unwrap();
                assert!(
                    (actual - expected).abs() < 1e-9 * expected.abs().max(1.0),
                    "{case}: {actual} != {expected}"
                );
            }
            for (actual, expected) in gradient
                .stops
                .iter()
                .zip(case["result"]["stops"].as_array().unwrap())
            {
                assert_eq!(actual.offset, expected["offset"].as_f64().unwrap());
                let css = if actual.opacity >= 1.0 {
                    format!("#{:06X}", actual.color)
                } else {
                    format!(
                        "rgba({},{},{},{})",
                        (actual.color >> 16) & 255,
                        (actual.color >> 8) & 255,
                        actual.color & 255,
                        actual.opacity
                    )
                };
                assert_eq!(css, expected["color"].as_str().unwrap());
            }
        }
    }
    #[test]
    fn style_fallbacks_inherit_canvas_state_and_keep_alias_order() {
        assert_eq!(
            blend(
                &node("solidPaint", json!({"blendMode":"sourceOver"})),
                BlendMode::Multiply
            ),
            BlendMode::Multiply
        );
        assert_eq!(assigned_alpha(0.25, -0.5), 0.25);
        let mut line = LineStyle::default();
        assert_eq!(
            line.apply(&node("stroke", json!({"width":6,"miterLimit":12})))
                .unwrap()
                .width,
            6.0
        );
        let stroke = line
            .apply(&node(
                "stroke",
                json!({"width":0,"miterLimit":-1,"join":"unknown"}),
            ))
            .unwrap();
        assert_eq!(stroke.width, 6.0);
        assert_eq!(stroke.miter_limit, 12.0);
        assert_eq!(stroke.line_join, LineJoin::Miter);
        assert!(line.apply(&node("stroke", json!({"width":10000}))).is_err());
        let source = node("solidPaint", json!({"color":"red","fill":null}));
        assert_eq!(color(&source), 0);
    }
}

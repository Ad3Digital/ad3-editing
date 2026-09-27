// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Metrics {
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
    pub left: f64,
    pub right: f64,
    pub ink_ascent: f64,
    pub ink_descent: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Baseline {
    Top,
    Middle,
    Bottom,
    Alphabetic,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Piece {
    pub metrics: Metrics,
    pub newline_after: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Positioned {
    pub piece: usize,
    pub line: usize,
    pub x: i32,
    pub y: i32,
    pub bounds: [i32; 4],
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Layout {
    pub size: [f64; 2],
    pub tokens: Vec<Positioned>,
}

#[derive(Default)]
struct LineData {
    pieces: Vec<usize>,
    width: f64,
    height: f64,
    max_ascent: Option<f64>,
}

/// Splits only on ASCII spaces, retaining every matched trailing space.
/// Newlines remain in the returned pieces for the caller to split explicitly.
pub fn tokenize(text: &str) -> Vec<&str> {
    if !text.as_bytes().contains(&b' ') {
        return vec![text];
    }

    let mut tokens = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b' ' {
            tokens.push(&text[start..=index]);
            start = index + 1;
        }
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

pub fn layout(
    pieces: &[Piece],
    max_width: f64,
    authored_size: bool,
    computed_size: [f64; 2],
    leading: f64,
    align: Align,
    baseline: Baseline,
) -> crate::Result<Layout> {
    if pieces.len() > 100_000 {
        return Err("text layout accepts at most 100000 pieces".into());
    }
    if max_width.is_nan() {
        return Err("max_width must not be NaN".into());
    }
    finite(computed_size[0], "computed width")?;
    finite(computed_size[1], "computed height")?;
    if !leading.is_finite() || leading <= 0.0 {
        return Err("leading must be finite and greater than zero".into());
    }

    for piece in pieces {
        validate_metrics(piece.metrics)?;
    }

    let autosize = !authored_size || computed_size[0] == 0.0 || computed_size[1] == 0.0;
    if pieces.is_empty() {
        return Ok(Layout {
            size: if autosize { [0.0, 0.0] } else { computed_size },
            tokens: Vec::new(),
        });
    }

    let mut lines = vec![LineData::default()];
    let mut offset = 0.0;

    for (index, piece) in pieces.iter().enumerate() {
        let metrics = piece.metrics;
        let proposed_offset = add(offset, metrics.width, "line width")?;
        if proposed_offset > max_width && offset > 0.0 {
            lines.push(LineData::default());
        }

        let line = lines.last_mut().expect("text layout always has a line");
        line.pieces.push(index);
        line.width = add(line.width, metrics.width, "line width")?;
        offset = line.width;

        let piece_height = add(metrics.ascent, metrics.descent, "piece height")?;
        if piece_height > line.height {
            line.height = piece_height;
        }
        line.max_ascent = Some(match line.max_ascent {
            Some(current) => current.max(metrics.ascent),
            None => metrics.ascent,
        });

        if piece.newline_after {
            lines.push(LineData::default());
            offset = 0.0;
        }
    }

    let mut max_line_width = lines[0].width;
    let mut total_height = 0.0;
    for (index, line) in lines.iter().enumerate() {
        max_line_width = max_line_width.max(line.width);
        let factor = if index + 1 == lines.len() {
            1.0
        } else {
            leading
        };
        total_height = add(total_height, line.height * factor, "total height")?;
    }

    let size = if autosize {
        [
            finite(max_line_width.ceil(), "layout width")?,
            finite(total_height.ceil(), "layout height")?,
        ]
    } else {
        computed_size
    };

    let container_width = size[0];
    let container_height = size[1];
    let mut offset_y = match baseline {
        Baseline::Middle => finite((container_height - total_height) / 2.0, "vertical offset")?,
        Baseline::Bottom => sub(container_height, total_height, "vertical offset")?,
        Baseline::Top | Baseline::Alphabetic => 0.0,
    };

    let mut tokens = Vec::with_capacity(pieces.len());
    for (line_index, line) in lines.iter().enumerate() {
        let offset_x = match align {
            Align::Left => 0.0,
            Align::Center => finite((container_width - line.width) / 2.0, "horizontal offset")?,
            Align::Right => sub(container_width, line.width, "horizontal offset")?,
        };
        let line_baseline = match baseline {
            Baseline::Top => 0.0,
            Baseline::Middle => line.height / 2.0,
            Baseline::Bottom => line.height,
            Baseline::Alphabetic => match line.max_ascent {
                Some(ascent) if ascent != 0.0 => ascent,
                _ => line.height * 0.75,
            },
        };
        finite(line_baseline, "line baseline")?;

        let mut piece_offset = 0.0;
        for &piece_index in &line.pieces {
            let metrics = pieces[piece_index].metrics;
            let x = add(piece_offset, offset_x, "token x")?;
            let y = add(offset_y, line_baseline, "token y")?;
            let left = sub(x, metrics.left, "token left")?;
            let right = add(x, metrics.right, "token right")?;
            let top = sub(y, metrics.ink_ascent, "token top")?;
            let bottom = add(y, metrics.ink_descent, "token bottom")?;

            tokens.push(Positioned {
                piece: piece_index,
                line: line_index,
                x: to_int32(x),
                y: to_int32(y),
                bounds: [
                    to_int32(left),
                    to_int32(top),
                    to_int32(right),
                    to_int32(bottom),
                ],
            });
            piece_offset = add(piece_offset, metrics.width, "token offset")?;
        }

        if line_index + 1 != lines.len() {
            offset_y = add(offset_y, line.height * leading, "next line offset")?;
        }
    }

    Ok(Layout { size, tokens })
}

fn validate_metrics(metrics: Metrics) -> crate::Result<()> {
    for (value, name) in [
        (metrics.width, "metric width"),
        (metrics.ascent, "metric ascent"),
        (metrics.descent, "metric descent"),
        (metrics.left, "metric left"),
        (metrics.right, "metric right"),
        (metrics.ink_ascent, "metric ink ascent"),
        (metrics.ink_descent, "metric ink descent"),
    ] {
        finite(value, name)?;
    }
    Ok(())
}

fn finite(value: f64, name: &str) -> crate::Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{name} must be finite"))
    }
}

fn add(left: f64, right: f64, name: &str) -> crate::Result<f64> {
    finite(left + right, name)
}

fn sub(left: f64, right: f64, name: &str) -> crate::Result<f64> {
    finite(left - right, name)
}

/// ECMAScript's finite-number ToInt32 conversion, including signed wrapping.
fn to_int32(value: f64) -> i32 {
    let unsigned = value.trunc().rem_euclid(4_294_967_296.0) as u64;
    if unsigned >= 2_147_483_648 {
        (unsigned as i64 - 4_294_967_296) as i32
    } else {
        unsigned as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(width: f64) -> Metrics {
        Metrics {
            width,
            ascent: 7.0,
            descent: 3.0,
            left: 1.0,
            right: 2.0,
            ink_ascent: 6.0,
            ink_descent: 2.0,
        }
    }

    fn piece(width: f64, newline_after: bool) -> Piece {
        Piece {
            metrics: metrics(width),
            newline_after,
        }
    }

    #[test]
    fn tokenizes_ascii_spaces_like_the_original_regex() {
        assert_eq!(tokenize(""), vec![""]);
        assert_eq!(tokenize("alpha beta "), vec!["alpha ", "beta "]);
        assert_eq!(tokenize("a  b"), vec!["a ", " ", "b"]);
        assert_eq!(tokenize("a\nb c"), vec!["a\nb ", "c"]);
    }

    #[test]
    fn wraps_only_after_a_nonempty_offset() {
        let result = layout(
            &[piece(4.0, false), piece(4.0, false)],
            6.0,
            false,
            [99.0, 99.0],
            1.0,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();

        assert_eq!(result.size, [4.0, 20.0]);
        assert_eq!(result.tokens[0].line, 0);
        assert_eq!(result.tokens[1].line, 1);
        assert_eq!(result.tokens[1].x, 0);
        assert_eq!(result.tokens[1].y, 10);
    }

    #[test]
    fn explicit_newline_preserves_a_final_empty_line() {
        let result = layout(
            &[piece(2.0, true), piece(0.0, false)],
            f64::INFINITY,
            false,
            [0.0, 0.0],
            1.0,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();

        assert_eq!(result.size, [2.0, 20.0]);
        assert_eq!(result.tokens[0].line, 0);
        assert_eq!(result.tokens[1].line, 1);
    }

    #[test]
    fn authored_box_does_not_change_authored_wrap_width() {
        let result = layout(
            &[piece(4.0, false), piece(4.0, false)],
            6.0,
            true,
            [20.0, 20.0],
            1.0,
            Align::Center,
            Baseline::Top,
        )
        .unwrap();

        assert_eq!(result.size, [20.0, 20.0]);
        assert_eq!(result.tokens[0].line, 0);
        assert_eq!(result.tokens[1].line, 1);
        assert_eq!(result.tokens[0].x, 8);
        assert_eq!(result.tokens[1].x, 8);
    }

    #[test]
    fn supports_all_baseline_formulas() {
        let input = [piece(2.0, false)];
        let top = layout(
            &input,
            f64::INFINITY,
            true,
            [10.0, 30.0],
            1.0,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();
        let middle = layout(
            &input,
            f64::INFINITY,
            true,
            [10.0, 30.0],
            1.0,
            Align::Left,
            Baseline::Middle,
        )
        .unwrap();
        let bottom = layout(
            &input,
            f64::INFINITY,
            true,
            [10.0, 30.0],
            1.0,
            Align::Left,
            Baseline::Bottom,
        )
        .unwrap();
        let alphabetic = layout(
            &input,
            f64::INFINITY,
            true,
            [10.0, 30.0],
            1.0,
            Align::Left,
            Baseline::Alphabetic,
        )
        .unwrap();

        assert_eq!(top.tokens[0].y, 0);
        assert_eq!(top.tokens[0].bounds, [-1, -6, 2, 2]);
        assert_eq!(middle.tokens[0].y, 15);
        assert_eq!(bottom.tokens[0].y, 30);
        assert_eq!(alphabetic.tokens[0].y, 7);
    }

    #[test]
    fn applies_leading_to_every_line_except_the_last() {
        let result = layout(
            &[piece(2.0, true), piece(2.0, false)],
            f64::INFINITY,
            false,
            [0.0, 0.0],
            1.5,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();

        assert_eq!(result.size, [2.0, 25.0]);
        assert_eq!(result.tokens[1].y, 15);
    }

    #[test]
    fn permits_negative_widths_and_finite_empty_layouts() {
        let negative = layout(
            &[piece(-2.0, false)],
            -1.0,
            false,
            [0.0, 0.0],
            1.0,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();
        assert_eq!(negative.size, [-2.0, 10.0]);

        let empty = layout(
            &[],
            f64::INFINITY,
            false,
            [9.0, 9.0],
            1.0,
            Align::Left,
            Baseline::Top,
        )
        .unwrap();
        assert_eq!(empty.size, [0.0, 0.0]);
        assert!(empty.tokens.is_empty());
    }

    #[test]
    fn uses_signed_js_int32_wrapping_for_coordinates() {
        let result = layout(
            &[piece(1.0, false)],
            f64::INFINITY,
            true,
            [4_294_967_297.0, 10.0],
            1.0,
            Align::Right,
            Baseline::Top,
        )
        .unwrap();

        assert_eq!(result.tokens[0].x, 0);
        assert_eq!(result.tokens[0].bounds[0], -1);
    }

    #[test]
    fn rejects_invalid_geometry() {
        let mut invalid = metrics(1.0);
        invalid.width = f64::NAN;
        assert!(
            layout(
                &[Piece {
                    metrics: invalid,
                    newline_after: false
                }],
                f64::INFINITY,
                false,
                [0.0, 0.0],
                1.0,
                Align::Left,
                Baseline::Top,
            )
            .is_err()
        );
        assert!(
            layout(
                &[],
                f64::NAN,
                false,
                [0.0, 0.0],
                1.0,
                Align::Left,
                Baseline::Top
            )
            .is_err()
        );
        assert!(
            layout(
                &[],
                f64::INFINITY,
                false,
                [0.0, 0.0],
                0.0,
                Align::Left,
                Baseline::Top
            )
            .is_err()
        );
    }
}

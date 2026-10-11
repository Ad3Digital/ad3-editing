// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Contact-sheet geometry shared by native media grabs and future multi-captures.
use crate::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

pub const MAX_FRAMES_PER_SHEET: usize = 12;
pub const SHEET_MAX_WIDTH: u32 = 2576;
pub const SHEET_MAX_HEIGHT: u32 = 1456;
pub const MARGIN: u32 = 4;
pub const GUTTER: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetPlan {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub width: u32,
    pub height: u32,
}

impl SheetPlan {
    pub fn cell_position(self, index: usize) -> Option<[u32; 2]> {
        if self.columns == 0
            || self.rows == 0
            || self.columns > 12
            || self.rows > 12
            || self.cell_width > SHEET_MAX_WIDTH
            || self.cell_height > SHEET_MAX_HEIGHT
        {
            return None;
        }
        let index = u32::try_from(index).ok()?;
        (index < self.columns * self.rows).then(|| {
            [
                MARGIN + (index % self.columns) * (self.cell_width + GUTTER),
                MARGIN + (index / self.columns) * (self.cell_height + GUTTER),
            ]
        })
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        Err("Contact sheet cancelled".into())
    } else {
        Ok(())
    }
}

/// Composite captures in requested order over the original opaque grey gutters.
pub fn compose_sheet(
    frames: &[(&Pixmap, &str)],
    plan: SheetPlan,
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    cancelled(cancel)?;
    if frames.is_empty()
        || frames.len() > MAX_FRAMES_PER_SHEET
        || plan.columns == 0
        || plan.rows == 0
        || plan.columns > 12
        || plan.rows > 12
        || plan.columns * plan.rows < frames.len() as u32
        || plan.cell_width == 0
        || plan.cell_height == 0
        || plan.width > SHEET_MAX_WIDTH
        || plan.height > SHEET_MAX_HEIGHT
        || plan.cell_width > SHEET_MAX_WIDTH
        || plan.cell_height > SHEET_MAX_HEIGHT
        || plan.width != 2 * MARGIN + plan.columns * plan.cell_width + GUTTER * (plan.columns - 1)
        || plan.height != 2 * MARGIN + plan.rows * plan.cell_height + GUTTER * (plan.rows - 1)
    {
        return Err("Invalid contact-sheet composition layout".into());
    }
    let mut canvas = Pixmap::new(plan.width, plan.height).ok_or("Cannot allocate contact sheet")?;
    canvas.fill(Color::from_rgba8(68, 68, 68, 255));
    let image_paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };
    let band_height = (f64::from(plan.cell_height) * 0.06).round().max(30.0);
    let font_size = (band_height * 0.72).round();
    let padding = (band_height * 0.4).round();
    let mut fonts = crate::fonts::FontBook::default();
    let spec = crate::fonts::FontSpec {
        size: font_size,
        ..Default::default()
    };
    let mut white = tiny_skia::Paint::default();
    white.set_color_rgba8(255, 255, 255, 255);
    let mut black = tiny_skia::Paint::default();
    black.set_color_rgba8(0, 0, 0, 255);
    for (index, (frame, label)) in frames.iter().enumerate() {
        cancelled(cancel)?;
        let [x, y] = plan
            .cell_position(index)
            .ok_or("Invalid contact-sheet cell")?;
        canvas.draw_pixmap(
            0,
            0,
            frame.as_ref(),
            &image_paint,
            Transform::from_row(
                plan.cell_width as f32 / frame.width() as f32,
                0.0,
                0.0,
                plan.cell_height as f32 / frame.height() as f32,
                x as f32,
                y as f32,
            ),
            None,
        );
        let shaped = fonts.shape(label, &spec, crate::text_layout::Baseline::Middle, cancel)?;
        if let Some(path) = &shaped.path {
            let transform = Transform::from_translate(
                x as f32 + padding as f32,
                y as f32 + (padding + band_height / 2.0) as f32,
            );
            canvas.stroke_path(
                path,
                &black,
                &tiny_skia::Stroke {
                    width: (font_size / 3.0).round().max(2.0) as f32,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Default::default()
                },
                transform,
                None,
            );
            canvas.fill_path(path, &white, tiny_skia::FillRule::Winding, transform, None);
        }
    }
    cancelled(cancel)?;
    Ok(canvas)
}

/// Largest frame area first, then fewest blanks, then widest row. Never upscale.
pub fn plan_sheet(count: usize, source: [u32; 2]) -> Result<SheetPlan> {
    if !(1..=MAX_FRAMES_PER_SHEET).contains(&count) || source.contains(&0) {
        return Err("A contact sheet requires 1..12 frames and positive source dimensions".into());
    }
    let cells = count as u32;
    let mut best: Option<SheetPlan> = None;
    for columns in 1..=cells {
        let rows = cells.div_ceil(columns);
        let available_width = SHEET_MAX_WIDTH - 2 * MARGIN - GUTTER * (columns - 1);
        let available_height = SHEET_MAX_HEIGHT - 2 * MARGIN - GUTTER * (rows - 1);
        let scale = (available_width as f64 / columns as f64 / source[0] as f64)
            .min(available_height as f64 / rows as f64 / source[1] as f64)
            .min(1.0);
        let cell_width = (source[0] as f64 * scale).floor().max(1.0) as u32;
        let cell_height = (source[1] as f64 * scale).floor().max(1.0) as u32;
        let candidate = SheetPlan {
            columns,
            rows,
            cell_width,
            cell_height,
            width: 2 * MARGIN + columns * cell_width + GUTTER * (columns - 1),
            height: 2 * MARGIN + rows * cell_height + GUTTER * (rows - 1),
        };
        let score = |p: SheetPlan| {
            (
                p.cell_width * p.cell_height,
                std::cmp::Reverse(p.columns * p.rows - cells),
                p.columns,
            )
        };
        if best.is_none_or(|p| score(candidate) > score(p)) {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| "No contact-sheet layout fits".into())
}

/// Balance sheet sizes rather than leaving one small sheet at the end.
pub fn plan_sheet_sizes(total: usize, per_sheet: Option<usize>) -> Result<Vec<usize>> {
    let max = per_sheet.unwrap_or(MAX_FRAMES_PER_SHEET);
    if !(1..=MAX_FRAMES_PER_SHEET).contains(&max) {
        return Err("Frames per sheet must be between 1 and 12".into());
    }
    // Explicit bound covers allocation even when the CLI safety cap is lifted.
    if total == 0 || total > 1_000_000 {
        return Err("Contact-sheet frame count must be between 1 and 1000000".into());
    }
    let sheets = total.div_ceil(max);
    let base = total / sheets;
    let extra = total % sheets;
    Ok((0..sheets).map(|i| base + usize::from(i < extra)).collect())
}

/// Compact filename stamp uses the same frame clock as the filmstrip ruler.
pub fn format_timecode(seconds: f64, frame_rate: f64) -> Result<String> {
    let fixed = crate::filmstrip::format_timestamp(seconds, frame_rate)?;
    let mut compact = String::new();
    for (value, unit) in fixed.split(':').zip(['h', 'm', 's', 'f']) {
        if value.bytes().any(|digit| digit != b'0') {
            compact.push_str(value);
            compact.push(unit);
        }
    }
    Ok(if compact.is_empty() {
        "0f".into()
    } else {
        compact
    })
}

pub fn sheet_timecode(times: &[f64], frame_rate: f64) -> Result<String> {
    if times.is_empty() || times.iter().any(|time| !time.is_finite() || *time < 0.0) {
        return Err("Contact-sheet timestamps must be nonempty, finite and nonnegative".into());
    }
    let first = format_timecode(
        times.iter().copied().fold(f64::INFINITY, f64::min),
        frame_rate,
    )?;
    let last = format_timecode(times.iter().copied().fold(0.0, f64::max), frame_rate)?;
    Ok(if first == last {
        first
    } else {
        format!("{first}-{last}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_preserves_order_alpha_gutters_scaling_and_native_labels() {
        let mut red = Pixmap::new(320, 180).unwrap();
        red.fill(Color::from_rgba8(255, 0, 0, 128));
        let mut blue = Pixmap::new(640, 360).unwrap();
        blue.fill(Color::from_rgba8(0, 0, 255, 255));
        let plan = plan_sheet(2, [320, 180]).unwrap();
        let canvas = compose_sheet(&[(&blue, "08s"), (&red, "0f")], plan, None).unwrap();
        assert_eq!(canvas.pixel(0, 0).unwrap().red(), 68);
        let [x, y] = plan.cell_position(0).unwrap();
        assert_eq!(canvas.pixel(x + 160, y + 100).unwrap().blue(), 255);
        let [x, y] = plan.cell_position(1).unwrap();
        let blended = canvas.pixel(x + 160, y + 100).unwrap();
        assert!((161..=162).contains(&blended.red()));
        assert!((33..=34).contains(&blended.blue()));
        assert_eq!(blended.alpha(), 255);
        assert!(
            canvas
                .pixels()
                .iter()
                .any(|pixel| pixel.red() == 255 && pixel.green() == 255 && pixel.blue() == 255)
        );
        assert!(
            canvas
                .pixels()
                .iter()
                .any(|pixel| pixel.red() == 0 && pixel.green() == 0 && pixel.blue() == 0)
        );
        assert!(compose_sheet(&[(&red, "0f")], plan, Some(&AtomicBool::new(true))).is_err());
        assert!(compose_sheet(&[], plan, None).is_err());
        let invalid = SheetPlan { columns: 0, ..plan };
        assert!(invalid.cell_position(0).is_none());
        assert!(compose_sheet(&[(&red, "0f")], invalid, None).is_err());
    }

    #[test]
    fn balancing_preserves_all_frames_and_sheet_limit() {
        assert_eq!(plan_sheet_sizes(13, None).unwrap(), [7, 6]);
        assert_eq!(plan_sheet_sizes(25, None).unwrap(), [9, 8, 8]);
        assert_eq!(plan_sheet_sizes(13, Some(4)).unwrap(), [4, 3, 3, 3]);
        for total in 1..=200 {
            for max in 1..=12 {
                let sizes = plan_sheet_sizes(total, Some(max)).unwrap();
                assert_eq!(sizes.iter().sum::<usize>(), total);
                assert!(sizes.iter().all(|size| *size > 0 && *size <= max));
                assert!(sizes[0] - sizes[sizes.len() - 1] <= 1);
            }
        }
        for (total, max) in [(0, 12), (1, 0), (1, 13), (usize::MAX, 12)] {
            assert!(plan_sheet_sizes(total, Some(max)).is_err());
        }
    }

    #[test]
    fn layout_matches_original_landscape_portrait_small_and_partial_rows() {
        for (count, grid, cell) in [
            (1, [1, 1], [2568, 1444]),
            (2, [2, 1], [1280, 720]),
            (4, [2, 2], [1280, 720]),
            (5, [3, 2], [850, 478]),
            (9, [3, 3], [848, 477]),
            (12, [4, 3], [636, 357]),
        ] {
            let plan = plan_sheet(count, [3840, 2160]).unwrap();
            assert_eq!([plan.columns, plan.rows], grid);
            assert_eq!([plan.cell_width, plan.cell_height], cell);
        }
        let small = plan_sheet(12, [64, 32]).unwrap();
        assert_eq!([small.columns, small.rows], [12, 1]);
        assert_eq!([small.cell_width, small.cell_height], [64, 32]);
        for source in [[1080, 1920], [1, 1], [u32::MAX, 1], [1, u32::MAX]] {
            for count in 1..=12 {
                let plan = plan_sheet(count, source).unwrap();
                assert!(plan.width <= SHEET_MAX_WIDTH && plan.height <= SHEET_MAX_HEIGHT);
                assert!(plan.cell_width <= source[0] && plan.cell_height <= source[1]);
                let last = plan.cell_position(count - 1).unwrap();
                assert!(last[0] + plan.cell_width + MARGIN <= plan.width);
                assert!(last[1] + plan.cell_height + MARGIN <= plan.height);
                assert!(
                    plan.cell_position((plan.columns * plan.rows) as usize)
                        .is_none()
                );
            }
        }
        assert!(plan_sheet(0, [1, 1]).is_err());
        assert!(plan_sheet(13, [1, 1]).is_err());
        assert!(plan_sheet(1, [0, 1]).is_err());
    }

    #[test]
    fn compact_stamps_use_frame_rounding_and_span_ignores_request_order() {
        for (time, fps, stamp) in [
            (0.0, 30.0, "0f"),
            (8.0 + 10.0 / 30.0, 30.0, "08s10f"),
            (65.0, 30.0, "01m05s"),
            (3600.0, 24.0, "01h"),
            (59.99, 30.0, "01m"),
            (0.5, 23.976, "12f"),
        ] {
            assert_eq!(format_timecode(time, fps).unwrap(), stamp);
        }
        assert_eq!(sheet_timecode(&[8.0, 0.0, 2.0], 30.0).unwrap(), "0f-08s");
        assert_eq!(sheet_timecode(&[0.001, 0.0], 30.0).unwrap(), "0f");
        assert!(sheet_timecode(&[], 30.0).is_err());
        assert!(sheet_timecode(&[f64::NAN], 30.0).is_err());
        assert!(format_timecode(-1.0, 30.0).is_err());
    }
}

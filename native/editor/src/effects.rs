// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Canvas filter functions, in document order and sRGB, plus device-space blur.
//! Formulas: https://www.w3.org/TR/filter-effects-1/#FilterProperty
use crate::{Result, document::Node, encoder::cancelled, paints};
use std::{
    f64::consts::PI,
    sync::{Arc, atomic::AtomicBool},
};
use tiny_skia::Pixmap;

#[derive(Clone, Debug)]
pub(crate) enum Filter {
    Blur(f64),
    Matrix([[f64; 4]; 3]),
}
pub(crate) type Filters = Arc<[Filter]>;
const EPSILON: f64 = 0.0001;

fn saturation(value: f64, [r, g, b]: [f64; 3]) -> [[f64; 4]; 3] {
    [
        [r + (1.0 - r) * value, g - g * value, b - b * value, 0.0],
        [r - r * value, g + (1.0 - g) * value, b - b * value, 0.0],
        [r - r * value, g - g * value, b + (1.0 - b) * value, 0.0],
    ]
}
fn diagonal(slope: f64, offset: f64) -> [[f64; 4]; 3] {
    [
        [slope, 0.0, 0.0, offset],
        [0.0, slope, 0.0, offset],
        [0.0, 0.0, slope, offset],
    ]
}
pub(crate) fn inherited(node: &Node, parent: &Filters) -> Result<Filters> {
    let mut filters = Vec::new();
    let blur = paints::number(node, "blur", 0.0);
    if blur > EPSILON {
        filters.push(Filter::Blur(blur));
    }
    for effect in node
        .children
        .iter()
        .filter(|n| n.tag == "effect" && !n.flag("hidden"))
    {
        let value = paints::number(effect, "value", 0.0);
        let v = value.clamp(0.0, 1.0);
        let filter = match effect.string("type").unwrap_or("blur") {
            "brightness" if (v - 1.0).abs() > EPSILON => Some(Filter::Matrix(diagonal(v, 0.0))),
            "contrast" if (v - 1.0).abs() > EPSILON => {
                Some(Filter::Matrix(diagonal(v, 0.5 * (1.0 - v))))
            }
            "saturate" if (v - 1.0).abs() > EPSILON => {
                Some(Filter::Matrix(saturation(v, [0.213, 0.715, 0.072])))
            }
            "grayscale" if v > EPSILON => Some(Filter::Matrix(saturation(
                1.0 - v,
                [0.2126, 0.7152, 0.0722],
            ))),
            "invert" if v > EPSILON => Some(Filter::Matrix(diagonal(1.0 - 2.0 * v, v))),
            "sepia" if v > EPSILON => Some(Filter::Matrix([
                [1.0 - 0.607 * v, 0.769 * v, 0.189 * v, 0.0],
                [0.349 * v, 1.0 - 0.314 * v, 0.168 * v, 0.0],
                [0.272 * v, 0.534 * v, 1.0 - 0.869 * v, 0.0],
            ])),
            "hueRotate" if value.abs() > EPSILON => {
                let (s, c) = (value.rem_euclid(360.0) * PI / 180.0).sin_cos();
                Some(Filter::Matrix([
                    [
                        0.213 + 0.787 * c - 0.213 * s,
                        0.715 - 0.715 * c - 0.715 * s,
                        0.072 - 0.072 * c + 0.928 * s,
                        0.0,
                    ],
                    [
                        0.213 - 0.213 * c + 0.143 * s,
                        0.715 + 0.285 * c + 0.140 * s,
                        0.072 - 0.072 * c - 0.283 * s,
                        0.0,
                    ],
                    [
                        0.213 - 0.213 * c - 0.787 * s,
                        0.715 - 0.715 * c + 0.715 * s,
                        0.072 + 0.928 * c + 0.072 * s,
                        0.0,
                    ],
                ]))
            }
            "brightness" | "contrast" | "saturate" | "grayscale" | "invert" | "sepia"
            | "hueRotate" => None,
            // The JSX adapter maps an unknown type to LAYER_BLUR.
            _ if value > EPSILON => Some(Filter::Blur(value)),
            _ => None,
        };
        if let Some(filter) = filter {
            filters.push(filter);
        }
        if filters.len() > 64 {
            return Err("A native holder accepts at most 64 active filters".into());
        }
    }
    let mut support = 0.0;
    for filter in &filters {
        if let Filter::Blur(sigma) = filter {
            support += blur_support(*sigma)?;
        }
    }
    if support > 8192.0 {
        return Err("Native filter chain exceeds 8192 pixels of blur support".into());
    }
    Ok(if filters.is_empty() {
        parent.clone()
    } else {
        filters.into()
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Shadow {
    pub color: u32,
    pub blur: f64,
    pub offset: [f64; 2],
}
#[derive(Default)]
pub(crate) struct ShadowState {
    blur: f64,
}
impl ShadowState {
    pub fn assign(&mut self, node: &Node) -> Result<Shadow> {
        let requested = paints::number(node, "blur", 0.0);
        // Canvas ignores negative shadowBlur assignments, retaining prior state.
        if requested >= 0.0 {
            self.blur = requested;
        }
        let offset = [
            paints::number(node, "offsetX", 0.0),
            paints::number(node, "offsetY", 0.0),
        ];
        if self.blur > 512.0 || offset.iter().any(|v| v.abs() > 1e6) {
            return Err("Native shadow exceeds blur 512 or offset 1000000".into());
        }
        Ok(Shadow {
            color: paints::color(node),
            blur: self.blur,
            offset,
        })
    }
}

pub(crate) fn blur_support(sigma: f64) -> Result<f64> {
    if !sigma.is_finite() || !(0.0..=256.0).contains(&sigma) {
        return Err("Native blur sigma must be finite and at most 256 device pixels".into());
    }
    Ok(if sigma <= 0.03 {
        0.0
    } else {
        (3.0 * sigma).ceil() + 2.0
    })
}

// Charged across the complete frame, not reset for each styled glyph/draw.
#[derive(Default)]
pub(crate) struct Work {
    count: u64,
}
impl Work {
    pub fn charge(&mut self, pixels: usize, passes: usize) -> Result<()> {
        self.count = self
            .count
            .saturating_add((pixels as u64).saturating_mul(passes as u64));
        if self.count > 1 << 31 {
            return Err("Native frame exceeds effect pixel work budget".into());
        }
        Ok(())
    }
}
pub(crate) fn buffer(width: u32, height: u32) -> Result<Pixmap> {
    let len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(4))
        .ok_or("Effect buffer size overflows")?;
    if width > 16384 || height > 16384 || len > 64 * 1024 * 1024 {
        return Err("Native effect buffer exceeds 16384 pixels per edge or 64 MiB".into());
    }
    let size = tiny_skia::IntSize::from_wh(width, height).ok_or("Invalid effect buffer size")?;
    Pixmap::from_vec(bytes(len)?, size).ok_or_else(|| "Invalid effect buffer storage".into())
}
fn bytes(len: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| "Cannot allocate native effect buffer")?;
    data.resize(len, 0);
    Ok(data)
}

pub(crate) fn apply(
    pixels: &mut Pixmap,
    filters: &[Filter],
    work: &mut Work,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let mut start = 0;
    while start < filters.len() {
        cancelled(cancel)?;
        if let Filter::Blur(sigma) = filters[start] {
            blur(pixels, sigma, work, cancel)?;
            start += 1;
            continue;
        }
        let end = filters[start..]
            .iter()
            .position(|f| matches!(f, Filter::Blur(_)))
            .map_or(filters.len(), |i| start + i);
        work.charge(
            pixels.width() as usize * pixels.height() as usize,
            end - start,
        )?;
        for part in pixels.data_mut().chunks_mut(16384) {
            cancelled(cancel)?;
            for p in part.as_chunks_mut::<4>().0 {
                let a = p[3] as f64;
                if a == 0.0 {
                    continue;
                }
                let mut rgb = [p[0] as f64 / a, p[1] as f64 / a, p[2] as f64 / a];
                for filter in &filters[start..end] {
                    if let Filter::Matrix(m) = filter {
                        rgb = m.map(|r| {
                            (r[0] * rgb[0] + r[1] * rgb[1] + r[2] * rgb[2] + r[3]).clamp(0.0, 1.0)
                        });
                    }
                }
                for i in 0..3 {
                    p[i] = (rgb[i] * a).round() as u8;
                }
            }
        }
        start = end;
    }
    Ok(())
}

pub(crate) fn blur(
    pixels: &mut Pixmap,
    sigma: f64,
    work: &mut Work,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    blur_support(sigma)?;
    if sigma <= 0.03 {
        return Ok(());
    }
    let (w, h) = (pixels.width() as usize, pixels.height() as usize);
    let mut scratch = bytes(pixels.data().len())?;
    if sigma <= 32.0 {
        let radius = (3.0 * sigma).ceil() as i32;
        let mut weights = (-radius..=radius)
            .map(|x| (-(x as f64).powi(2) / (2.0 * sigma * sigma)).exp())
            .collect::<Vec<_>>();
        let total: f64 = weights.iter().sum();
        for v in &mut weights {
            *v /= total;
        }
        work.charge(w * h, 2 * weights.len())?;
        for vertical in [false, true] {
            for y in 0..h {
                cancelled(cancel)?;
                for x in 0..w {
                    let mut sum = [0.0; 4];
                    for (i, weight) in weights.iter().enumerate() {
                        let delta = i as i32 - radius;
                        let (xx, yy) = if vertical {
                            (x as i32, y as i32 + delta)
                        } else {
                            (x as i32 + delta, y as i32)
                        };
                        if xx >= 0 && yy >= 0 && xx < w as i32 && yy < h as i32 {
                            let p = &pixels.data()[(yy as usize * w + xx as usize) * 4..][..4];
                            for c in 0..4 {
                                sum[c] += p[c] as f64 * weight;
                            }
                        }
                    }
                    let k = (y * w + x) * 4;
                    for c in 0..4 {
                        scratch[k + c] = sum[c].round() as u8;
                    }
                }
            }
            pixels.data_mut().copy_from_slice(&scratch);
        }
    } else {
        // The three-box Gaussian approximation specified by Filter Effects 1.
        // Even widths use opposite half-pixel centers, then an odd third box.
        let d = (sigma * 3.0 * (2.0 * PI).sqrt() / 4.0 + 0.5).floor() as usize;
        let boxes = if d % 2 == 1 {
            [(d / 2, d / 2); 3]
        } else {
            [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)]
        };
        work.charge(w * h, 6)?;
        for vertical in [false, true] {
            for (left, right) in boxes {
                box_pass(
                    pixels.data(),
                    &mut scratch,
                    w,
                    h,
                    vertical,
                    left,
                    right,
                    cancel,
                )?;
                pixels.data_mut().copy_from_slice(&scratch);
            }
        }
    }
    cancelled(cancel)
}

// tiny-skia 0.12 changes every pure translation to nearest-neighbor, even
// when bilinear sampling is requested. Resample the fractional part explicitly.
pub(crate) fn fractional_offset(
    pixels: &mut Pixmap,
    offset: [f64; 2],
    work: &mut Work,
    cancel: Option<&AtomicBool>,
) -> Result<[f64; 2]> {
    let integer = offset.map(f64::floor);
    let [fx, fy] = [offset[0] - integer[0], offset[1] - integer[1]];
    if fx == 0.0 && fy == 0.0 {
        return Ok(integer);
    }
    let (w, h) = (pixels.width() as usize, pixels.height() as usize);
    work.charge(w * h, 4)?;
    let mut output = bytes(pixels.data().len())?;
    for y in 0..h {
        cancelled(cancel)?;
        for x in 0..w {
            let mut sum = [0.0; 4];
            for (dx, dy, weight) in [
                (0, 0, (1.0 - fx) * (1.0 - fy)),
                (1, 0, fx * (1.0 - fy)),
                (0, 1, (1.0 - fx) * fy),
                (1, 1, fx * fy),
            ] {
                if x >= dx && y >= dy {
                    let p = &pixels.data()[((y - dy) * w + x - dx) * 4..][..4];
                    for c in 0..4 {
                        sum[c] += p[c] as f64 * weight;
                    }
                }
            }
            for c in 0..4 {
                output[(y * w + x) * 4 + c] = sum[c].round() as u8;
            }
        }
    }
    pixels.data_mut().copy_from_slice(&output);
    Ok(integer)
}
#[allow(clippy::too_many_arguments)]
fn box_pass(
    input: &[u8],
    output: &mut [u8],
    w: usize,
    h: usize,
    vertical: bool,
    left: usize,
    right: usize,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let (lines, len, stride) = if vertical { (w, h, w * 4) } else { (h, w, 4) };
    let divisor = (left + right + 1) as u32;
    for line in 0..lines {
        cancelled(cancel)?;
        let base = if vertical { line * 4 } else { line * w * 4 };
        let mut sum = [0u32; 4];
        for i in 0..=right.min(len - 1) {
            for c in 0..4 {
                sum[c] += input[base + i * stride + c] as u32;
            }
        }
        for i in 0..len {
            for c in 0..4 {
                output[base + i * stride + c] = ((sum[c] + divisor / 2) / divisor) as u8;
            }
            if i >= left {
                for c in 0..4 {
                    sum[c] -= input[base + (i - left) * stride + c] as u32;
                }
            }
            if i + right + 1 < len {
                for c in 0..4 {
                    sum[c] += input[base + (i + right + 1) * stride + c] as u32;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effect_buffers_work_and_cancellation_are_bounded() {
        assert!(buffer(u32::MAX, 2).is_err());
        assert!(buffer(8192, 8192).is_err());
        assert!(buffer(16385, 1).is_err());
        assert!(blur_support(f64::NAN).is_err());
        assert!(blur_support(257.0).is_err());
        let mut pixels = buffer(16, 16).unwrap();
        let mut work = Work { count: 1 << 31 };
        assert!(
            blur(&mut pixels, 2.0, &mut work, None)
                .unwrap_err()
                .contains("work budget")
        );
        let cancel = AtomicBool::new(true);
        assert!(
            blur(&mut pixels, 2.0, &mut Work::default(), Some(&cancel))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            fractional_offset(&mut pixels, [0.5, 0.5], &mut Work::default(), Some(&cancel))
                .unwrap_err()
                .contains("cancelled")
        );
    }
    #[test]
    fn fractional_shadow_translation_preserves_subpixel_coverage() {
        let mut p = buffer(4, 4).unwrap();
        p.data_mut()[0..4].copy_from_slice(&[255; 4]);
        assert_eq!(
            fractional_offset(&mut p, [-1.5, 2.25], &mut Work::default(), None).unwrap(),
            [-2.0, 2.0]
        );
        assert_eq!(&p.data()[..8], &[96, 96, 96, 96, 96, 96, 96, 96]);
        assert_eq!(&p.data()[16..24], &[32, 32, 32, 32, 32, 32, 32, 32]);
    }
}

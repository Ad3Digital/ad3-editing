// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Stateful selection of settled visual states, ported from frame-triage.ts.
use crate::Result;
use std::sync::atomic::{AtomicBool, Ordering};

pub const GRID: usize = 32;
pub const DIM: usize = GRID * GRID;
pub type Fingerprint = [f32; DIM];
const THRESHOLD: f64 = 2.0 / DIM as f64;

/// Original 32x32 block averages with 8x8 bilinear taps per block.
/// UV coordinates sample texel centers with clamp-to-edge behavior.
pub fn fingerprint(frame: &tiny_skia::Pixmap, cancel: Option<&AtomicBool>) -> Result<Fingerprint> {
    let mut print = [0.0; DIM];
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let pixels = frame.pixels();
    let luma = |x: usize, y: usize| {
        let pixel = pixels[y * width + x];
        let alpha = pixel.alpha() as f32;
        if alpha == 0.0 {
            return 0.0;
        }
        // tiny-skia stores premultiplied bytes; fingerprints use source RGB.
        (pixel.red() as f32 * 0.2126 + pixel.green() as f32 * 0.7152 + pixel.blue() as f32 * 0.0722)
            / alpha
    };
    for block_y in 0..GRID {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Auto fingerprint cancelled".into());
        }
        for block_x in 0..GRID {
            let mut sum = 0.0f32;
            for tap_y in 0..8 {
                let y = (((block_y as f32 + (tap_y as f32 + 0.5) / 8.0) / GRID as f32)
                    * height as f32
                    - 0.5)
                    .clamp(0.0, (height - 1) as f32);
                let y0 = y.floor() as usize;
                let y1 = (y0 + 1).min(height - 1);
                let fy = y - y0 as f32;
                for tap_x in 0..8 {
                    let x = (((block_x as f32 + (tap_x as f32 + 0.5) / 8.0) / GRID as f32)
                        * width as f32
                        - 0.5)
                        .clamp(0.0, (width - 1) as f32);
                    let x0 = x.floor() as usize;
                    let x1 = (x0 + 1).min(width - 1);
                    let fx = x - x0 as f32;
                    let top = luma(x0, y0) * (1.0 - fx) + luma(x1, y0) * fx;
                    let bottom = luma(x0, y1) * (1.0 - fx) + luma(x1, y1) * fx;
                    sum += top * (1.0 - fy) + bottom * fy;
                }
            }
            print[block_y * GRID + block_x] = (sum / 64.0).clamp(0.0, 1.0);
        }
    }
    Ok(print)
}

pub fn distance(a: &Fingerprint, b: &Fingerprint) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let delta = (f64::from(*a) - f64::from(*b)).abs();
            if delta > 0.02 {
                ((delta - 0.02) / 0.08).min(1.0)
            } else {
                0.0
            }
        })
        .sum::<f64>()
        / DIM as f64
}

#[derive(Default)]
pub struct Triage {
    kept: Vec<(f64, f64)>,
    last_kept: Option<Fingerprint>,
    previous: Option<Fingerprint>,
    previous_time: Option<f64>,
    run_start: f64,
    calmest: Option<(f64, Fingerprint, f64)>,
}

impl Triage {
    fn keep(&mut self, time: f64, print: Fingerprint, novelty: f64) -> Result<()> {
        if self.kept.len() >= 1_000_000 {
            return Err("Auto selection exceeds its state budget".into());
        }
        self.kept.push((time, novelty));
        self.last_kept = Some(print);
        Ok(())
    }

    pub fn push(&mut self, time: f64, print: Fingerprint) -> Result<()> {
        if !time.is_finite()
            || time < 0.0
            || print
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Auto selection needs finite nonnegative times and normalized luma".into());
        }
        if self.previous_time.is_some_and(|previous| time < previous) {
            return Err("Auto samples must be in presentation order".into());
        }
        if self.previous_time == Some(time) {
            return Ok(());
        }
        let motion = self
            .previous
            .as_ref()
            .map_or(0.0, |prev| distance(&print, prev));
        self.previous_time = Some(time);
        self.previous = Some(print);
        if motion <= THRESHOLD {
            self.calmest = None;
            let novelty = self
                .last_kept
                .as_ref()
                .map_or(f64::INFINITY, |prev| distance(&print, prev));
            if novelty >= THRESHOLD {
                self.keep(time, print, novelty)?;
            }
        } else {
            if self.calmest.is_none() {
                self.run_start = time;
            }
            if self
                .calmest
                .as_ref()
                .is_none_or(|(_, _, best)| motion < *best)
            {
                self.calmest = Some((time, print, motion));
            }
            if time - self.run_start >= 4.0 {
                let (at, calmest, _) = self.calmest.take().unwrap();
                let novelty = self
                    .last_kept
                    .as_ref()
                    .map_or(f64::INFINITY, |prev| distance(&calmest, prev));
                if novelty >= THRESHOLD {
                    self.keep(at, calmest, novelty)?;
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self, max: usize) -> Result<Vec<f64>> {
        if max == 0 || max > 1_000_000 {
            return Err("Auto frame limit must be between 1 and 1000000".into());
        }
        if self.kept.is_empty() {
            return Err("No frames decoded for auto selection".into());
        }
        // Stable ties preserve the original candidate order, as JavaScript sort does.
        self.kept.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.kept.truncate(max);
        self.kept.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(self.kept.into_iter().map(|(time, _)| time).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprints_match_luma_texel_centers_edges_and_local_blocks() {
        let mut frame = tiny_skia::Pixmap::new(256, 256).unwrap();
        frame.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let red = fingerprint(&frame, None).unwrap();
        assert!(red.iter().all(|value| (*value - 0.2126).abs() < 0.00001));
        frame.fill(tiny_skia::Color::BLACK);
        for y in 0..8usize {
            for x in 0..16usize {
                let at = (y * 256 + x) * 4;
                frame.data_mut()[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let local = fingerprint(&frame, None).unwrap();
        assert_eq!(&local[..3], &[1.0, 1.0, 0.0]);
        assert!(local[2..].iter().all(|value| *value == 0.0));
        assert_eq!(distance(&local, &[0.0; DIM]), THRESHOLD);
        let mut tiny = tiny_skia::Pixmap::new(1, 1).unwrap();
        tiny.fill(tiny_skia::Color::WHITE);
        assert_eq!(fingerprint(&tiny, None).unwrap(), [1.0; DIM]);
        tiny.fill(tiny_skia::Color::TRANSPARENT);
        assert_eq!(fingerprint(&tiny, None).unwrap(), [0.0; DIM]);
        let mut edge = tiny_skia::Pixmap::new(2, 1).unwrap();
        edge.data_mut()
            .copy_from_slice(&[255, 255, 255, 255, 0, 0, 0, 255]);
        let interpolated = fingerprint(&edge, None).unwrap();
        assert_eq!(interpolated[0], 1.0);
        assert_eq!(interpolated[31], 0.0);
        assert!((interpolated[15] - 0.53125).abs() < 0.00001);
        assert!((interpolated[16] - 0.46875).abs() < 0.00001);
        assert!(fingerprint(&tiny, Some(&AtomicBool::new(true))).is_err());
    }
    #[test]
    fn static_noise_and_repeated_frames_collapse_to_first_sample() {
        let mut triage = Triage::default();
        triage.push(0.0, [0.5; DIM]).unwrap();
        triage.push(0.0, [0.0; DIM]).unwrap();
        for i in 1..20 {
            triage.push(i as f64 / 2.0, [0.51; DIM]).unwrap();
        }
        assert_eq!(triage.finish(30).unwrap(), [0.0]);
        assert_eq!(distance(&[0.0; DIM], &[1.0; DIM]), 1.0);
        assert_eq!(distance(&[0.0; DIM], &[0.01; DIM]), 0.0);
    }
    #[test]
    fn transitions_wait_for_rest_and_budget_keeps_largest_novelty() {
        let mut triage = Triage::default();
        for (time, value) in [(0.0, 0.0), (0.5, 0.05), (1.0, 0.05), (1.5, 0.5), (2.0, 0.5)] {
            triage.push(time, [value; DIM]).unwrap();
        }
        assert_eq!(triage.finish(2).unwrap(), [0.0, 2.0]);
        let mut local = [0.0; DIM];
        local[..2].fill(1.0);
        let mut triage = Triage::default();
        triage.push(0.0, [0.0; DIM]).unwrap();
        triage.push(0.5, local).unwrap();
        assert_eq!(triage.finish(30).unwrap(), [0.0, 0.5]);
    }
    #[test]
    fn perpetual_motion_uses_calmest_sample_and_rejects_invalid_input() {
        let mut triage = Triage::default();
        triage.push(0.0, [0.0; DIM]).unwrap();
        for i in 1..=10 {
            triage
                .push(i as f64 / 2.0, [if i % 2 == 1 { 1.0 } else { 0.0 }; DIM])
                .unwrap();
        }
        assert_eq!(triage.finish(30).unwrap(), [0.0, 0.5]);
        let mut triage = Triage::default();
        assert!(triage.push(f64::NAN, [0.0; DIM]).is_err());
        assert!(triage.push(0.0, [f32::NAN; DIM]).is_err());
        triage.push(1.0, [0.0; DIM]).unwrap();
        assert!(triage.push(0.0, [0.0; DIM]).is_err());
        assert!(Triage::default().finish(30).is_err());
    }
}

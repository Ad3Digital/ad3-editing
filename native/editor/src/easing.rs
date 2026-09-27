// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
// Numeric easing algorithms adapted from anime.js 4.5.0 (MIT),
// Copyright (c) 2025-2026 Julian Garnier. See licenses/animejs-MIT.txt.
// Bezier solver originally (c) 2014 Gaetan Renaudeau; spring solver adapted
// by anime.js from (c) 2016 Webkit - Apple Inc.
//! Deterministic native equivalents of the original editor's easing curves.
use crate::Result;

#[derive(Clone, Debug)]
pub enum Easing {
    Linear,
    Bezier([f64; 4]),
    Steps { count: f64, from_start: bool },
    Spring(Spring),
}

/// ECMAScript whitespace, shared by descriptor normalization/text word reveals.
pub fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

impl Easing {
    pub fn parse(text: &str) -> Result<Self> {
        let text: String = text.chars().filter(|&c| !js_whitespace(c)).collect();
        let descriptor = match text.as_str() {
            "easeIn" => "cubicBezier(0.42,0,1,1)",
            "easeOut" => "cubicBezier(0,0,0.58,1)",
            "easeInOut" => "cubicBezier(0.42,0,0.58,1)",
            "gentle" => "spring(0.5,628)",
            "snappy" => "spring(0.15,300)",
            "bouncy" => "spring(0.4,500)",
            "strong" => "spring(0.65,400)",
            other => other,
        };
        if let Some(args) = arguments(descriptor, "steps") {
            let pieces: Vec<_> = args.split(',').collect();
            if (pieces.len() == 1 || pieces.len() == 2)
                && !pieces[0].is_empty()
                && pieces[0].bytes().all(|c| c.is_ascii_digit())
                && (pieces.len() == 1 || matches!(pieces[1], "true" | "false"))
            {
                let count: f64 = pieces[0].parse().map_err(|_| "Invalid step count")?;
                if count == 0.0 || !count.is_finite() {
                    return Err("Easing steps requires a finite positive count".into());
                }
                return Ok(Self::Steps {
                    count,
                    from_start: pieces.get(1) == Some(&"true"),
                });
            }
        }
        for (name, count) in [("cubicBezier", 4), ("spring", 2)] {
            if let Some(args) = arguments(descriptor, name) {
                let pieces: Vec<_> = args.split(',').collect();
                // Match the editor's descriptor grammar, not Rust's broader
                // float grammar (which would accept exponent notation/NaN).
                let valid = pieces.len() == count
                    && pieces.iter().all(|part| {
                        let digits = if name == "cubicBezier" {
                            part.strip_prefix('-').unwrap_or(part)
                        } else {
                            part
                        };
                        !digits.is_empty()
                            && digits.bytes().all(|c| {
                                c.is_ascii_digit() || c == b'.' || (name == "spring" && c == b'-')
                            })
                    });
                if !valid {
                    continue;
                }
                let numbers: Vec<f64> = pieces
                    .iter()
                    .map(|s| s.parse::<f64>().map_err(|_| "Invalid easing number"))
                    .collect::<std::result::Result<_, _>>()?;
                if numbers.iter().any(|n| !n.is_finite()) {
                    return Err("Non-finite easing parameter".into());
                }
                return Ok(if count == 4 {
                    Self::Bezier([numbers[0], numbers[1], numbers[2], numbers[3]])
                } else {
                    Self::Spring(Spring::new(numbers[0], numbers[1]))
                });
            }
        }
        // Unknown descriptors have always fallen back to linear in the editor.
        Ok(Self::Linear)
    }

    pub fn sample(&self, progress: f64) -> f64 {
        match self {
            Self::Linear => progress,
            Self::Bezier(points) => cubic_bezier(*points, progress),
            Self::Steps { count, from_start } => {
                let value = progress.clamp(0.0, 1.0) * count;
                (if *from_start {
                    value.ceil()
                } else {
                    value.floor()
                }) * (1.0 / count)
            }
            Self::Spring(spring) => spring.sample(progress),
        }
    }
}

fn arguments<'a>(descriptor: &'a str, name: &str) -> Option<&'a str> {
    descriptor
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

pub fn cubic_bezier([x1, y1, x2, y2]: [f64; 4], progress: f64) -> f64 {
    if (x1 == y1 && x2 == y2) || progress == 0.0 || progress == 1.0 {
        return progress;
    }
    fn calc(t: f64, a: f64, b: f64) -> f64 {
        (((1.0 - 3.0 * b + 3.0 * a) * t + (3.0 * b - 6.0 * a)) * t + 3.0 * a) * t
    }
    let (mut low, mut high) = (0.0, 1.0);
    let mut t = 0.5;
    for _ in 0..100 {
        t = low + (high - low) / 2.0;
        let error = calc(t, x1, x2) - progress;
        if error > 0.0 {
            high = t;
        } else {
            low = t;
        }
        if error.abs() <= 0.0000001 {
            break;
        }
    }
    calc(t, y1, y2)
}

#[derive(Clone, Debug)]
pub struct Spring {
    w0: f64,
    zeta: f64,
    wd: f64,
    b: f64,
    duration: f64,
}
impl Spring {
    fn new(bounce: f64, duration: f64) -> Self {
        let bounce = bounce.clamp(-1.0, 1.0);
        let seconds = duration.clamp(10.0, 10_000.0) / 1000.0;
        let pi = std::f64::consts::PI;
        let stiffness =
            ((2.0 * pi / seconds).powi(2).clamp(1e-11, 10_000.0) * 1000.0).round() / 1000.0;
        let damping = if bounce >= 0.0 {
            (1.0 - bounce) * 4.0 * pi / seconds
        } else {
            4.0 * pi / (seconds * (1.0 + bounce))
        };
        let damping = (damping.clamp(1e-11, 300.0) * 1000.0).round() / 1000.0;
        let w0 = stiffness.sqrt().clamp(1e-11, 1000.0);
        let zeta = damping / (2.0 * stiffness.sqrt());
        let wd = if zeta < 1.0 {
            w0 * (1.0 - zeta * zeta).sqrt()
        } else if zeta == 1.0 {
            0.0
        } else {
            w0 * (zeta * zeta - 1.0).sqrt()
        };
        let b = if zeta == 1.0 { w0 } else { zeta * w0 / wd };
        let mut spring = Self {
            w0,
            zeta,
            wd,
            b,
            duration: 0.0,
        };
        let (mut solver_time, mut rest_steps, mut iterations) = (0.0, 0, 0);
        while rest_steps <= 10 && iterations <= 3000 {
            if (1.0 - spring.solve(solver_time)).abs() < 0.0005 {
                rest_steps += 1;
            } else {
                rest_steps = 0;
            }
            spring.duration = solver_time;
            solver_time += 0.02;
            iterations += 1;
        }
        spring
    }
    fn solve(&self, time: f64) -> f64 {
        let Self {
            w0, zeta, wd, b, ..
        } = *self;
        let remaining = if zeta < 1.0 {
            (-time * zeta * w0).exp() * ((wd * time).cos() + b * (wd * time).sin())
        } else if zeta == 1.0 {
            (1.0 + b * time) * (-time * w0).exp()
        } else {
            ((1.0 + b) * ((-zeta * w0 + wd) * time).exp()
                + (1.0 - b) * ((-zeta * w0 - wd) * time).exp())
                / 2.0
        };
        1.0 - remaining
    }
    fn sample(&self, progress: f64) -> f64 {
        if progress == 0.0 || progress == 1.0 {
            progress
        } else {
            self.solve(progress * self.duration)
        }
    }
}

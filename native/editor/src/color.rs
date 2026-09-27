// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
// RGB/HSL conversion follows colord (MIT), Copyright (c) 2020 Vlad Shilov.
// See licenses/colord-MIT.txt. Named/fallback behavior follows the AD3 runtime.
use crate::easing::js_whitespace;

pub fn number(value: f64) -> Option<u32> {
    value
        .is_finite()
        .then(|| (value.trunc().rem_euclid(4_294_967_296.0) as u32) & 0xff_ffff)
}

pub fn parse(input: &str) -> Option<u32> {
    let text = input.trim_matches(js_whitespace);
    if text.is_empty() {
        return None;
    }
    if let Some(color) = named(&text.to_lowercase()) {
        return Some(color);
    }
    if let Some(hex) = text.strip_prefix('#')
        && hex.bytes().all(|c| c.is_ascii_hexdigit())
    {
        match hex.len() {
            3 | 4 => {
                return u32::from_str_radix(
                    &hex[..3].chars().flat_map(|c| [c, c]).collect::<String>(),
                    16,
                )
                .ok();
            }
            6 | 8 => return u32::from_str_radix(&hex[..6], 16).ok(),
            _ => {}
        }
    }
    if let Some(color) = functional(text) {
        return Some(color);
    }
    let hex: String = text.chars().filter(char::is_ascii_hexdigit).collect();
    let normalized = match hex.len() {
        0 => return None,
        1 => hex.repeat(6),
        2 => hex.repeat(3),
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        4 => format!("{hex}00"),
        5 => format!("{hex}0"),
        _ => hex[..6].to_owned(),
    };
    u32::from_str_radix(&normalized, 16).ok()
}

fn decimal(text: &str) -> Option<f64> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let valid = if let Some((integer, fraction)) = digits.split_once('.') {
        integer.bytes().all(|c| c.is_ascii_digit())
            && !fraction.is_empty()
            && fraction.bytes().all(|c| c.is_ascii_digit())
    } else {
        !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit())
    };
    valid.then(|| text.parse().ok()).flatten()
}

fn percentage(text: &str) -> Option<(f64, bool)> {
    let percent = text.ends_with('%');
    Some((decimal(text.strip_suffix('%').unwrap_or(text))?, percent))
}

fn functional(text: &str) -> Option<u32> {
    let (name, arguments) = text.split_once('(')?;
    let arguments = arguments.strip_suffix(')')?;
    let hsl = match name.to_ascii_lowercase().as_str() {
        "hsl" | "hsla" => true,
        "rgb" | "rgba" => false,
        _ => return None,
    };
    let components: Vec<&str> = if arguments.contains(',') {
        arguments
            .split(',')
            .map(|s| s.trim_matches(js_whitespace))
            .collect()
    } else {
        let (values, alpha) = arguments
            .split_once('/')
            .map_or((arguments, None), |(v, a)| (v, Some(a)));
        let mut values: Vec<_> = values
            .split(js_whitespace)
            .filter(|v| !v.is_empty())
            .collect();
        if values.len() != 3 {
            return None;
        }
        if let Some(alpha) = alpha {
            values.push(alpha.trim_matches(js_whitespace));
        }
        values
    };
    if !(3..=4).contains(&components.len()) {
        return None;
    }
    if let Some(alpha) = components.get(3) {
        percentage(alpha)?;
    }
    if !hsl {
        let channels: Vec<_> = components[..3]
            .iter()
            .map(|v| percentage(v))
            .collect::<Option<_>>()?;
        if channels[0].1 != channels[1].1 || channels[1].1 != channels[2].1 {
            return None;
        }
        let divisor = if channels[0].1 { 100.0 / 255.0 } else { 1.0 };
        return Some(pack([
            channels[0].0 / divisor,
            channels[1].0 / divisor,
            channels[2].0 / divisor,
        ]));
    }
    let hue = components[0];
    let lower = hue.to_ascii_lowercase();
    let (hue, unit) = ["grad", "turn", "deg", "rad"]
        .into_iter()
        .find(|unit| lower.ends_with(unit))
        .map_or((hue, "deg"), |unit| hue.split_at(hue.len() - unit.len()));
    let hue = decimal(hue)?
        * match unit {
            "grad" => 0.9,
            "turn" => 360.0,
            "rad" => 360.0 / (2.0 * std::f64::consts::PI),
            _ => 1.0,
        };
    let (s, sp) = percentage(components[1])?;
    let (l, lp) = percentage(components[2])?;
    if !sp || !lp {
        return None;
    }
    let hue = if hue.is_finite() { hue % 360.0 } else { 0.0 };
    let hue = if hue > 0.0 { hue } else { hue + 360.0 };
    let s = s.clamp(0.0, 100.0);
    let l = l.clamp(0.0, 100.0);
    let chroma = s * ((if l < 50.0 { l } else { 100.0 - l }) / 100.0);
    let saturation = if chroma > 0.0 {
        2.0 * chroma / (l + chroma) * 100.0
    } else {
        0.0
    } / 100.0;
    let value = (l + chroma) / 100.0;
    let sector = hue / 360.0 * 6.0;
    let index = sector.floor();
    let (low, falling, rising) = (
        value * (1.0 - saturation),
        value * (1.0 - (sector - index) * saturation),
        value * (1.0 - (1.0 - sector + index) * saturation),
    );
    let rgb = match (index as usize) % 6 {
        0 => [value, rising, low],
        1 => [falling, value, low],
        2 => [low, value, rising],
        3 => [low, falling, value],
        4 => [rising, low, value],
        _ => [value, low, falling],
    };
    Some(pack(rgb.map(|v| v * 255.0)))
}

fn pack(channels: [f64; 3]) -> u32 {
    let [r, g, b] = channels.map(|v| (v.clamp(0.0, 255.0) + 0.5).floor() as u32);
    (r << 16) | (g << 8) | b
}

pub fn lerp(from: u32, to: u32, progress: f64) -> u32 {
    let channel = |shift: u32| {
        let a = ((from >> shift) & 255_u32) as f64;
        let b = ((to >> shift) & 255_u32) as f64;
        let value = a + (b - a) * progress;
        let floor = value.floor();
        // Math.round without losing odd integers above 2^52 by adding 0.5.
        let rounded = if value - floor >= 0.5 {
            floor + 1.0
        } else {
            floor
        };
        // JS bitwise operands use ToInt32 modulo 2^32, not Rust's saturating
        // float-to-integer cast. Non-finite intermediate values become zero.
        if rounded.is_finite() {
            rounded.rem_euclid(4_294_967_296.0) as u32
        } else {
            0
        }
    };
    // Match JS bitwise packing even when a spring overshoots a color channel.
    ((channel(16) << 16) | (channel(8) << 8) | channel(0)) & 0xff_ffff
}

fn named(name: &str) -> Option<u32> {
    match name {
        "red" => Some(0xFF0000),
        "green" => Some(0x008000),
        "blue" => Some(0x0000FF),
        "white" => Some(0xFFFFFF),
        "black" => Some(0x000000),
        "yellow" => Some(0xFFFF00),
        "cyan" => Some(0x00FFFF),
        "magenta" => Some(0xFF00FF),
        "orange" => Some(0xFFA500),
        "purple" => Some(0x800080),
        "pink" => Some(0xFFC0CB),
        "gray" => Some(0x808080),
        "grey" => Some(0x808080),
        "brown" => Some(0xA52A2A),
        "lime" => Some(0x00FF00),
        "navy" => Some(0x000080),
        "teal" => Some(0x008080),
        "maroon" => Some(0x800000),
        "olive" => Some(0x808000),
        "aqua" => Some(0x00FFFF),
        "coral" => Some(0xFF7F50),
        "salmon" => Some(0xFA8072),
        "gold" => Some(0xFFD700),
        "silver" => Some(0xC0C0C0),
        "indigo" => Some(0x4B0082),
        "violet" => Some(0xEE82EE),
        "crimson" => Some(0xDC143C),
        "tomato" => Some(0xFF6347),
        "khaki" => Some(0xF0E68C),
        "plum" => Some(0xDDA0DD),
        "orchid" => Some(0xDA70D6),
        "tan" => Some(0xD2B48C),
        "beige" => Some(0xF5F5DC),
        "ivory" => Some(0xFFFFF0),
        "lavender" => Some(0xE6E6FA),
        "turquoise" => Some(0x40E0D0),
        "chartreuse" => Some(0x7FFF00),
        "sienna" => Some(0xA0522D),
        "peru" => Some(0xCD853F),
        "firebrick" => Some(0xB22222),
        "skyblue" => Some(0x87CEEB),
        "hotpink" => Some(0xFF69B4),
        "mint" => Some(0x3EB489),
        _ => None,
    }
}

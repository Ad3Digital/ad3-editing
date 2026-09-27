// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native port of runtime/utils/text-motion.ts. Authored JS indexes UTF-16;
//! unmatched surrogate halves become replacement glyphs at the Rust text edge.
use crate::easing::js_whitespace;

pub fn reveal_chars(text: &str, ratio: f64) -> String {
    if ratio >= 1.0 {
        return text.into();
    }
    if ratio <= 0.0 {
        return String::new();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let visible = (ratio * units.len() as f64).floor() as usize;
    String::from_utf16_lossy(&units[..visible.min(units.len())])
}

pub fn reveal_words(text: &str, ratio: f64) -> String {
    if ratio >= 1.0 {
        return text.into();
    }
    if ratio <= 0.0 {
        return String::new();
    }
    // Reproduce split(/(\s+)/), including its empty boundary word tokens.
    let mut words = Vec::new();
    let (mut start, mut whitespace) = (0, false);
    for (offset, c) in text.char_indices() {
        let next = js_whitespace(c);
        if next != whitespace {
            words.push(&text[start..offset]);
            start = offset;
            whitespace = next;
        }
    }
    words.push(&text[start..]);
    if whitespace {
        words.push("");
    }
    let count = words.len().div_ceil(2);
    let visible = (ratio * count as f64).floor() as usize;
    if visible == 0 {
        String::new()
    } else {
        words[..(2 * visible - 1).min(words.len())].concat()
    }
}

pub fn scramble_chars(text: &str, ratio: f64) -> String {
    if text.is_empty() || ratio >= 1.0 {
        return text.into();
    }
    const CHARACTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!%#_";
    const STEP: f64 = 1.0 / 30.0;
    let units: Vec<u16> = text.encode_utf16().collect();
    let seed = units.iter().fold(2_166_136_261_u32, |h, &c| {
        (h ^ u32::from(c)).wrapping_mul(16_777_619)
    });
    let step = ((ratio / STEP).trunc().rem_euclid(4_294_967_296.0)) as u32;
    let spacing = (1.0 - 0.3) / units.len() as f64;
    let out: Vec<u16> = units
        .iter()
        .enumerate()
        .map(|(position, &c)| {
            let start = (position as f64 * spacing) as f32;
            let end = (((f64::from(start) + 0.3) / STEP).ceil() * STEP) as f32;
            if ratio >= f64::from(end) || c == 32 {
                return c;
            }
            let mut h = seed
                ^ (position as u32).wrapping_add(1).wrapping_mul(0x9e37_79b1)
                ^ step.wrapping_add(1).wrapping_mul(0x85eb_ca6b);
            h = (h ^ (h >> 16)).wrapping_mul(0x85eb_ca6b);
            h = (h ^ (h >> 13)).wrapping_mul(0xc2b2_ae35);
            h ^= h >> 16;
            u16::from(CHARACTERS[h as usize % CHARACTERS.len()])
        })
        .collect();
    String::from_utf16_lossy(&out)
}

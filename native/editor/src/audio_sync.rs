// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Recording alignment from 500 Hz RMS envelopes, matching the original runtime.
use crate::Result;
use std::f64::consts::PI;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Decode the primary audio stream without downmixing phase-opposed channels.
pub fn extract_envelope(
    tools: &crate::media::Tools,
    path: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let probe = tools.probe_cancellable(path, cancel)?;
    if !probe.has_audio || probe.sample_rate == 0 || probe.channels == 0 {
        return Err("Source has no decodable audio track to sync".into());
    }
    if probe.sample_rate > 384_000 || probe.channels > 64 {
        return Err("Audio sync source layout exceeds native limits".into());
    }
    let output = crate::media::checked_output_cancellable_limit(
        crate::media::command(&tools.ffmpeg)
            .arg("-nostdin")
            .arg("-v")
            .arg("error")
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                "0:a:0",
                "-vn",
                "-sn",
                "-dn",
                "-f",
                "f32le",
                "-acodec",
                "pcm_f32le",
                "pipe:1",
            ]),
        cancel,
        Duration::from_secs(120),
        128 * 1024 * 1024,
    )?;
    envelope_from_pcm(&output.stdout, probe.sample_rate, probe.channels, cancel)
}

fn envelope_from_pcm(
    bytes: &[u8],
    rate: u32,
    channels: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    if rate == 0 || channels == 0 {
        return Err("Invalid audio sync PCM layout".into());
    }
    let stride = channels as usize * 4;
    if bytes.len() % stride != 0 {
        return Err("Truncated audio sync PCM frame".into());
    }
    let frames = bytes.len() / stride;
    let bins = (frames as u64 * 500).div_ceil(rate as u64) as usize;
    if bins > 4_194_304 {
        return Err("Audio sync envelope exceeds native work limit".into());
    }
    let mut envelope = Vec::with_capacity(bins);
    for bin in 0..bins {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Operation cancelled".into());
        }
        let start = (bin as u64 * rate as u64 / 500) as usize;
        let end = (((bin + 1) as u64 * rate as u64 / 500) as usize)
            .max(start + 1)
            .min(frames);
        let samples = &bytes[start * stride..end * stride];
        let mut energy = 0.0;
        for sample in samples.chunks_exact(4) {
            let value = f32::from_le_bytes(sample.try_into().unwrap()) as f64;
            if !value.is_finite() {
                return Err("Audio sync PCM samples must be finite".into());
            }
            energy += value * value;
        }
        envelope.push((energy / (samples.len() / 4) as f64).sqrt() as f32);
    }
    if envelope.is_empty() {
        return Err("No decodable audio to align".into());
    }
    Ok(envelope)
}

pub fn align_files(
    tools: &crate::media::Tools,
    audio: &Path,
    reference: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<AudioSyncResult> {
    let audio = extract_envelope(tools, audio, cancel)?;
    let reference = extract_envelope(tools, reference, cancel)?;
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Operation cancelled".into());
    }
    align_envelopes_cancellable(&audio, &reference, cancel)
}

#[derive(Clone, Copy, Debug)]
pub struct AudioSyncResult {
    pub offset_seconds: f64,
    pub confidence: f64,
}

#[derive(Default)]
pub struct AudioSyncCache(
    std::collections::BTreeMap<
        (crate::media::SourceVersion, crate::media::SourceVersion),
        AudioSyncResult,
    >,
);

impl AudioSyncCache {
    pub fn offsets(
        &mut self,
        root: &crate::document::Node,
        library: &crate::media::MediaLibrary,
        tools: &crate::media::Tools,
        cancel: Option<&AtomicBool>,
    ) -> Result<std::collections::BTreeMap<String, f64>> {
        let mut offsets = std::collections::BTreeMap::new();
        for node in root.nodes() {
            let Some(value) = node.props.get("syncTo") else {
                continue;
            };
            let target = value
                .as_str()
                .ok_or("syncTo requires a reference ID string")?;
            let reference = root
                .find(target)
                .ok_or_else(|| format!("Missing syncTo reference {target:?}"))?;
            if !matches!(node.tag.as_str(), "audio" | "video")
                || !matches!(reference.tag.as_str(), "audio" | "video")
            {
                return Err("syncTo requires audio/video recordings on both ends".into());
            }
            let mut seen = std::collections::BTreeSet::from([node.id.clone()]);
            let mut current = reference;
            loop {
                if !seen.insert(current.id.clone()) {
                    return Err("syncTo links cannot form a cycle".into());
                }
                let Some(next) = current.string("syncTo") else {
                    break;
                };
                current = root.find(next).ok_or("Missing syncTo reference")?;
            }
            let audio = library
                .sources
                .get(&node.id)
                .ok_or("Missing audio sync source")?;
            let reference = library
                .sources
                .get(target)
                .ok_or("Missing audio sync reference source")?;
            let key = (
                crate::media::SourceVersion::read(audio)?,
                crate::media::SourceVersion::read(reference)?,
            );
            let result = if let Some(result) = self.0.get(&key) {
                *result
            } else {
                let result = align_files(tools, audio, reference, cancel)?;
                if key
                    != (
                        crate::media::SourceVersion::read(audio)?,
                        crate::media::SourceVersion::read(reference)?,
                    )
                {
                    return Err("Audio source changed while measuring sync".into());
                }
                if self.0.len() >= 256 {
                    self.0.clear();
                }
                self.0.insert(key, result);
                result
            };
            offsets.insert(node.id.clone(), result.offset_seconds);
        }
        Ok(offsets)
    }
}

/// Place a measured recording against a reference without frame-rounding its origin.
/// Explicit source trims take precedence over the inferred intersection.
pub fn place_recording(
    recording: &crate::timing::Span,
    reference: &crate::timing::Span,
    offset_seconds: f64,
    source_in: Option<f64>,
    source_out: Option<f64>,
    duration: Option<f64>,
) -> Result<crate::timing::Span> {
    let rate = recording.playback_rate;
    if !offset_seconds.is_finite() || !rate.is_finite() || rate <= 0.0 {
        return Err("Invalid audio sync offset or playback rate".into());
    }
    if source_in
        .into_iter()
        .chain(source_out)
        .chain(duration)
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err("Invalid audio sync trim or duration".into());
    }
    let origin = reference.origin + offset_seconds;
    let start = source_in.unwrap_or(((reference.start - origin) * rate).max(0.0));
    let end = source_out.unwrap_or_else(|| {
        let end = (reference.end - origin) * rate;
        duration.map_or(end, |duration| end.min(duration))
    });
    let end = duration.map_or(end, |duration| end.min(duration));
    if !origin.is_finite() || !start.is_finite() || !end.is_finite() || end <= start {
        return Err("Aligned recording does not overlap the reference window".into());
    }
    let mut placed = recording.clone();
    placed.origin = origin;
    placed.source_in = start;
    placed.source_out = end;
    placed.start = origin + start / rate;
    placed.end = origin + end / rate;
    if !placed.start.is_finite() || !placed.end.is_finite() {
        return Err("Audio sync placement overflows".into());
    }
    Ok(placed)
}

fn check_cancel(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Operation cancelled".into());
    }
    Ok(())
}

fn fft(re: &mut [f64], im: &mut [f64], inverse: bool, cancel: Option<&AtomicBool>) -> Result<()> {
    check_cancel(cancel)?;
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        if i % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = if inverse { 2.0 } else { -2.0 } * PI / len as f64;
        let (wi, wr) = angle.sin_cos();
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                if (i + k) % 1024 == 0 {
                    check_cancel(cancel)?;
                }
                let a = i + k;
                let b = a + len / 2;
                let vr = re[b] * cr - im[b] * ci;
                let vi = re[b] * ci + im[b] * cr;
                re[b] = re[a] - vr;
                im[b] = im[a] - vi;
                re[a] += vr;
                im[a] += vi;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len *= 2;
    }
    if inverse {
        for value in re.iter_mut().chain(im.iter_mut()) {
            *value /= n as f64;
        }
    }
    check_cancel(cancel)
}

fn prefix(values: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let mut sums = vec![0.0; values.len() + 1];
    let mut squares = sums.clone();
    for (i, value) in values.iter().enumerate() {
        let value = *value as f64;
        sums[i + 1] = sums[i] + value;
        squares[i + 1] = squares[i] + value * value;
    }
    (sums, squares)
}

pub fn align_envelopes(audio: &[f32], reference: &[f32]) -> Result<AudioSyncResult> {
    align_envelopes_cancellable(audio, reference, None)
}

pub fn align_envelopes_cancellable(
    audio: &[f32],
    reference: &[f32],
    cancel: Option<&AtomicBool>,
) -> Result<AudioSyncResult> {
    check_cancel(cancel)?;
    // Bound FFT storage/work before allocating four double arrays.
    let length = audio
        .len()
        .checked_add(reference.len())
        .ok_or("Audio sync length overflow")?;
    if audio.is_empty() || reference.is_empty() {
        return Err("No decodable audio to align".into());
    }
    if length > 4_194_304 {
        return Err("Audio sync envelope exceeds native work limit".into());
    }
    if audio
        .iter()
        .chain(reference)
        .any(|value| !value.is_finite())
    {
        return Err("Audio sync envelope must be finite".into());
    }
    let n = length.next_power_of_two();
    let mut ar = vec![0.0; n];
    let mut ai = ar.clone();
    let mut br = ar.clone();
    let mut bi = ar.clone();
    for (to, from) in ar.iter_mut().zip(audio) {
        *to = *from as f64;
    }
    for (to, from) in br.iter_mut().zip(reference) {
        *to = *from as f64;
    }
    fft(&mut ar, &mut ai, false, cancel)?;
    fft(&mut br, &mut bi, false, cancel)?;
    for i in 0..n {
        if i % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let re = ar[i] * br[i] + ai[i] * bi[i];
        ai[i] = ar[i] * bi[i] - ai[i] * br[i];
        ar[i] = re;
    }
    fft(&mut ar, &mut ai, true, cancel)?;
    let (pa, qa) = prefix(audio);
    let (pb, qb) = prefix(reference);
    let score = |lag: isize| {
        let lo = 0.max(-lag);
        let hi = (audio.len() as isize).min(reference.len() as isize - lag);
        let count = hi - lo;
        if count < 500 {
            return f64::NEG_INFINITY;
        }
        let (lo, hi, jlo, jhi) = (
            lo as usize,
            hi as usize,
            (lo + lag) as usize,
            (hi + lag) as usize,
        );
        let count = count as f64;
        let a = pa[hi] - pa[lo];
        let b = pb[jhi] - pb[jlo];
        let va = qa[hi] - qa[lo] - a * a / count;
        let vb = qb[jhi] - qb[jlo] - b * b / count;
        if va <= 1e-9 || vb <= 1e-9 {
            return f64::NEG_INFINITY;
        }
        (ar[lag.rem_euclid(n as isize) as usize] - a * b / count) / (va * vb).sqrt()
    };
    let (mut lag, mut best) = (0, f64::NEG_INFINITY);
    for candidate in -(audio.len() as isize - 1)..=reference.len() as isize - 1 {
        if candidate % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let value = score(candidate);
        if value > best {
            best = value;
            lag = candidate;
        }
    }
    if !best.is_finite() {
        return Err("Clips do not overlap enough to align".into());
    }
    if best < 0.1 {
        return Err("Audio tracks share too little common sound".into());
    }
    let (left, right) = (score(lag - 1), score(lag + 1));
    let denominator = left - 2.0 * best + right;
    let mut delta = if left.is_finite() && right.is_finite() && denominator != 0.0 {
        0.5 * (left - right) / denominator
    } else {
        0.0
    };
    if !delta.is_finite() || delta.abs() > 1.0 {
        delta = 0.0;
    }
    Ok(AudioSyncResult {
        offset_seconds: (lag as f64 + delta) / 500.0,
        confidence: best.clamp(0.0, 1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn span(origin: f64, start: f64, end: f64, rate: f64) -> crate::timing::Span {
        crate::timing::Span {
            id: "audio".into(),
            parent_id: Some("group".into()),
            scene_id: "s".into(),
            origin,
            start,
            end,
            playback_rate: rate,
            source_in: 0.0,
            source_out: 10.0,
            hidden: false,
            muted: true,
            volume_db: -6.0,
        }
    }
    #[test]
    fn placement_preserves_fractional_origin_and_infers_trim_intersection() {
        let recording = span(0.0, 0.0, 10.0, 2.0);
        let reference = span(1.0, 2.0, 5.0, 1.0);
        let placed = place_recording(&recording, &reference, 0.403, None, None, Some(6.0)).unwrap();
        assert_eq!(placed.origin, 1.403);
        assert_eq!(placed.start, 2.0);
        assert!((placed.end - 4.403).abs() < 1e-12);
        assert!((placed.source_in - 1.194).abs() < 1e-12);
        assert_eq!(placed.source_out, 6.0);
        assert_eq!(placed.parent_id, recording.parent_id);
        assert_eq!(placed.volume_db, -6.0);
        assert!(placed.muted);
    }
    #[test]
    fn placement_keeps_authored_trims_and_rejects_no_overlap() {
        let recording = span(0.0, 0.0, 10.0, 1.0);
        let reference = span(0.0, 0.0, 3.0, 1.0);
        let placed = place_recording(
            &recording,
            &reference,
            -0.25,
            Some(1.0),
            Some(4.0),
            Some(2.0),
        )
        .unwrap();
        assert_eq!(
            [
                placed.start,
                placed.end,
                placed.source_in,
                placed.source_out
            ],
            [0.75, 1.75, 1.0, 2.0]
        );
        assert!(place_recording(&recording, &reference, 4.0, None, None, Some(10.0)).is_err());
        assert!(place_recording(&recording, &reference, 0.0, Some(2.0), Some(1.0), None).is_err());
        assert!(place_recording(&recording, &reference, f64::NAN, None, None, None).is_err());
    }
    #[test]
    fn pcm_envelope_keeps_channel_energy_and_rejects_bad_frames() {
        let pcm = (0..960)
            .flat_map(|_| [0.5f32, -0.5].into_iter().flat_map(f32::to_le_bytes))
            .collect::<Vec<_>>();
        let result = envelope_from_pcm(&pcm, 48_000, 2, None).unwrap();
        assert_eq!(result, vec![0.5; 10]);
        assert!(envelope_from_pcm(&pcm[..pcm.len() - 1], 48_000, 2, None).is_err());
        assert!(envelope_from_pcm(&f32::NAN.to_le_bytes(), 500, 1, None).is_err());
        let cancelled = AtomicBool::new(true);
        assert!(
            envelope_from_pcm(&pcm, 48_000, 2, Some(&cancelled))
                .unwrap_err()
                .contains("cancelled")
        );
    }
    fn signal() -> Vec<f32> {
        let mut state = 17u32;
        (0..2400)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 8) as f32 / 16777216.0
            })
            .collect()
    }
    #[test]
    fn alignment_finds_positive_and_negative_recording_offsets_with_gain() {
        let reference = signal();
        let audio = reference[350..1800]
            .iter()
            .map(|value| value * 0.3 + 0.2)
            .collect::<Vec<_>>();
        let result = align_envelopes(&audio, &reference).unwrap();
        assert!((result.offset_seconds - 0.7).abs() < 0.002);
        assert!(result.confidence > 0.9999);
        let reverse = align_envelopes(&reference, &audio).unwrap();
        assert!((reverse.offset_seconds + 0.7).abs() < 0.002);
    }
    #[test]
    fn alignment_rejects_silence_short_overlap_and_invalid_samples() {
        assert!(align_envelopes(&[0.0; 1000], &[0.0; 1000]).is_err());
        assert!(align_envelopes(&signal()[..499], &signal()).is_err());
        assert!(align_envelopes(&[f32::NAN], &signal()).is_err());
        assert!(align_envelopes(&[], &signal()).is_err());
    }

    #[test]
    fn cancellation_stops_fft_and_alignment_without_poisoning_retries() {
        let cancelled = AtomicBool::new(true);
        let samples = signal();
        assert!(
            align_envelopes_cancellable(&samples, &samples, Some(&cancelled))
                .unwrap_err()
                .contains("cancelled")
        );
        let mut real = vec![0.0; 1024];
        let mut imaginary = real.clone();
        assert!(
            fft(&mut real, &mut imaginary, false, Some(&cancelled))
                .unwrap_err()
                .contains("cancelled")
        );
        cancelled.store(false, Ordering::Relaxed);
        let result = align_envelopes_cancellable(&samples, &samples, Some(&cancelled)).unwrap();
        assert!(result.offset_seconds.abs() < 0.002);
        assert!(result.confidence > 0.9999);
    }
}

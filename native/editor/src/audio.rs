// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Offline stereo mixer. Decode/time-stretch each source once, then Rust owns
//! the sample clock, clip windows, reactive bus gain, silence and summation.
use crate::{
    Result,
    document::Node,
    media::{MediaLibrary, Tools, checked_output_cancellable, command},
    render::{JobDirectory, tempo_filters},
    timing::Timeline,
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::Duration,
};

pub const SAMPLE_RATE: f64 = 48_000.0;
struct Decoded {
    source: PathBuf,
    rate: f64,
    file: fs::File,
    samples: u64,
}
#[derive(Default)]
pub(crate) struct Mixer {
    decoded: Vec<Decoded>,
    pub has_audio: bool,
}
impl Mixer {
    fn decoded(
        &mut self,
        source: &std::path::Path,
        rate: f64,
        directory: &mut JobDirectory,
        tools: &Tools,
        cancel: Option<&AtomicBool>,
    ) -> Result<&mut Decoded> {
        if let Some(index) = self
            .decoded
            .iter()
            .position(|item| item.source == source && item.rate == rate)
        {
            return Ok(&mut self.decoded[index]);
        }
        if self.decoded.len() >= 128 {
            return Err("Export exceeds 128 decoded source/rate variants".into());
        }
        let path = directory.file(&format!("decoded-{}.f32", self.decoded.len()));
        let mut decode = command(&tools.ffmpeg);
        decode
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-protocol_whitelist",
                "file,pipe",
                "-i",
            ])
            .arg(source)
            .args([
                "-map",
                "0:a:0",
                "-vn",
                "-af",
                &tempo_filters(rate),
                "-ar",
                "48000",
                "-ac",
                "2",
                "-c:a",
                "pcm_f32le",
                "-f",
                "f32le",
            ])
            .arg(&path);
        checked_output_cancellable(&mut decode, cancel, Duration::from_secs(24 * 3600))?;
        let file = fs::File::open(&path).map_err(|e| e.to_string())?;
        let samples = file.metadata().map_err(|e| e.to_string())?.len() / 8;
        self.decoded.push(Decoded {
            source: source.into(),
            rate,
            file,
            samples,
        });
        Ok(self.decoded.last_mut().unwrap())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mix(
        &mut self,
        root: &Node,
        timeline: &Timeline,
        library: &MediaLibrary,
        scene: &str,
        work_start: f64,
        range: std::ops::Range<u64>,
        directory: &mut JobDirectory,
        tools: &Tools,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>> {
        let count = usize::try_from(range.end - range.start).map_err(|e| e.to_string())?;
        if count > 48_000 {
            return Err("Audio mixing block exceeds one second".into());
        }
        let mut output = vec![0.0_f32; count * 2];
        for node in root.nodes() {
            let Some(audio_node) = library.audio_source(node) else {
                continue;
            };
            if !library
                .probes
                .get(&audio_node.id)
                .is_some_and(|probe| probe.has_audio)
            {
                continue;
            }
            let span = timeline.get(&node.id)?;
            if span.scene_id != scene || span.hidden || span.muted {
                continue;
            }
            // A rounded visual in point may precede fractional source zero.
            // Leave that prefix silent instead of seeking before the recording.
            let first = (((span.start.max(span.origin) - work_start) * SAMPLE_RATE)
                .ceil()
                .max(0.0) as u64)
                .max(range.start);
            let last =
                (((span.end - work_start) * SAMPLE_RATE).ceil().max(0.0) as u64).min(range.end);
            if first >= last {
                continue;
            }
            self.has_audio = true;
            let scene_time = work_start + first as f64 / SAMPLE_RATE;
            let source = library
                .sources
                .get(&audio_node.id)
                .ok_or("Missing audio source")?;
            let decoded = self.decoded(source, span.playback_rate, directory, tools, cancel)?;
            // The decoded file already has this playback rate applied, hence
            // its sample offset is (scene time - source origin) * sample rate.
            let offset = ((scene_time - span.origin) * SAMPLE_RATE).round().max(0.0) as u64;
            let available = (last - first).min(decoded.samples.saturating_sub(offset));
            if available == 0 {
                continue;
            }
            decoded
                .file
                .seek(SeekFrom::Start(offset * 8))
                .map_err(|e| e.to_string())?;
            let mut bytes = vec![0; available as usize * 8];
            decoded
                .file
                .read_exact(&mut bytes)
                .map_err(|e| e.to_string())?;
            let gain = 10_f64.powf(span.volume_db / 20.0) as f32;
            let start = (first - range.start) as usize * 2;
            for (index, bytes) in bytes.as_chunks::<4>().0.iter().enumerate() {
                output[start + index] += f32::from_le_bytes(*bytes) * gain;
            }
        }
        if output.iter().any(|value| !value.is_finite()) {
            return Err("Non-finite mixed audio sample".into());
        }
        Ok(output.into_iter().flat_map(f32::to_le_bytes).collect())
    }
}

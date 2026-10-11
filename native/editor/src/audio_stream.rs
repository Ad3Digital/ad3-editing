// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Bounded stereo PCM decoding for native playback producers.
use crate::{
    Result,
    media::{SourceVersion, Tools},
};
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const SAMPLE_RATE: u32 = 48_000;
pub const BLOCK_FRAMES: usize = 1024;
pub struct PcmBlock {
    pub first_frame: u64,
    pub samples: Vec<f32>,
}

pub(crate) fn validate_window(start: f64, duration: Option<f64>, rate: f64) -> Result<()> {
    if !start.is_finite()
        || start < 0.0
        || !rate.is_finite()
        || !(0.01..=100.0).contains(&rate)
        || duration.is_some_and(|duration| !duration.is_finite() || duration <= 0.0)
    {
        Err("Invalid audio stream window or playback rate".into())
    } else {
        Ok(())
    }
}

fn read_blocks(
    mut reader: impl Read,
    cancel: Option<&AtomicBool>,
    mut emit: impl FnMut(PcmBlock) -> Result<()>,
) -> Result<()> {
    let mut bytes = [0u8; BLOCK_FRAMES * 8];
    let mut first_frame = 0u64;
    loop {
        let mut filled = 0;
        while filled < bytes.len() {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Err("Audio stream cancelled".into());
            }
            let read = reader
                .read(&mut bytes[filled..])
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        if filled % 8 != 0 {
            return Err("Audio stream ended inside a stereo frame".into());
        }
        let samples = bytes[..filled]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect::<Vec<_>>();
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("Audio stream contains non-finite samples".into());
        }
        let frames = filled as u64 / 8;
        emit(PcmBlock {
            first_frame,
            samples,
        })?;
        first_frame = first_frame
            .checked_add(frames)
            .filter(|frame| *frame <= (1u64 << 53))
            .ok_or("Audio stream exceeds its precise sample clock")?;
    }
    Ok(())
}

/// `start`/`duration` are content seconds; packet clocks are output sample frames.
/// Tempo changes source speed while retaining pitch and stereo 48kHz output.
pub fn decode(
    tools: &Tools,
    source: &Path,
    start: f64,
    duration: Option<f64>,
    rate: f64,
    cancel: Option<&AtomicBool>,
    emit: impl FnMut(PcmBlock) -> Result<()> + Send,
) -> Result<()> {
    validate_window(start, duration, rate)?;
    let probe = tools.probe_cancellable(source, cancel)?;
    if !probe.has_audio {
        return Err("Audio stream needs an audio track".into());
    }
    let version = SourceVersion::read(source)?;
    if probe.version.as_ref() != Some(&version) {
        return Err("Audio changed since probing".into());
    }
    let mut process = crate::media::command(&tools.ffmpeg);
    process
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-ss",
        ])
        .arg(format!("{start:.9}"))
        .arg("-i")
        .arg(source)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-af"])
        .arg(if rate == 1.0 {
            "anull".into()
        } else {
            crate::render::tempo_filters(rate)
        })
        .args(["-ar", "48000", "-ac", "2"]);
    if let Some(duration) = duration {
        process.arg("-t").arg(format!("{:.9}", duration / rate));
    }
    process.args(["-c:a", "pcm_f32le", "-f", "f32le", "pipe:1"]);
    crate::media::checked_stream_cancellable(
        &mut process,
        cancel,
        Duration::from_secs(120),
        |stdout| read_blocks(stdout, cancel, emit),
    )?;
    if SourceVersion::read(source)? != version {
        return Err("Audio changed during streaming decode".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_packets_keep_channel_order_clock_and_short_tail() {
        let samples = (0..(BLOCK_FRAMES + 7))
            .flat_map(|_| [0.25f32, -0.5])
            .collect::<Vec<_>>();
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        struct ShortReads(std::io::Cursor<Vec<u8>>);
        impl Read for ShortReads {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let count = output.len().min(3);
                self.0.read(&mut output[..count])
            }
        }
        let mut packets = Vec::new();
        read_blocks(
            ShortReads(std::io::Cursor::new(bytes.clone())),
            None,
            |packet| {
                packets.push(packet);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].first_frame, 0);
        assert_eq!(packets[1].first_frame, BLOCK_FRAMES as u64);
        assert_eq!(packets[1].samples.len(), 14);
        assert_eq!(
            packets
                .into_iter()
                .flat_map(|packet| packet.samples)
                .collect::<Vec<_>>(),
            samples
        );
        assert!(read_blocks(&bytes[..bytes.len() - 1], None, |_| Ok(())).is_err());
        assert!(read_blocks(&f32::NAN.to_le_bytes().repeat(2)[..], None, |_| Ok(())).is_err());
        assert!(
            read_blocks(&bytes[..], Some(&AtomicBool::new(true)), |_| panic!(
                "cancelled stream emitted"
            ))
            .is_err()
        );
        assert_eq!(
            read_blocks(&bytes[..], None, |_| Err("stop output".into())).unwrap_err(),
            "stop output"
        );
    }
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Device boundary: bounded stereo PCM16 packets and a continuous sample clock.
use crate::{
    Result,
    audio_stream::{BLOCK_FRAMES, PcmBlock},
};

#[derive(Default)]
pub struct OutputClock {
    next_frame: u64,
}
impl OutputClock {
    #[cfg(windows)]
    pub(crate) fn set_position(&mut self, frame: u64) {
        self.next_frame = frame;
    }
    pub fn next_frame(&self) -> u64 {
        self.next_frame
    }
    /// Encode only after validating the entire packet. Mixer headroom is clipped
    /// at the device boundary, never wrapped when converting to signed PCM.
    pub fn encode(&mut self, packet: &PcmBlock) -> Result<Vec<u8>> {
        if packet.first_frame != self.next_frame
            || packet.samples.is_empty()
            || packet.samples.len() % 2 != 0
            || packet.samples.len() > BLOCK_FRAMES * 2
            || packet.samples.iter().any(|sample| !sample.is_finite())
        {
            return Err("Invalid or discontinuous device audio packet".into());
        }
        let next = self
            .next_frame
            .checked_add((packet.samples.len() / 2) as u64)
            .filter(|frame| *frame <= 1u64 << 53)
            .ok_or("Device sample clock overflow")?;
        let bytes = packet
            .samples
            .iter()
            .flat_map(|sample| {
                let value = if *sample <= -1.0 {
                    i16::MIN
                } else if *sample >= 1.0 {
                    i16::MAX
                } else {
                    (*sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16
                };
                value.to_le_bytes()
            })
            .collect();
        self.next_frame = next;
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stereo_clipping_and_rejected_packets_preserve_clock() {
        let mut clock = OutputClock::default();
        let bytes = clock
            .encode(&PcmBlock {
                first_frame: 0,
                samples: vec![-2.0, 2.0, -0.5, 0.5, 0.0, 0.0],
            })
            .unwrap();
        let values = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        assert_eq!(values, vec![-32768, 32767, -16384, 16384, 0, 0]);
        assert_eq!(clock.next_frame(), 3);
        for samples in [vec![], vec![0.0], vec![f32::NAN, 0.0], vec![0.0; 2050]] {
            assert!(
                clock
                    .encode(&PcmBlock {
                        first_frame: 3,
                        samples
                    })
                    .is_err()
            );
            assert_eq!(clock.next_frame(), 3);
        }
        assert!(
            clock
                .encode(&PcmBlock {
                    first_frame: 2,
                    samples: vec![0.0; 2]
                })
                .is_err()
        );
        assert!(
            clock
                .encode(&PcmBlock {
                    first_frame: 3,
                    samples: vec![0.0; 2]
                })
                .is_ok()
        );
    }
}

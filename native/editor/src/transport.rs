// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use crate::{Result, timing::snap};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Transport {
    pub time: f64,
    pub playing: bool,
    pub speed: f64,
    pub looping: bool,
    pub workarea: [f64; 2],
    /// Consumers discard queued audio/preview when this generation changes.
    pub generation: u64,
}

impl Transport {
    pub fn new(workarea: [f64; 2]) -> Result<Self> {
        if workarea.iter().any(|t| !t.is_finite())
            || workarea[0] < 0.0
            || workarea[1] <= workarea[0]
        {
            return Err("Invalid playback workarea".into());
        }
        Ok(Self {
            time: workarea[0],
            playing: false,
            speed: 1.0,
            looping: false,
            workarea,
            generation: 0,
        })
    }
    pub fn seek(&mut self, time: f64) -> Result<()> {
        if !time.is_finite() {
            return Err("Invalid playhead time".into());
        }
        self.time = snap(time.max(0.0));
        self.generation += 1;
        Ok(())
    }
    pub fn toggle(&mut self) {
        if !self.playing && self.time >= self.workarea[1] {
            self.time = self.workarea[0];
        }
        self.playing = !self.playing;
        self.speed = 1.0;
        self.generation += 1;
    }
    pub fn shuttle(&mut self, reverse: bool) {
        let direction = if reverse { -1.0 } else { 1.0 };
        let magnitude = if self.playing && self.speed.signum() == direction {
            (self.speed.abs() + 1.0).clamp(2.0, 5.0)
        } else {
            2.0
        };
        if reverse && (self.time <= self.workarea[0] || self.time >= self.workarea[1]) {
            self.time = (self.workarea[1] - 1.0 / 30.0).max(self.workarea[0]);
        } else if !reverse && (self.time < self.workarea[0] || self.time >= self.workarea[1]) {
            self.time = self.workarea[0];
        }
        self.playing = true;
        self.speed = direction * magnitude;
        self.generation += 1;
    }
    pub fn k(&mut self) {
        if self.playing && self.speed != 1.0 {
            self.speed = 1.0;
            self.generation += 1;
        } else {
            self.toggle();
        }
    }
    pub fn advance(&mut self, elapsed_seconds: f64) -> Result<()> {
        if !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
            return Err("Invalid playback delta".into());
        }
        if !self.playing {
            return Ok(());
        }
        let time = self.time + elapsed_seconds * self.speed;
        if !time.is_finite() {
            return Err("Playback clock overflow".into());
        }
        if time >= self.workarea[1] || time < self.workarea[0] {
            self.generation += 1;
            if self.looping {
                self.time = self.workarea[0]
                    + (time - self.workarea[0]).rem_euclid(self.workarea[1] - self.workarea[0]);
            } else {
                self.time = time.clamp(self.workarea[0], self.workarea[1]);
                self.playing = false;
            }
        } else {
            self.time = time;
        }
        Ok(())
    }
    pub fn audio_enabled(&self) -> bool {
        self.playing && self.speed > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shuttle_direction_k_loop_and_audio() {
        let mut t = Transport::new([2.0, 5.0]).unwrap();
        for _ in 0..8 {
            t.shuttle(false);
        }
        assert_eq!(t.speed, 5.0);
        t.shuttle(true);
        assert_eq!(t.speed, -2.0);
        assert!(!t.audio_enabled());
        t.k();
        assert_eq!(t.speed, 1.0);
        assert!(t.audio_enabled());
        t.looping = true;
        t.advance(7.0).unwrap();
        assert!(t.time >= 2.0 && t.time < 5.0);
        t.looping = false;
        t.advance(10.0).unwrap();
        assert!(!t.playing);
        assert_eq!(t.time, 5.0);
        t.toggle();
        assert_eq!(t.time, 2.0);
    }
}

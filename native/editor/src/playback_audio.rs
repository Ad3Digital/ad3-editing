// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Forward mixing over lazy, bounded PCM sessions.
use crate::{
    Result,
    audio_session::AudioSession,
    audio_stream::{BLOCK_FRAMES, PcmBlock, SAMPLE_RATE},
    media::Tools,
};
use std::{path::PathBuf, sync::atomic::AtomicBool};

pub struct VoiceConfig {
    pub id: String,
    pub source: PathBuf,
    pub first_frame: u64,
    pub end_frame: u64,
    pub source_start: f64,
    pub rate: f64,
    pub gain_db: f64,
}
struct Voice {
    config: VoiceConfig,
    gain: f32,
    muted: bool,
    session: Option<AudioSession>,
    packet: Option<PcmBlock>,
    offset: usize,
    ended: bool,
}
pub struct PlaybackMixer {
    tools: Tools,
    voices: Vec<Voice>,
    frame: u64,
    failed: bool,
}
fn gain(db: f64) -> Result<f32> {
    let value = 10f64.powf(db / 20.0) as f32;
    if db.is_finite() && value.is_finite() {
        Ok(value)
    } else {
        Err("Invalid playback gain".into())
    }
}
pub(crate) fn ceil_sample_frame(value: f64) -> f64 {
    let nearest = value.round();
    let tolerance = (f64::EPSILON * value.abs().max(1.0) * 8.0).min(1e-7);
    if (value - nearest).abs() <= tolerance {
        nearest
    } else {
        value.ceil()
    }
}
fn sample_window(span: &crate::timing::Span, playhead: f64) -> Result<Option<(u64, u64, f64)>> {
    let first =
        ceil_sample_frame((span.start.max(span.origin) - playhead) * SAMPLE_RATE as f64).max(0.0);
    let end = ceil_sample_frame((span.end - playhead) * SAMPLE_RATE as f64).max(0.0);
    if !first.is_finite()
        || !end.is_finite()
        || first > (1u64 << 53) as f64
        || end > (1u64 << 53) as f64
    {
        return Err("Playback timeline exceeds the sample clock".into());
    }
    if first >= end {
        return Ok(None);
    }
    let source_start = span
        .source_at(playhead + first / SAMPLE_RATE as f64)
        .max(0.0);
    Ok(Some((first as u64, end as u64, source_start)))
}
fn speed_timeline(
    timeline: &crate::timing::Timeline,
    origin: f64,
    speed: f64,
) -> Result<crate::timing::Timeline> {
    if !origin.is_finite() || !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
        return Err("Invalid forward audio transport speed".into());
    }
    let mut scaled = timeline.clone();
    for span in scaled.spans.values_mut() {
        span.start = origin + (span.start - origin) / speed;
        span.end = origin + (span.end - origin) / speed;
        span.origin = origin + (span.origin - origin) / speed;
        span.playback_rate *= speed;
    }
    Ok(scaled)
}
impl PlaybackMixer {
    pub fn from_timeline_at_speed(
        tools: Tools,
        root: &crate::document::Node,
        timeline: &crate::timing::Timeline,
        library: &crate::media::MediaLibrary,
        scene: &str,
        playhead: f64,
        speed: f64,
    ) -> Result<Self> {
        let scaled = speed_timeline(timeline, playhead, speed)?;
        Self::from_timeline(tools, root, &scaled, library, scene, playhead)
    }
    pub fn reconcile_timeline_at_speed(
        &mut self,
        root: &crate::document::Node,
        timeline: &crate::timing::Timeline,
        library: &crate::media::MediaLibrary,
        scene: &str,
        origin: f64,
        speed: f64,
    ) -> Result<()> {
        let plan = Self::from_timeline_at_speed(
            self.tools.clone(),
            root,
            timeline,
            library,
            scene,
            origin,
            speed,
        )?;
        self.replace_plan(plan)
    }
    pub fn reconcile_timeline(
        &mut self,
        root: &crate::document::Node,
        timeline: &crate::timing::Timeline,
        library: &crate::media::MediaLibrary,
        scene: &str,
        playhead_origin: f64,
    ) -> Result<()> {
        let plan = Self::from_timeline(
            self.tools.clone(),
            root,
            timeline,
            library,
            scene,
            playhead_origin,
        )?;
        self.replace_plan(plan)
    }
    fn replace_plan(&mut self, mut plan: Self) -> Result<()> {
        if self.failed {
            return Err("Playback mixer must be rebuilt after failure".into());
        }
        for voice in &mut plan.voices {
            let config = &voice.config;
            if let Some(index) = self.voices.iter().position(|old| {
                let old = &old.config;
                old.id == config.id
                    && old.source == config.source
                    && old.rate == config.rate
                    && old.first_frame == config.first_frame
                    && old.end_frame == config.end_frame
                    && old.source_start == config.source_start
            }) {
                let mut retained = self.voices.remove(index);
                retained.gain = voice.gain;
                retained.muted = voice.muted;
                *voice = retained;
            }
        }
        // Dropping unmatched sessions cancels only removed/changed voices.
        self.voices = plan.voices;
        Ok(())
    }
    /// Build a new forward transport at a scene-time playhead. Muted voices
    /// remain scheduled so unmuting preserves their source position.
    pub fn from_timeline(
        tools: Tools,
        root: &crate::document::Node,
        timeline: &crate::timing::Timeline,
        library: &crate::media::MediaLibrary,
        scene: &str,
        playhead: f64,
    ) -> Result<Self> {
        if !playhead.is_finite() || !timeline.workareas.contains_key(scene) {
            return Err("Invalid playback scene or playhead".into());
        }
        let mut configs = Vec::new();
        let mut muted = Vec::new();
        for node in root.nodes() {
            let Some(source_node) = library.audio_source(node) else {
                continue;
            };
            if !library
                .probes
                .get(&source_node.id)
                .is_some_and(|probe| probe.has_audio)
            {
                continue;
            }
            let span = timeline.get(&node.id)?;
            if span.scene_id != scene || span.hidden {
                continue;
            }
            let Some((first_frame, end_frame, source_start)) = sample_window(span, playhead)?
            else {
                continue;
            };
            configs.push(VoiceConfig {
                id: node.id.clone(),
                source: library
                    .sources
                    .get(&source_node.id)
                    .ok_or("Missing playback audio source")?
                    .clone(),
                first_frame,
                end_frame,
                source_start,
                rate: span.playback_rate,
                gain_db: span.volume_db,
            });
            if span.muted {
                muted.push(node.id.clone());
            }
        }
        let mut mixer = Self::new(tools, configs)?;
        for id in muted {
            mixer.set_muted(&id, true)?;
        }
        Ok(mixer)
    }
    pub fn new(tools: Tools, configs: Vec<VoiceConfig>) -> Result<Self> {
        if configs.len() > 128 {
            return Err("Playback exceeds 128 audio voices".into());
        }
        let mut voices = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for config in configs {
            if !ids.insert(config.id.clone())
                || config.id.is_empty()
                || config.end_frame <= config.first_frame
                || config.end_frame > (1u64 << 53)
            {
                return Err("Invalid playback voice identity or sample window".into());
            }
            crate::audio_stream::validate_window(config.source_start, None, config.rate)?;
            let gain = gain(config.gain_db)?;
            voices.push(Voice {
                config,
                gain,
                muted: false,
                session: None,
                packet: None,
                offset: 0,
                ended: false,
            });
        }
        Ok(Self {
            tools,
            voices,
            frame: 0,
            failed: false,
        })
    }
    pub fn set_gain(&mut self, id: &str, db: f64) -> Result<()> {
        let gain = gain(db)?;
        self.voices
            .iter_mut()
            .find(|voice| voice.config.id == id)
            .ok_or("Unknown playback voice")?
            .gain = gain;
        Ok(())
    }
    /// Apply evaluated bus levels between PCM blocks. Validate the whole update
    /// before changing any voice, so a bad reactive value cannot partially apply.
    /// Timing/source edits still require rebuilding the transport.
    pub fn sync_levels(&mut self, timeline: &crate::timing::Timeline) -> Result<()> {
        let levels = self
            .voices
            .iter()
            .map(|voice| {
                let span = timeline.get(&voice.config.id)?;
                Ok((gain(span.volume_db)?, span.muted || span.hidden))
            })
            .collect::<Result<Vec<_>>>()?;
        for (voice, (gain, muted)) in self.voices.iter_mut().zip(levels) {
            voice.gain = gain;
            voice.muted = muted;
        }
        Ok(())
    }
    pub fn set_muted(&mut self, id: &str, muted: bool) -> Result<()> {
        self.voices
            .iter_mut()
            .find(|voice| voice.config.id == id)
            .ok_or("Unknown playback voice")?
            .muted = muted;
        Ok(())
    }
    pub fn mix(&mut self, frames: usize, cancel: Option<&AtomicBool>) -> Result<PcmBlock> {
        if self.failed {
            return Err("Playback mixer must be rebuilt after an error or seek".into());
        }
        if frames == 0 || frames > BLOCK_FRAMES {
            return Err("Playback block must contain 1..1024 frames".into());
        }
        let end = self
            .frame
            .checked_add(frames as u64)
            .filter(|end| *end <= (1u64 << 53))
            .ok_or("Playback sample clock overflow")?;
        let mut samples = vec![0f32; frames * 2];
        let result = (|| {
            crate::encoder::cancelled(cancel)?;
            for voice in &mut self.voices {
                let first = self.frame.max(voice.config.first_frame);
                let last = end.min(voice.config.end_frame);
                if first >= last || voice.ended {
                    continue;
                }
                if voice.session.is_none() {
                    let duration = (voice.config.end_frame - first) as f64 / SAMPLE_RATE as f64
                        * voice.config.rate;
                    voice.session = Some(AudioSession::start(
                        self.tools.clone(),
                        voice.config.source.clone(),
                        voice.config.source_start
                            + (first - voice.config.first_frame) as f64 / SAMPLE_RATE as f64
                                * voice.config.rate,
                        Some(duration),
                        voice.config.rate,
                    )?);
                }
                let mut at = (first - self.frame) as usize * 2;
                let stop = (last - self.frame) as usize * 2;
                while at < stop {
                    crate::encoder::cancelled(cancel)?;
                    if voice
                        .packet
                        .as_ref()
                        .is_none_or(|packet| voice.offset == packet.samples.len())
                    {
                        voice.packet = voice.session.as_mut().unwrap().next(cancel)?;
                        voice.offset = 0;
                    }
                    let Some(packet) = &voice.packet else {
                        voice.ended = true;
                        voice.session.take();
                        break;
                    };
                    let count = (stop - at).min(packet.samples.len() - voice.offset);
                    if !voice.muted {
                        for (target, source) in samples[at..at + count]
                            .iter_mut()
                            .zip(&packet.samples[voice.offset..voice.offset + count])
                        {
                            *target += *source * voice.gain;
                        }
                    }
                    voice.offset += count;
                    at += count;
                }
                if end >= voice.config.end_frame {
                    voice.session.take();
                    voice.packet.take();
                    voice.ended = true;
                }
            }
            if samples.iter().any(|sample| !sample.is_finite()) {
                return Err("Non-finite playback mix".into());
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.failed = true;
            for voice in &mut self.voices {
                voice.session.take();
                voice.packet.take();
            }
            return Err(error);
        }
        let first_frame = self.frame;
        self.frame = end;
        Ok(PcmBlock {
            first_frame,
            samples,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replanning_preserves_unchanged_pcm_and_replaces_changed_sources() {
        let config = |id: &str, source: &str, gain_db| VoiceConfig {
            id: id.into(),
            source: source.into(),
            first_frame: 0,
            end_frame: 4096,
            source_start: 0.25,
            rate: 1.0,
            gain_db,
        };
        let mut mixer = PlaybackMixer::new(
            Tools::default(),
            vec![config("a", "one.wav", 0.0), config("b", "two.wav", 0.0)],
        )
        .unwrap();
        mixer.frame = 1024;
        mixer.voices[0].packet = Some(PcmBlock {
            first_frame: 0,
            samples: vec![0.25; 20],
        });
        mixer.voices[0].offset = 6;
        mixer.voices[1].ended = true;
        let mut plan = PlaybackMixer::new(
            Tools::default(),
            vec![
                config("a", "one.wav", -6.020599913279624),
                config("b", "new.wav", 0.0),
                config("c", "three.wav", 0.0),
            ],
        )
        .unwrap();
        plan.set_muted("a", true).unwrap();
        mixer.replace_plan(plan).unwrap();
        assert_eq!(mixer.frame, 1024);
        assert_eq!(mixer.voices[0].offset, 6);
        assert_eq!(
            mixer.voices[0].packet.as_ref().unwrap().samples,
            vec![0.25; 20]
        );
        assert!(mixer.voices[0].muted);
        assert!((mixer.voices[0].gain - 0.5).abs() < 1e-6);
        assert!(!mixer.voices[1].ended);
        assert!(mixer.voices[1].packet.is_none());
        mixer
            .replace_plan(PlaybackMixer::new(Tools::default(), Vec::new()).unwrap())
            .unwrap();
        assert!(mixer.voices.is_empty());
        assert_eq!(mixer.mix(7, None).unwrap().first_frame, 1024);
    }
    #[test]
    fn fractional_source_origin_seek_and_rate() {
        let span = crate::timing::Span {
            id: "clip".into(),
            parent_id: None,
            scene_id: "scene".into(),
            origin: 1.00001,
            start: 1.0,
            end: 2.0,
            playback_rate: 2.0,
            source_in: 0.0,
            source_out: 2.0,
            hidden: false,
            muted: false,
            volume_db: 0.0,
        };
        let (first, end, source) = sample_window(&span, 0.0).unwrap().unwrap();
        assert_eq!((first, end), (48001, 96000));
        assert!(source >= 0.0 && source < 2.0 / 48000.0);
        let (first, end, source) = sample_window(&span, 1.5).unwrap().unwrap();
        assert_eq!((first, end), (0, 24000));
        assert!((source - 0.99998).abs() < 1e-9);
        assert!(sample_window(&span, 2.0).unwrap().is_none());
        assert!(sample_window(&span, -1e20).is_err());
        let timeline = crate::timing::Timeline {
            spans: [(span.id.clone(), span)].into(),
            workareas: Default::default(),
            active: "scene".into(),
        };
        let scaled = speed_timeline(&timeline, 1.5, 2.0).unwrap();
        let span = scaled.get("clip").unwrap();
        let (first, end, source) = sample_window(span, 1.5).unwrap().unwrap();
        assert_eq!((first, end), (0, 12000));
        assert!((source - 0.99998).abs() < 1e-9);
        assert_eq!(span.playback_rate, 4.0);
        assert!(speed_timeline(&timeline, 1.5, -2.0).is_err());
        assert!(speed_timeline(&timeline, 1.5, f64::NAN).is_err());
    }
    #[test]
    fn evaluated_levels_validate_before_mutating_voices() {
        let configs = ["a", "b"]
            .into_iter()
            .map(|id| VoiceConfig {
                id: id.into(),
                source: "unused.wav".into(),
                first_frame: 0,
                end_frame: 1024,
                source_start: 0.0,
                rate: 1.0,
                gain_db: 0.0,
            })
            .collect();
        let mut mixer = PlaybackMixer::new(Tools::default(), configs).unwrap();
        let mut timeline = crate::timing::Timeline {
            spans: Default::default(),
            workareas: Default::default(),
            active: "scene".into(),
        };
        for id in ["a", "b"] {
            timeline.spans.insert(
                id.into(),
                crate::timing::Span {
                    id: id.into(),
                    parent_id: None,
                    scene_id: "scene".into(),
                    origin: 0.0,
                    start: 0.0,
                    end: 1.0,
                    playback_rate: 1.0,
                    source_in: 0.0,
                    source_out: 1.0,
                    hidden: false,
                    muted: true,
                    volume_db: -6.020599913279624,
                },
            );
        }
        timeline.spans.get_mut("b").unwrap().volume_db = f64::NAN;
        assert!(mixer.sync_levels(&timeline).is_err());
        assert!(
            mixer
                .voices
                .iter()
                .all(|voice| voice.gain == 1.0 && !voice.muted)
        );
        timeline.spans.get_mut("b").unwrap().volume_db = 0.0;
        mixer.sync_levels(&timeline).unwrap();
        assert!((mixer.voices[0].gain - 0.5).abs() < 1e-6);
        assert!(mixer.voices.iter().all(|voice| voice.muted));
        timeline.spans.remove("b");
        assert!(mixer.sync_levels(&timeline).is_err());
    }
    #[test]
    fn silence_clock_invalid_blocks_and_cancellation() {
        let mut mixer = PlaybackMixer::new(Tools::default(), Vec::new()).unwrap();
        assert!(mixer.mix(0, None).is_err());
        let packet = mixer.mix(17, None).unwrap();
        assert_eq!(packet.first_frame, 0);
        assert_eq!(packet.samples, vec![0.0; 34]);
        assert_eq!(mixer.mix(1, None).unwrap().first_frame, 17);
        assert!(mixer.mix(1, Some(&AtomicBool::new(true))).is_err());
        assert!(mixer.mix(1, None).is_err());
    }
}

//! Streaming waveform peaks and silence spans from runtime/media/transcode.ts.
use crate::{Result, encoder::cancelled};
use std::{ops::Range, sync::atomic::AtomicBool};

const MAX_PEAKS: usize = 1_000_000;

pub const HELP: &str = "dapi-rust media waveform <audio-or-video> [-s|--start <time>] [-e|--end <time>] [-x|--scale <factor>] [-o|--output <png>] [--project <dir>]\nOutputs JSON with absolute path and silence ranges in source seconds.\n";

pub fn command(
    args: &[String],
    tools: &crate::media::Tools,
    cancel: Option<&AtomicBool>,
) -> Result<(std::path::PathBuf, Vec<Silence>)> {
    let request = crate::filmstrip::prepare_preview(args, "waveform", HELP, cancel)?;
    let probe = tools.probe_cancellable(&request.source, cancel)?;
    if !probe.has_audio {
        return Err("Waveform source has no audio track".into());
    }
    let duration = probe
        .duration
        .ok_or("Waveform needs source duration metadata")?;
    let fps = if probe.has_video && probe.frame_rate > 0.0 {
        probe.frame_rate
    } else {
        30.0
    };
    let layout = WaveformLayout::new(duration, fps, request.start, request.end, request.scale)?;
    let peaks = decode_peaks(tools, &request.source, &probe, &layout, cancel)?;
    let (canvas, silences) = render_peaks(&peaks, &layout, cancel)?;
    crate::filmstrip::write_png(&canvas, &request.output, cancel)?;
    Ok((request.output, silences))
}

/// Stream the requested source window as interleaved PCM without a complete
/// recording buffer. Channels and sample rate match the probed audio track.
pub fn decode_peaks(
    tools: &crate::media::Tools,
    path: &std::path::Path,
    probe: &crate::media::Probe,
    layout: &WaveformLayout,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>> {
    use std::io::Read;
    cancelled(cancel)?;
    if !probe.has_audio {
        return Err("Waveform source has no audio track".into());
    }
    let version = crate::media::SourceVersion::read(path)?;
    if probe.version.as_ref() != Some(&version) {
        return Err("Waveform source changed since probing".into());
    }
    let channels = probe.channels as usize;
    let mut sampler = PeakSampler::new(probe.sample_rate, channels, layout.peaks_per_second)?;
    let mut command = crate::media::command(&tools.ffmpeg);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-ss",
            &layout.ruler.start.to_string(),
            "-i",
        ])
        .arg(path)
        .args([
            "-t",
            &(layout.ruler.end - layout.ruler.start).to_string(),
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-ac",
            &probe.channels.to_string(),
            "-ar",
            &probe.sample_rate.to_string(),
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "pipe:1",
        ]);
    let peaks = crate::media::checked_stream_cancellable(
        &mut command,
        cancel,
        std::time::Duration::from_secs(120),
        |mut stdout| {
            let mut bytes = Vec::with_capacity(65536 + channels * 4);
            let mut buffer = [0u8; 65536];
            loop {
                let count = stdout
                    .read(&mut buffer)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let complete = bytes.len() / (channels * 4) * (channels * 4);
                let samples = bytes[..complete]
                    .chunks_exact(4)
                    .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                    .collect::<Vec<_>>();
                sampler.push(&samples, cancel)?;
                bytes.drain(..complete);
            }
            if !bytes.is_empty() {
                return Err("Decoder returned incomplete waveform PCM".into());
            }
            Ok(sampler.finish())
        },
    )?;
    if crate::media::SourceVersion::read(path)? != version {
        return Err("Waveform source changed during decoding".into());
    }
    cancelled(cancel)?;
    Ok(peaks)
}

pub struct WaveformLayout {
    pub ruler: crate::filmstrip::FilmstripLayout,
    pub peaks_per_second: f64,
}

impl WaveformLayout {
    pub fn new(
        duration: f64,
        frame_rate: f64,
        start: Option<f64>,
        end: Option<f64>,
        scale: Option<f64>,
    ) -> Result<Self> {
        let mut ruler = crate::filmstrip::FilmstripLayout::new(
            [1, 1],
            duration,
            frame_rate,
            start,
            end,
            scale,
        )?;
        let scale = scale.unwrap_or(1.0).clamp(0.25, 4.0);
        let height = (6.0 * scale).round().clamp(1.0, 51.0) as u32;
        let width = (6.0 * scale).round().clamp(1.0, 64.0) as u32;
        ruler.rows = 52 / (height + 1);
        ruler.columns = 64 / width;
        ruler.thumbnail = [width * 28, height * 28];
        ruler.row_height = (height + 1) * 28;
        ruler.canvas = [ruler.columns * width * 28, ruler.rows * ruler.row_height];
        ruler.seconds_per_column = (ruler.end - ruler.start) / f64::from(ruler.cells());
        let peaks_per_second =
            (f64::from(ruler.thumbnail[0]) / ruler.seconds_per_column / 2.0).round();
        if !peaks_per_second.is_finite() || peaks_per_second <= 0.0 {
            return Err("Waveform window cannot be represented by the peak clock".into());
        }
        Ok(Self {
            ruler,
            peaks_per_second,
        })
    }
}

#[derive(Clone, Debug, serde::Serialize, PartialEq)]
pub struct Silence {
    pub start: f64,
    pub end: f64,
}

pub fn render_peaks(
    peaks: &[u8],
    layout: &WaveformLayout,
    cancel: Option<&AtomicBool>,
) -> Result<(tiny_skia::Pixmap, Vec<Silence>)> {
    use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};
    cancelled(cancel)?;
    let spans = silent_spans(peaks, layout.peaks_per_second)?;
    let ruler = &layout.ruler;
    if u64::from(ruler.canvas[0]) * u64::from(ruler.canvas[1]) > 4096 * 4096 {
        return Err("Waveform canvas exceeds the pixel budget".into());
    }
    let mut canvas =
        Pixmap::new(ruler.canvas[0], ruler.canvas[1]).ok_or("Cannot allocate waveform canvas")?;
    canvas.fill(Color::BLACK);
    let per_row = ruler.canvas[0] as usize / 2;
    if per_row == 0 {
        return Err("Waveform row is empty".into());
    }
    let height = ruler.thumbnail[1] as f32;
    let half = height / 2.0;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    for (index, peak) in peaks.iter().take(per_row * ruler.rows as usize).enumerate() {
        if index.is_multiple_of(4096) {
            cancelled(cancel)?;
        }
        let top = (index / per_row) as f32 * ruler.row_height as f32 + ruler.ruler_height as f32;
        let amplitude = f32::from(*peak) / 255.0 * half;
        if let Some(rect) = Rect::from_xywh(
            (index % per_row * 2) as f32,
            top + half - amplitude,
            2.0,
            (amplitude * 2.0).max(1.0),
        ) {
            canvas.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
    paint.set_color_rgba8(255, 64, 64, 102);
    for span in &spans {
        cancelled(cancel)?;
        let mut index = span.start;
        while index < span.end {
            let row = index / per_row;
            if row >= ruler.rows as usize {
                break;
            }
            let end = span.end.min((row + 1) * per_row);
            let top = row as f32 * ruler.row_height as f32 + ruler.ruler_height as f32;
            if let Some(rect) = Rect::from_xywh(
                (index % per_row * 2) as f32,
                top,
                ((end - index) * 2) as f32,
                height,
            ) {
                canvas.fill_rect(rect, &paint, Transform::identity(), None);
            }
            index = end;
        }
    }
    crate::filmstrip::draw_ruler(&mut canvas, ruler, ruler.frame_rate, cancel)?;
    let silences = spans
        .into_iter()
        .map(|span| Silence {
            start: ruler.start + span.start as f64 / layout.peaks_per_second,
            end: ruler.start + span.end as f64 / layout.peaks_per_second,
        })
        .collect();
    Ok((canvas, silences))
}

pub struct PeakSampler {
    channels: usize,
    frames_per_peak: f64,
    frame_cursor: u64,
    peaks: Vec<u8>,
    peak_max: f64,
}

impl PeakSampler {
    pub fn new(sample_rate: u32, channels: usize, peaks_per_second: f64) -> Result<Self> {
        if sample_rate == 0
            || !(1..=64).contains(&channels)
            || !peaks_per_second.is_finite()
            || peaks_per_second <= 0.0
        {
            return Err("Invalid waveform sample rate, channel count or peak rate".into());
        }
        let frames_per_peak = f64::from(sample_rate) / peaks_per_second;
        if !frames_per_peak.is_finite() || frames_per_peak <= 0.0 {
            return Err("Waveform peak clock overflows".into());
        }
        Ok(Self {
            channels,
            frames_per_peak,
            frame_cursor: 0,
            peaks: Vec::new(),
            peak_max: 0.0,
        })
    }

    pub fn push(&mut self, samples: &[f32], cancel: Option<&AtomicBool>) -> Result<()> {
        cancelled(cancel)?;
        if !samples.len().is_multiple_of(self.channels) {
            return Err("Waveform PCM has an incomplete channel frame".into());
        }
        for chunk in samples.chunks(4096) {
            cancelled(cancel)?;
            if chunk.iter().any(|sample| !sample.is_finite()) {
                return Err("Waveform PCM contains nonfinite samples".into());
            }
        }
        for (index, frame) in samples.chunks_exact(self.channels).enumerate() {
            if index.is_multiple_of(4096) {
                cancelled(cancel)?;
            }
            let target = (self.frame_cursor as f64 / self.frames_per_peak).floor();
            if !target.is_finite()
                || target >= MAX_PEAKS as f64
                || self.frame_cursor >= 9_007_199_254_740_991
            {
                return Err("Waveform peak/frame budget exceeded".into());
            }
            while self.peaks.len() < target as usize {
                self.peaks
                    .push((255.0 * self.peak_max).floor().clamp(0.0, 255.0) as u8);
                self.peak_max = 0.0;
            }
            for sample in frame {
                self.peak_max = self.peak_max.max(f64::from(*sample).abs().sqrt());
            }
            self.frame_cursor += 1;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Vec<u8> {
        if self.frame_cursor > 0 {
            self.peaks
                .push((255.0 * self.peak_max).floor().clamp(0.0, 255.0) as u8);
        }
        self.peaks
    }
}

pub fn silent_spans(peaks: &[u8], peaks_per_second: f64) -> Result<Vec<Range<usize>>> {
    if !peaks_per_second.is_finite() || peaks_per_second <= 0.0 || peaks.len() > MAX_PEAKS {
        return Err("Invalid waveform silence clock or peak budget".into());
    }
    let minimum = (0.4 * peaks_per_second).round();
    let mut spans = Vec::new();
    let mut start = None;
    for index in 0..=peaks.len() {
        if peaks.get(index).is_some_and(|peak| *peak <= 12) {
            start.get_or_insert(index);
        } else if let Some(start) = start.take()
            && (index - start) as f64 >= minimum
        {
            spans.push(start..index);
        }
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires installed FFmpeg and FFprobe"]
    fn streaming_decoder_preserves_stereo_window_and_silence_from_real_wav() {
        let folder = std::env::temp_dir().join(format!(
            "ad3-waveform-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("stereo {source}.wav");
        let mut bytes = Vec::new();
        let data_size = 16000u32 * 8;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&64000u32.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for frame in 0..16000 {
            bytes.extend_from_slice(&0f32.to_le_bytes());
            bytes.extend_from_slice(&(if frame < 8000 { 0f32 } else { -0.25f32 }).to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        let tools = crate::media::Tools::default();
        let probe = tools.probe(&path).unwrap();
        assert_eq!((probe.sample_rate, probe.channels), (8000, 2));
        let mut streamed_frames = 0;
        crate::audio_stream::decode(&tools, &path, 1.2, Some(0.2), 1.0, None, |packet| {
            assert_eq!(packet.first_frame, streamed_frames);
            assert!(packet.samples.len() <= crate::audio_stream::BLOCK_FRAMES * 2);
            assert!(
                packet
                    .samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|frame| frame[0] == 0.0 && (frame[1] + 0.25).abs() < 0.001)
            );
            streamed_frames += packet.samples.len() as u64 / 2;
            Ok(())
        })
        .unwrap();
        assert_eq!(streamed_frames, 9600);
        let mut faster_frames = 0u64;
        crate::audio_stream::decode(&tools, &path, 1.2, Some(0.2), 2.0, None, |packet| {
            faster_frames += packet.samples.len() as u64 / 2;
            Ok(())
        })
        .unwrap();
        assert_eq!(faster_frames, 4800);
        let voice =
            |id: &str, first_frame, end_frame, gain_db| crate::playback_audio::VoiceConfig {
                id: id.into(),
                source: path.clone(),
                first_frame,
                end_frame,
                source_start: 1.2,
                rate: 1.0,
                gain_db,
            };
        let mut mixer = crate::playback_audio::PlaybackMixer::new(
            tools.clone(),
            vec![
                voice("main", 0, 4096, 0.0),
                voice("overlap", 512, 1536, -6.020599913279624),
            ],
        )
        .unwrap();
        let mixed = mixer.mix(1024, None).unwrap();
        assert_eq!(mixed.first_frame, 0);
        for (index, frame) in mixed.samples.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(frame[0], 0.0);
            assert!(
                (frame[1] - if index < 512 { -0.25 } else { -0.375 }).abs() < 0.001,
                "sample {index}: {}",
                frame[1]
            );
        }
        mixer.set_muted("main", true).unwrap();
        let mixed = mixer.mix(1024, None).unwrap();
        for (index, frame) in mixed.samples.as_chunks::<2>().0.iter().enumerate() {
            assert!((frame[1] - if index < 512 { -0.125 } else { 0.0 }).abs() < 0.001);
        }
        mixer.set_muted("main", false).unwrap();
        assert!(
            mixer
                .mix(1024, None)
                .unwrap()
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| (frame[1] + 0.25).abs() < 0.001)
        );
        assert!(mixer.set_gain("main", f64::NAN).is_err());
        mixer.set_gain("main", -6.020599913279624).unwrap();
        assert!(
            mixer
                .mix(1024, None)
                .unwrap()
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| (frame[1] + 0.125).abs() < 0.001)
        );
        assert!(
            mixer
                .mix(1024, None)
                .unwrap()
                .samples
                .iter()
                .all(|sample| *sample == 0.0)
        );
        // Add a voice after source second one. Starting it at source zero
        // would incorrectly play the fixture's silent first second.
        let document = crate::source::SourceDocument::parse(folder.join("replan.tsx"),
            r#"export default <stage><scene id="s" active workarea={[0,2]}><audio id="main" duration={2}/></scene></stage>"#.into()).unwrap();
        let mut root = document.root;
        let mut library = crate::media::MediaLibrary {
            sources: [("main".into(), path.clone())].into(),
            probes: [("main".into(), probe.clone())].into(),
            durations: [("main".into(), 2.0)].into(),
        };
        let timeline = crate::timing::Timeline::resolve_with_media_sync(
            &root,
            &library,
            30.0,
            &Default::default(),
        )
        .unwrap();
        let mut replanned = crate::playback_audio::PlaybackMixer::from_timeline(
            tools.clone(),
            &root,
            &timeline,
            &library,
            "s",
            0.0,
        )
        .unwrap();
        for _ in 0..48 {
            replanned.mix(1024, None).unwrap();
        }
        let mut added = root.children[0].children[0].clone();
        added.id = "added".into();
        root.children[0].children.push(added);
        library.sources.insert("added".into(), path.clone());
        library.probes.insert("added".into(), probe.clone());
        library.durations.insert("added".into(), 2.0);
        let timeline = crate::timing::Timeline::resolve_with_media_sync(
            &root,
            &library,
            30.0,
            &Default::default(),
        )
        .unwrap();
        replanned
            .reconcile_timeline(&root, &timeline, &library, "s", 0.0)
            .unwrap();
        let packet = replanned.mix(137, None).unwrap();
        assert_eq!(packet.first_frame, 49152);
        assert!(
            packet
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| frame[0] == 0.0 && (frame[1] + 0.5).abs() < 0.001)
        );
        // Hide the original voice and retain the added voice's partial packet.
        root.children[0].children[0]
            .props
            .insert("hidden".into(), true.into());
        let timeline = crate::timing::Timeline::resolve_with_media_sync(
            &root,
            &library,
            30.0,
            &Default::default(),
        )
        .unwrap();
        replanned
            .reconcile_timeline(&root, &timeline, &library, "s", 0.0)
            .unwrap();
        let packet = replanned.mix(83, None).unwrap();
        assert_eq!(packet.first_frame, 49289);
        assert!(
            packet
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| (frame[1] + 0.25).abs() < 0.001)
        );
        let replacement = folder.join("replacement.wav");
        let mut replacement_bytes = bytes.clone();
        for frame in 8000..16000 {
            let offset = 44 + frame * 8 + 4;
            replacement_bytes[offset..offset + 4].copy_from_slice(&0.125f32.to_le_bytes());
        }
        std::fs::write(&replacement, replacement_bytes).unwrap();
        library.sources.insert("added".into(), replacement.clone());
        library
            .probes
            .insert("added".into(), tools.probe(&replacement).unwrap());
        replanned
            .reconcile_timeline(&root, &timeline, &library, "s", 0.0)
            .unwrap();
        let packet = replanned.mix(71, None).unwrap();
        assert_eq!(packet.first_frame, 49372);
        assert!(
            packet
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| (frame[1] - 0.125).abs() < 0.001)
        );
        drop(replanned);
        let automated_source = format!(
            r#"export default <stage><scene id="s" active volume={{0}} workarea={{[0,2]}}><keyframeTrack property="volume"><keyframe time={{0}} value={{-12}}/><keyframe time={{2}} value={{0}}/></keyframeTrack><audio id="main" duration={{2}} src={}/></scene></stage>"#,
            serde_json::to_string(&path.to_string_lossy()).unwrap(),
        );
        let document =
            crate::source::SourceDocument::parse(folder.join("automated.tsx"), automated_source)
                .unwrap();
        let mut renderer =
            crate::project::ProjectRenderer::new(&document, &folder, tools.clone(), false).unwrap();
        renderer.sample(1.2, true).unwrap();
        let mut automated = crate::playback_audio::PlaybackMixer::from_timeline(
            tools.clone(),
            &renderer.root,
            &renderer.timeline,
            &renderer.library,
            "s",
            1.2,
        )
        .unwrap();
        let mut position = 0u64;
        while position < 4800 {
            let time = 1.2 + position as f64 / 48000.0;
            renderer.sample(time, true).unwrap();
            let db = renderer.timeline.get("main").unwrap().volume_db;
            assert!(
                (db - (-12.0 + crate::timing::snap(time) * 6.0)).abs() < 1e-6,
                "time {time}, gain {db}"
            );
            automated
                .reconcile_timeline(
                    &renderer.root,
                    &renderer.timeline,
                    &renderer.library,
                    "s",
                    1.2,
                )
                .unwrap();
            let count = (4800 - position).min(1024) as usize;
            let packet = automated.mix(count, None).unwrap();
            assert_eq!(packet.first_frame, position);
            let expected = -0.25 * 10f32.powf(db as f32 / 20.0);
            assert!(
                packet
                    .samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|frame| frame[0] == 0.0 && (frame[1] - expected).abs() < 0.001)
            );
            position += count as u64;
        }
        drop(automated);
        drop(renderer);
        let mut accelerated = crate::playback_audio::PlaybackMixer::from_timeline_at_speed(
            tools.clone(),
            &root,
            &timeline,
            &library,
            "s",
            1.2,
            2.0,
        )
        .unwrap();
        let mut output_frame = 0u64;
        while output_frame < 19200 {
            accelerated
                .reconcile_timeline_at_speed(&root, &timeline, &library, "s", 1.2, 2.0)
                .unwrap();
            let count = (19200 - output_frame).min(1024) as usize;
            let packet = accelerated.mix(count, None).unwrap();
            assert_eq!(packet.first_frame, output_frame);
            if output_frame < 4096 {
                assert!(
                    packet
                        .samples
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|frame| frame[0] == 0.0 && (frame[1] - 0.125).abs() < 0.001)
                );
            }
            output_frame += count as u64;
        }
        assert_eq!(output_frame, 19200);
        let tail = accelerated.mix(137, None).unwrap();
        assert_eq!(tail.first_frame, 19200);
        assert!(tail.samples.iter().all(|sample| *sample == 0.0));
        drop(accelerated);
        let mut session = crate::audio_session::AudioSession::start(
            tools.clone(),
            path.clone(),
            1.2,
            Some(0.2),
            1.0,
        )
        .unwrap();
        let mut received = 0;
        while let Some(block) = session.next(None).unwrap() {
            assert_eq!(block.first_frame, received);
            assert!(
                block
                    .samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|frame| frame[0] == 0.0 && (frame[1] + 0.25).abs() < 0.001)
            );
            received += block.samples.len() as u64 / 2;
            if received == 1024 || received == 2048 {
                session.expire_for_test();
            }
        }
        assert_eq!(received, 9600);
        assert!(session.next(None).unwrap().is_none());
        let mut session =
            crate::audio_session::AudioSession::start(tools.clone(), path.clone(), 0.0, None, 1.0)
                .unwrap();
        session.next(None).unwrap().unwrap();
        let before_drop = std::time::Instant::now();
        drop(session);
        assert!(before_drop.elapsed() < std::time::Duration::from_secs(5));
        let mut session =
            crate::audio_session::AudioSession::start(tools.clone(), path.clone(), 0.0, None, 1.0)
                .unwrap();
        assert!(session.next(Some(&AtomicBool::new(true))).is_err());
        assert!(
            crate::audio_session::AudioSession::start(tools.clone(), path.clone(), 0.0, None, 0.0)
                .is_err()
        );
        assert_eq!(
            crate::audio_stream::decode(&tools, &path, 0.0, None, 1.0, None, |_| Err(
                "stop PCM consumer".into()
            ))
            .unwrap_err(),
            "stop PCM consumer"
        );
        let layout = WaveformLayout::new(2.0, 30.0, None, None, None).unwrap();
        let peaks = decode_peaks(&tools, &path, &probe, &layout, None).unwrap();
        assert!(peaks[..2940].iter().all(|peak| *peak == 0));
        assert!(peaks[2940..].iter().all(|peak| *peak == 127));
        let (_, silences) = render_peaks(&peaks, &layout, None).unwrap();
        assert_eq!(
            silences,
            vec![Silence {
                start: 0.0,
                end: 1.0
            }]
        );
        let window = WaveformLayout::new(2.0, 30.0, Some(1.0), None, None).unwrap();
        let peaks = decode_peaks(&tools, &path, &probe, &window, None).unwrap();
        assert!(peaks.iter().all(|peak| *peak == 127));
        assert!(
            decode_peaks(&tools, &path, &probe, &layout, Some(&AtomicBool::new(true))).is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let manifest = serde_yaml::to_string(&serde_json::json!({"assets": [
            {"id":"recording", "path":"audio/sample", "source":"stereo {source}.wav"},
            {"path":"unrelated", "source":"missing.wav"}
        ]}))
        .unwrap();
        std::fs::write(folder.join("assets.yml"), &manifest).unwrap();
        let output = folder.join("waveform.png");
        let args = vec![
            "audio/sample".into(),
            "--project".into(),
            folder.to_string_lossy().into_owned(),
            "--start=0".into(),
            "--end=2".into(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
        ];
        let (written, silences) = command(&args, &tools, None).unwrap();
        assert_eq!(written, output);
        assert_eq!(
            silences,
            vec![Silence {
                start: 0.0,
                end: 1.0
            }]
        );
        let png = tiny_skia::Pixmap::load_png(&output).unwrap();
        assert_eq!([png.width(), png.height()], layout.ruler.canvas);
        assert_eq!(png.pixel(10, 50).unwrap().red(), 102);
        let saved = std::fs::read(&output).unwrap();
        assert!(command(&args, &tools, None).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), saved);
        let mut id_args = args.clone();
        id_args[0] = "recording".into();
        let id_output = folder.join("id.png");
        *id_args.last_mut().unwrap() = id_output.to_string_lossy().into_owned();
        let (_, id_silences) = command(&id_args, &tools, None).unwrap();
        assert_eq!(id_silences, silences);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read_to_string(folder.join("assets.yml")).unwrap(),
            manifest
        );
        let oscillator = folder.join("oscillator.wav");
        let mut oscillator_bytes = bytes.clone();
        for frame in 8000usize..16000 {
            let sample =
                (std::f32::consts::TAU * 1000.0 * (frame - 8000) as f32 / 8000.0).sin() * 0.25;
            let offset = 44 + frame * 8 + 4;
            oscillator_bytes[offset..offset + 4].copy_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&oscillator, &oscillator_bytes).unwrap();
        let video = folder.join("stereo video.mp4");
        crate::media::checked_output(
            crate::media::command(&tools.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=red:s=32x16:r=24:d=2",
                    "-i",
                ])
                .arg(&oscillator)
                .args([
                    "-map", "0:v:0", "-map", "1:a:0", "-c:v", "libx264", "-pix_fmt", "yuv420p",
                    "-c:a", "aac", "-b:a", "64k",
                ])
                .arg(&video),
        )
        .unwrap();
        let video_probe = tools.probe(&video).unwrap();
        assert!(video_probe.has_video && video_probe.has_audio);
        assert_eq!(video_probe.channels, 2);
        let video_output = folder.join("video-waveform.png");
        let (_, video_silences) = command(
            &[
                video.to_string_lossy().into_owned(),
                "--end=2".into(),
                "--output".into(),
                video_output.to_string_lossy().into_owned(),
            ],
            &tools,
            None,
        )
        .unwrap();
        assert_eq!(video_silences.len(), 1, "{video_silences:?}");
        assert_eq!(video_silences[0].start, 0.0);
        assert!((video_silences[0].end - 1.0).abs() < 0.05);
        let video_window = WaveformLayout::new(2.0, 24.0, Some(1.2), Some(1.8), None).unwrap();
        let video_peaks = decode_peaks(&tools, &video, &video_probe, &video_window, None).unwrap();
        assert!(video_peaks.iter().any(|peak| *peak > 12));
        assert!(
            silent_spans(&video_peaks, video_window.peaks_per_second)
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(&oscillator).unwrap(), oscillator_bytes);
        let canonical = std::fs::canonicalize(folder).unwrap();
        assert!(canonical.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()));
        std::fs::remove_dir_all(canonical).unwrap();
    }
    #[test]
    fn native_waveform_grid_draws_energy_and_timed_silence_overlay() {
        let layout = WaveformLayout::new(70.0, 30.0, Some(10.0), Some(70.0), None).unwrap();
        assert_eq!(layout.ruler.canvas, [1680, 1372]);
        assert_eq!((layout.ruler.columns, layout.ruler.rows), (10, 7));
        assert_eq!(layout.peaks_per_second, 98.0);
        let mut peaks = vec![255; 100];
        peaks.extend(vec![0; 50]);
        let (canvas, silences) = render_peaks(&peaks, &layout, None).unwrap();
        assert_eq!(
            silences,
            vec![Silence {
                start: 10.0 + 100.0 / 98.0,
                end: 10.0 + 150.0 / 98.0
            }]
        );
        assert_eq!(canvas.pixel(10, 50).unwrap().red(), 255);
        let silent = canvas.pixel(210, 50).unwrap();
        assert_eq!(silent.red(), 102);
        assert!((25..=26).contains(&silent.green()));
        assert!(render_peaks(&peaks, &layout, Some(&AtomicBool::new(true))).is_err());
        assert!(WaveformLayout::new(1e300, 30.0, None, None, None).is_err());
    }
    #[test]
    fn streaming_peaks_preserve_multichannel_energy_packet_boundaries_and_partial_tail() {
        let pcm = [0.25, 0.0, -0.25, 0.0, 0.0, 1.0, 0.0, 0.0, 4.0, 0.0];
        let mut whole = PeakSampler::new(4, 2, 2.0).unwrap();
        whole.push(&pcm, None).unwrap();
        let expected = whole.finish();
        assert_eq!(expected, [127, 255, 255]);
        let mut packets = PeakSampler::new(4, 2, 2.0).unwrap();
        for packet in pcm.chunks(2) {
            packets.push(packet, None).unwrap();
        }
        assert_eq!(packets.finish(), expected);
        let mut gap = PeakSampler::new(4, 1, 8.0).unwrap();
        gap.push(&[0.25, 1.0], None).unwrap();
        assert_eq!(gap.finish(), [127, 0, 255]);
        assert!(PeakSampler::new(4, 1, 2.0).unwrap().finish().is_empty());
    }

    #[test]
    fn silence_threshold_duration_and_invalid_pcm_match_the_original_contract() {
        assert_eq!(
            silent_spans(&[0, 12, 13, 0, 0, 0, 0, 13], 10.0).unwrap(),
            vec![3..7]
        );
        assert_eq!(
            silent_spans(&[13, 12, 12, 12, 12], 10.0).unwrap(),
            vec![1..5]
        );
        assert!(silent_spans(&[0, 0], f64::NAN).is_err());
        let mut sampler = PeakSampler::new(4, 2, 2.0).unwrap();
        assert!(sampler.push(&[0.0], None).is_err());
        assert!(sampler.push(&[0.0, f32::NAN], None).is_err());
        assert!(
            sampler
                .push(&[1.0, 1.0], Some(&AtomicBool::new(true)))
                .is_err()
        );
        assert!(sampler.finish().is_empty());
    }
}

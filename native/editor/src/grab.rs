// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Local video frame selection and contact-sheet rendering for media grab.
use crate::{Result, contact_sheet, media::Tools};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use tiny_skia::Pixmap;

pub const HELP: &str = "dapi-rust media grab <video-or-library-ref> [options]\n  -t, --time <time...>  Requested times; negatives count from the end\n  -c, --count <n>       Evenly sample n frames\n  -s, --start <time>    Count window start\n  -e, --end <time>      Count window end\n  -q, --quality <name> small, medium, large or fullres\n  -S, --separate        Write individual frames (default quality: small)\n      --per-sheet <n>   1..12 frames per contact sheet\n      --auto            Select settled visual states (default max: 30)\n      --uncapped        Lift the 100-frame request cap\n  -o, --output <dir>    PNG output directory\n      --project <dir>   Resolve assets.yml paths and IDs\n  -h, --help            Show help\nOutputs JSON Lines with timecode and absolute path. Existing PNGs are replaced atomically.\n";

#[derive(Debug)]
struct Request {
    source: String,
    project: Option<std::path::PathBuf>,
    output: Option<std::path::PathBuf>,
    selection: Selection,
    quality: Quality,
    per_sheet: Option<usize>,
    separate: bool,
    auto: bool,
}

fn parse_request(args: &[String]) -> Result<Request> {
    let Some(source) = args.first().filter(|arg| !arg.starts_with('-')) else {
        return Err(HELP.into());
    };
    let mut request = Request {
        source: source.clone(),
        project: None,
        output: None,
        selection: Selection::default(),
        quality: Quality::Fullres,
        per_sheet: None,
        separate: false,
        auto: false,
    };
    let mut quality_explicit = false;
    let mut index = 1;
    while index < args.len() {
        let argument = &args[index];
        let (flag, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(f, v)| (f, Some(v)));
        index += 1;
        if flag == "--auto" && inline.is_none() {
            request.auto = true;
            continue;
        }
        if matches!(flag, "-S" | "--separate") && inline.is_none() {
            request.separate = true;
            continue;
        }
        if flag == "--uncapped" && inline.is_none() {
            request.selection.uncapped = true;
            continue;
        }
        let value = if let Some(value) = inline {
            value
        } else {
            let value = args
                .get(index)
                .ok_or_else(|| format!("Missing value for {flag}"))?;
            index += 1;
            value.as_str()
        };
        match flag {
            "-t" | "--time" => {
                let times = request.selection.times.get_or_insert_with(Vec::new);
                times.push(crate::timing::parse_time(value)?);
                while index < args.len() && crate::timing::parse_time(&args[index]).is_ok() {
                    times.push(crate::timing::parse_time(&args[index])?);
                    index += 1;
                }
            }
            "-c" | "--count" => {
                request.selection.count = Some(
                    value
                        .parse()
                        .map_err(|_| "Count must be a positive integer")?,
                )
            }
            "-s" | "--start" => request.selection.start = Some(crate::timing::parse_time(value)?),
            "-e" | "--end" => request.selection.end = Some(crate::timing::parse_time(value)?),
            "-q" | "--quality" => {
                quality_explicit = true;
                request.quality = match value {
                    "small" => Quality::Small,
                    "medium" => Quality::Medium,
                    "large" => Quality::Large,
                    "fullres" => Quality::Fullres,
                    _ => return Err("Quality must be small, medium, large or fullres".into()),
                }
            }
            "--per-sheet" => {
                request.per_sheet = Some(
                    value
                        .parse()
                        .map_err(|_| "Frames per sheet must be an integer")?,
                )
            }
            "-o" | "--output" => request.output = Some(value.into()),
            "--project" => request.project = Some(value.into()),
            _ => return Err(format!("Unknown grab option: {flag}")),
        }
    }
    // Validate independent options before filesystem/codec work.
    if request.separate && request.per_sheet.is_some() {
        return Err("Per-sheet cannot be combined with separate frames".into());
    }
    if request.separate && !quality_explicit {
        request.quality = Quality::Small;
    }
    if request.auto {
        if request.selection.times.is_some() {
            return Err("Auto cannot be combined with explicit times".into());
        }
        let mut validation = request.selection.clone();
        validation.count = Some(validation.count.unwrap_or(30));
        validation.resolve(f64::MAX)?;
    } else {
        request.selection.resolve(f64::MAX)?;
    }
    contact_sheet::plan_sheet_sizes(1, request.per_sheet)?;
    Ok(request)
}

pub fn command(
    args: &[String],
    tools: &Tools,
    cancel: Option<&AtomicBool>,
    mut emit: impl FnMut(serde_json::Value) -> Result<()>,
) -> Result<()> {
    let mut request = parse_request(args)?;
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Grab cancelled".into());
    }
    let source = if let Some(project) = &request.project {
        if !project.is_dir() {
            return Err("Grab project must be a directory".into());
        }
        crate::media::resolve_source(&request.source, project)?
    } else {
        std::path::absolute(&request.source).map_err(|error| error.to_string())?
    };
    let output = std::path::absolute(request.output.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "dapi-grab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }))
    .map_err(|error| error.to_string())?;
    if request.auto {
        let times = pick_informative_times(
            tools,
            &source,
            request.selection.start,
            request.selection.end,
            request.selection.count.unwrap_or(30),
            cancel,
        )?;
        request.selection = Selection {
            times: Some(times),
            uncapped: request.selection.uncapped,
            ..Default::default()
        };
    }
    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let mut written = std::collections::BTreeSet::new();
    let mut publish = |image: GrabImage| {
        let path = output.join(format!("{}.png", image.timecode));
        if !request.separate || written.insert(path.clone()) {
            if std::fs::canonicalize(&path).ok() == std::fs::canonicalize(&source).ok()
                && path.exists()
            {
                return Err("Grab output would replace the source video".into());
            }
            write_png(&image.pixels, &path, cancel)?;
        }
        emit(serde_json::json!({"timecode": image.timecode, "path": path}))
    };
    if request.separate {
        return render_separate(
            tools,
            &source,
            &request.selection,
            request.quality,
            cancel,
            publish,
        );
    }
    render_sheets(
        tools,
        &source,
        &request.selection,
        request.quality,
        request.per_sheet,
        cancel,
        &mut publish,
    )
}

/// Publish a complete PNG, atomically replacing an existing regular output.
fn write_png(canvas: &Pixmap, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    use std::io::Read;
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err("Grab cancelled".into());
    }
    let metadata = match std::fs::symlink_metadata(output) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return crate::filmstrip::write_png(canvas, output, cancel);
        }
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Grab overwrite target must be a regular file".into());
    }
    const MAX_EXISTING: u64 = 64 * 1024 * 1024;
    let mut expected = Vec::new();
    std::fs::File::open(output)
        .map_err(|error| error.to_string())?
        .take(MAX_EXISTING + 1)
        .read_to_end(&mut expected)
        .map_err(|error| error.to_string())?;
    if expected.len() as u64 > MAX_EXISTING {
        return Err("Grab overwrite target exceeds 64 MiB".into());
    }
    let directory = output
        .parent()
        .ok_or("Grab output needs a parent directory")?
        .join(format!(
            ".ad3-grab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
    let candidate = directory.join("replacement.png");
    crate::filmstrip::write_png(canvas, &candidate, cancel)?;
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(format!(
            "Grab cancelled; PNG candidate retained at {}",
            candidate.display()
        ));
    }
    crate::atomic_file::replace(&candidate, output, &expected)
        .map_err(|error| format!("{error}; PNG recovery directory: {}", directory.display()))?;
    std::fs::remove_dir(&directory).map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    #[default]
    Fullres,
    Small,
    Medium,
    Large,
}

impl Quality {
    pub fn dimensions(self, source: [u32; 2]) -> Result<[u32; 2]> {
        if source.contains(&0) {
            return Err("Grab needs positive video dimensions".into());
        }
        let budget = match self {
            Self::Fullres => 0,
            Self::Small => 384u64.pow(2),
            Self::Medium => 768u64.pow(2),
            Self::Large => 1536u64.pow(2),
        };
        let pixels = u64::from(source[0]) * u64::from(source[1]);
        if budget == 0 || pixels <= budget {
            return Ok(source);
        }
        let scale = (budget as f64 / pixels as f64).sqrt();
        Ok(source.map(|dimension| (dimension as f64 * scale).round().max(1.0) as u32))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub times: Option<Vec<f64>>,
    pub count: Option<usize>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub uncapped: bool,
}

impl Selection {
    pub fn resolve(&self, duration: f64) -> Result<Vec<f64>> {
        if !duration.is_finite() || duration <= 0.0 {
            return Err("Grab needs finite positive video duration".into());
        }
        if self.times.is_some() && self.count.is_some() {
            return Err("Pass either times or count, not both".into());
        }
        if self.count.is_none() && (self.start.is_some() || self.end.is_some()) {
            return Err("Grab start/end require a frame count".into());
        }
        if self
            .start
            .into_iter()
            .chain(self.end)
            .any(|v| !v.is_finite() || v < 0.0)
            || matches!((self.start, self.end), (Some(s), Some(e)) if s >= e)
        {
            return Err("Invalid grab sampling window".into());
        }
        let requested = self
            .count
            .unwrap_or_else(|| self.times.as_ref().map_or(1, Vec::len));
        if requested == 0 || requested > 1_000_000 || (!self.uncapped && requested > 100) {
            return Err(
                "Grab frame count exceeds its limit (100 unless uncapped; maximum 1000000)".into(),
            );
        }
        if let Some(count) = self.count {
            let start = self.start.unwrap_or(0.0).min(duration);
            let end = self.end.unwrap_or(duration).max(start).min(duration);
            if end <= start {
                return Err("Grab sampling window is empty".into());
            }
            let step = (end - start) / count as f64;
            Ok((0..count).map(|i| start + i as f64 * step).collect())
        } else {
            self.times
                .as_deref()
                .unwrap_or(&[0.0])
                .iter()
                .map(|time| {
                    let resolved = if *time < 0.0 { duration + time } else { *time };
                    if !resolved.is_finite() || resolved < 0.0 || resolved > duration {
                        Err("Grab timestamp falls outside the video".into())
                    } else {
                        Ok(resolved)
                    }
                })
                .collect()
        }
    }
}

pub struct GrabImage {
    pub timecode: String,
    pub pixels: Pixmap,
}

/// Scan source pixels at two samples per second, retaining settled visual states.
pub fn pick_informative_times(
    tools: &Tools,
    source: &Path,
    start: Option<f64>,
    end: Option<f64>,
    max: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f64>> {
    let probe = tools.probe_cancellable(source, cancel)?;
    if !probe.has_video || probe.still {
        return Err("Auto selection needs a video asset".into());
    }
    let duration = probe
        .duration
        .ok_or("Auto selection needs duration metadata")?;
    let validation = Selection {
        count: Some(max),
        start,
        end,
        uncapped: true,
        ..Default::default()
    };
    let from = validation.resolve(duration)?[0];
    let to = end.unwrap_or(duration).min(duration);
    let samples = ((to - from) * 2.0).ceil();
    if samples > 1_000_000.0 {
        return Err("Auto scan exceeds its sample budget".into());
    }
    let (width, height) = probe.display_dimensions()?;
    let clock = crate::video_clock::probe_clock(tools, source, cancel)?;
    let origin = clock.timestamps[0];
    let mut previous_frame = None;
    let mut triage = crate::frame_triage::Triage::default();
    let mut indices = Vec::new();
    for index in 0..samples as usize {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Auto scan cancelled".into());
        }
        let time = from + index as f64 / 2.0;
        if time >= to {
            break;
        }
        let requested = time + origin;
        if !requested.is_finite() {
            return Err("Auto source clock overflows".into());
        }
        let Some((frame_index, source_time)) = clock.at(requested) else {
            continue;
        };
        if previous_frame == Some(source_time) {
            continue;
        }
        previous_frame = Some(source_time);
        indices.push(frame_index);
    }
    crate::video_decode::selected_frames_with_clock(
        tools,
        source,
        &probe,
        &indices,
        [width, height],
        false,
        &clock,
        cancel,
        |index, frame| {
            triage.push(
                (clock.timestamps[index] - origin).max(0.0),
                crate::frame_triage::fingerprint(&frame, cancel)?,
            )
        },
    )?;
    triage.finish(max)
}

/// Individual frames retain their pixels and alpha, without gutters or labels.
pub fn render_separate(
    tools: &Tools,
    source: &Path,
    selection: &Selection,
    quality: Quality,
    cancel: Option<&AtomicBool>,
    mut emit: impl FnMut(GrabImage) -> Result<()>,
) -> Result<()> {
    let probe = tools.probe_cancellable(source, cancel)?;
    if !probe.has_video || probe.still {
        return Err("Grab needs a video asset".into());
    }
    let times = selection.resolve(probe.duration.ok_or("Grab needs duration metadata")?)?;
    let (width, height) = probe.display_dimensions()?;
    let dimensions = quality.dimensions([width, height])?;
    let clock = crate::video_clock::probe_clock(tools, source, cancel)?;
    let origin = clock.timestamps[0];
    let frame_pixels = u64::from(dimensions[0]) * u64::from(dimensions[1]);
    if frame_pixels == 0 || frame_pixels > 16_000_000 {
        return Err("Separate frames exceed supported raster dimensions".into());
    }
    let frame_bytes = frame_pixels * 4;
    let batch_size = (64 * 1024 * 1024 / frame_bytes).clamp(1, 12) as usize;
    for batch in times.chunks(batch_size) {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Grab cancelled".into());
        }
        let slots = batch
            .iter()
            .map(|time| {
                let source_time = time + origin;
                if !source_time.is_finite() {
                    return Err("Grab source clock overflows".to_string());
                }
                clock
                    .at(source_time)
                    .map(|(index, _)| index)
                    .ok_or_else(|| "No video frame at requested time".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        let mut indices = slots.clone();
        indices.sort_unstable();
        indices.dedup();
        let mut frames = std::collections::BTreeMap::new();
        crate::video_decode::selected_frames_with_clock(
            tools,
            source,
            &probe,
            &indices,
            dimensions,
            false,
            &clock,
            cancel,
            |index, pixels| {
                frames.insert(index, pixels);
                Ok(())
            },
        )?;
        for (slot, (time, index)) in batch.iter().zip(&slots).enumerate() {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Err("Grab cancelled".into());
            }
            let pixels = if slots[slot + 1..].contains(index) {
                frames[index].clone()
            } else {
                frames.remove(index).ok_or("Missing decoded frame")?
            };
            emit(GrabImage {
                timecode: contact_sheet::format_timecode(*time, probe.frame_rate)?,
                pixels,
            })?;
        }
    }
    Ok(())
}

/// Decode one bounded sheet at a time and deliver it to the caller immediately.
/// Requested order is preserved; no complete clip or all-sheet pixel cache.
pub fn render_sheets(
    tools: &Tools,
    source: &Path,
    selection: &Selection,
    quality: Quality,
    per_sheet: Option<usize>,
    cancel: Option<&AtomicBool>,
    mut emit: impl FnMut(GrabImage) -> Result<()>,
) -> Result<()> {
    let cancelled = || {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            Err("Grab cancelled".to_string())
        } else {
            Ok(())
        }
    };
    cancelled()?;
    let probe = tools.probe_cancellable(source, cancel)?;
    if !probe.has_video || probe.still {
        return Err("Grab needs a video asset".into());
    }
    let times = selection.resolve(probe.duration.ok_or("Grab needs duration metadata")?)?;
    let (width, height) = probe.display_dimensions()?;
    let dimensions = quality.dimensions([width, height])?;
    let sizes = contact_sheet::plan_sheet_sizes(times.len(), per_sheet)?;
    let clock = crate::video_clock::probe_clock(tools, source, cancel)?;
    let origin = clock.timestamps[0];
    let mut offset = 0;
    for count in sizes {
        cancelled()?;
        let plan = contact_sheet::plan_sheet(count, dimensions)?;
        let window = &times[offset..offset + count];
        let mut slots = Vec::with_capacity(count);
        let mut labels = Vec::with_capacity(count);
        for time in window {
            cancelled()?;
            let source_time = time + origin;
            if !source_time.is_finite() {
                return Err("Grab source clock overflows".into());
            }
            slots.push(
                clock
                    .at(source_time)
                    .ok_or("No video frame at requested time")?
                    .0,
            );
            labels.push(contact_sheet::format_timecode(*time, probe.frame_rate)?);
        }
        let mut indices = slots.clone();
        indices.sort_unstable();
        indices.dedup();
        let mut frames = std::collections::BTreeMap::new();
        crate::video_decode::selected_frames_with_clock(
            tools,
            source,
            &probe,
            &indices,
            [plan.cell_width, plan.cell_height],
            false,
            &clock,
            cancel,
            |index, pixels| {
                frames.insert(index, pixels);
                Ok(())
            },
        )?;
        let references = slots
            .iter()
            .zip(&labels)
            .map(|(index, label)| (&frames[index], label.as_str()))
            .collect::<Vec<_>>();
        let pixels = contact_sheet::compose_sheet(&references, plan, cancel)?;
        cancelled()?;
        emit(GrabImage {
            timecode: contact_sheet::sheet_timecode(window, probe.frame_rate)?,
            pixels,
        })?;
        offset += count;
    }
    cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires local FFmpeg and FFprobe"]
    fn real_video_sheets_preserve_requested_colors_and_stop_after_emitter_failure() {
        let folder = std::env::temp_dir().join(format!(
            "ad3-grab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let source = folder.join("red blue.mkv");
        let tools = Tools::default();
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
                    "color=red:s=320x180:r=2:d=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:s=320x180:r=2:d=1",
                    "-filter_complex",
                    "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                    "-map",
                    "[v]",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&source),
        )
        .unwrap();
        let original = std::fs::read(&source).unwrap();
        let clock = crate::video_clock::probe_clock(&tools, &source, None).unwrap();
        assert_eq!(clock.timestamps, [0.0, 0.5, 1.0, 1.5]);
        let vfr = folder.join("irregular.mkv");
        crate::media::checked_output(
            crate::media::command(&tools.ffmpeg)
                .args(["-v", "error", "-nostdin", "-n", "-i"])
                .arg(&source)
                .args([
                    "-vf",
                    "setpts=N*N/(2*TB)",
                    "-fps_mode",
                    "vfr",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&vfr),
        )
        .unwrap();
        let vfr_clock = crate::video_clock::probe_clock(&tools, &vfr, None).unwrap();
        assert_eq!(vfr_clock.timestamps, [0.0, 0.5, 2.0, 4.5]);
        assert_eq!(vfr_clock.at(1.5), Some((1, 0.5)));
        let h264 = folder.join("reordered.mp4");
        crate::media::checked_output(
            crate::media::command(&tools.ffmpeg)
                .args(["-v", "error", "-nostdin", "-n", "-i"])
                .arg(&vfr)
                .args([
                    "-fps_mode",
                    "passthrough",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-bf",
                    "2",
                    "-g",
                    "12",
                    "-x264-params",
                    "b-adapt=0:scenecut=0",
                ])
                .arg(&h264),
        )
        .unwrap();
        let picture_types = crate::media::checked_output(
            crate::media::command(&tools.ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=pict_type",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&h264),
        )
        .unwrap();
        assert!(
            String::from_utf8_lossy(&picture_types.stdout)
                .lines()
                .any(|line| line.starts_with('B'))
        );
        let shifted = folder.join("shifted.mkv");
        crate::media::checked_output(
            crate::media::command(&tools.ffmpeg)
                .args(["-v", "error", "-nostdin", "-n", "-itsoffset", "5", "-i"])
                .arg(&h264)
                .args(["-c", "copy"])
                .arg(&shifted),
        )
        .unwrap();
        let shifted_clock = crate::video_clock::probe_clock(&tools, &shifted, None).unwrap();
        assert_eq!(shifted_clock.timestamps, [5.0, 5.5, 7.0, 9.5]);
        let shifted_probe = tools.probe(&shifted).unwrap();
        let mut reordered_colors = Vec::new();
        crate::video_decode::selected_frames(
            &tools,
            &shifted,
            &shifted_probe,
            &[0, 1, 2, 3],
            [32, 18],
            None,
            |_, pixels| {
                reordered_colors.push(pixels.pixel(16, 9).unwrap());
                Ok(())
            },
        )
        .unwrap();
        assert!(
            reordered_colors[..2]
                .iter()
                .all(|pixel| pixel.red() > 240 && pixel.blue() < 5)
        );
        assert!(
            reordered_colors[2..]
                .iter()
                .all(|pixel| pixel.blue() > 240 && pixel.red() < 5)
        );
        assert_eq!(
            pick_informative_times(&tools, &shifted, None, None, 30, None).unwrap(),
            [0.0, 4.5]
        );
        let vfr_probe = tools.probe(&vfr).unwrap();
        let session_clock =
            std::sync::Arc::new(crate::video_clock::probe_clock(&tools, &shifted, None).unwrap());
        let mut session = crate::video_session::VideoSession::start(
            tools.clone(),
            shifted.clone(),
            shifted_probe.clone(),
            session_clock.clone(),
            1,
        )
        .unwrap();
        let (_, session_red) = session.next(None).unwrap().unwrap();
        assert_eq!(
            session_red.pixel(160, 90).unwrap().red(),
            reordered_colors[1].red()
        );
        assert!(session.matches(shifted_probe.version.as_ref().unwrap(), 2));
        assert!(!session.matches_at(
            shifted_probe.version.as_ref().unwrap(),
            2,
            std::time::Instant::now() + std::time::Duration::from_secs(91)
        ));
        let (_, session_blue) = session.next(None).unwrap().unwrap();
        assert!(session_blue.pixel(160, 90).unwrap().blue() > 240);
        let before_drop = std::time::Instant::now();
        drop(session);
        assert!(before_drop.elapsed() < std::time::Duration::from_secs(5));
        let mut queued = crate::video_session::VideoSession::start(
            tools.clone(),
            shifted.clone(),
            shifted_probe.clone(),
            session_clock.clone(),
            0,
        )
        .unwrap();
        queued.next(None).unwrap().unwrap();
        let before_drop = std::time::Instant::now();
        drop(queued);
        assert!(before_drop.elapsed() < std::time::Duration::from_secs(5));
        let mut tail = crate::video_session::VideoSession::start(
            tools.clone(),
            shifted.clone(),
            shifted_probe.clone(),
            session_clock.clone(),
            3,
        )
        .unwrap();
        assert_eq!(tail.next(None).unwrap().unwrap().0, 3);
        assert!(tail.next(None).unwrap().is_none());
        let mut skipped = crate::video_session::VideoSession::start(
            tools.clone(),
            shifted.clone(),
            shifted_probe.clone(),
            session_clock.clone(),
            0,
        )
        .unwrap();
        let (at, pixels) = skipped.frame_at(2, None).unwrap().unwrap();
        assert_eq!(at, 2);
        assert!(pixels.pixel(160, 90).unwrap().blue() > 240);
        assert!(skipped.frame_at(1, None).is_err());
        assert!(skipped.frame_at(40, None).is_err());
        assert_eq!(skipped.frame_at(3, None).unwrap().unwrap().0, 3);
        let mut cancelled_session = crate::video_session::VideoSession::start(
            tools.clone(),
            shifted.clone(),
            shifted_probe.clone(),
            session_clock,
            0,
        )
        .unwrap();
        assert!(
            cancelled_session
                .next(Some(&AtomicBool::new(true)))
                .is_err()
        );
        let textures = crate::texture::TextureStore::new(tools.clone());
        let mut held_request = crate::texture::TextureRequest {
            path: vfr.clone(),
            probe: vfr_probe.clone(),
            frame: crate::texture::TextureFrame::Video(1.25),
        };
        let held_first = textures.frame(&held_request, None).unwrap();
        assert_eq!(textures.cached_bytes(), 3 * 320 * 180 * 4);
        assert!(held_first.pixel(160, 100).unwrap().red() > 240);
        held_request.frame = crate::texture::TextureFrame::Video(1.75);
        let held_again = textures.frame(&held_request, None).unwrap();
        assert!(std::sync::Arc::ptr_eq(&held_first, &held_again));
        held_request.frame = crate::texture::TextureFrame::Video(2.25);
        assert!(
            textures
                .frame(&held_request, None)
                .unwrap()
                .pixel(160, 100)
                .unwrap()
                .blue()
                > 240
        );
        let mut sought = Vec::new();
        crate::video_decode::selected_frames(
            &tools,
            &shifted,
            &shifted_probe,
            &[1, 3],
            [32, 18],
            None,
            |index, pixels| {
                sought.push((index, pixels.pixel(16, 9).unwrap()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(sought[0], (1, reordered_colors[1]));
        assert_eq!(sought[1], (3, reordered_colors[3]));
        let no_probe = Tools {
            ffmpeg: tools.ffmpeg.clone(),
            ffprobe: folder.join("missing-ffprobe.exe"),
        };
        let mut reused = Vec::new();
        crate::video_decode::selected_frames_with_clock(
            &no_probe,
            &shifted,
            &shifted_probe,
            &[1, 3],
            [32, 18],
            false,
            &shifted_clock,
            None,
            |index, pixels| {
                reused.push((index, pixels.pixel(16, 9).unwrap()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(reused, sought);
        assert!(
            crate::video_decode::selected_frames_with_clock(
                &no_probe,
                &shifted,
                &shifted_probe,
                &[1],
                [32, 18],
                false,
                &vfr_clock,
                None,
                |_, _| Ok(())
            )
            .is_err()
        );
        let held_selection = Selection {
            times: Some(vec![2.25, 1.25, 1.25]),
            ..Default::default()
        };
        let mut held = Vec::new();
        render_sheets(
            &tools,
            &vfr,
            &held_selection,
            Quality::Small,
            None,
            None,
            |image| {
                held.push(image);
                Ok(())
            },
        )
        .unwrap();
        let held_plan = contact_sheet::plan_sheet(3, [320, 180]).unwrap();
        let mut separate_held = Vec::new();
        render_separate(
            &tools,
            &vfr,
            &held_selection,
            Quality::Small,
            None,
            |image| {
                separate_held.push(image);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(separate_held.len(), 3);
        let batched = Selection {
            times: Some(
                (0..13)
                    .map(|i| if i % 2 == 0 { 2.25 } else { 1.25 })
                    .collect(),
            ),
            ..Default::default()
        };
        let mut batch_count = 0;
        render_separate(&tools, &vfr, &batched, Quality::Small, None, |image| {
            let pixel = image.pixels.pixel(160, 100).unwrap();
            assert!(if batch_count % 2 == 0 {
                pixel.blue() > 240
            } else {
                pixel.red() > 240
            });
            batch_count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(batch_count, 13);
        assert_eq!(
            render_separate(&tools, &vfr, &batched, Quality::Small, None, |_| Err(
                "stop separate output".into()
            ))
            .unwrap_err(),
            "stop separate output"
        );
        for (index, blue) in [(0, true), (1, false), (2, false)] {
            let [x, y] = held_plan.cell_position(index).unwrap();
            let pixel = held[0].pixels.pixel(x + 160, y + 100).unwrap();
            assert!(if blue {
                pixel.blue() > 240
            } else {
                pixel.red() > 240
            });
            let separate_pixel = separate_held[index].pixels.pixel(160, 100).unwrap();
            assert!(if blue {
                separate_pixel.blue() > 240
            } else {
                separate_pixel.red() > 240
            });
        }
        let mut decoded = Vec::new();
        crate::video_decode::selected_frames(
            &tools,
            &vfr,
            &vfr_probe,
            &[0, 2, 3],
            [32, 18],
            None,
            |index, pixels| {
                decoded.push((index, pixels.pixel(16, 9).unwrap()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            decoded.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [0, 2, 3]
        );
        assert!(decoded[0].1.red() > 240);
        assert!(decoded[1].1.blue() > 240 && decoded[2].1.blue() > 240);
        assert_eq!(
            crate::video_decode::selected_frames(
                &tools,
                &vfr,
                &vfr_probe,
                &[0, 2],
                [32, 18],
                None,
                |_, _| Err("consumer stopped".into())
            )
            .unwrap_err(),
            "consumer stopped"
        );
        assert!(
            crate::video_decode::selected_frames(
                &tools,
                &vfr,
                &vfr_probe,
                &[2, 0],
                [32, 18],
                None,
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(
            pick_informative_times(&tools, &vfr, None, None, 30, None).unwrap(),
            [0.0, 4.5]
        );
        assert_eq!(
            pick_informative_times(&tools, &vfr, Some(2.25), Some(4.0), 30, None).unwrap(),
            [2.0]
        );
        let selection = Selection {
            times: Some(vec![-0.5, 0.0, 0.5]),
            ..Default::default()
        };
        let mut emitted = Vec::new();
        render_sheets(
            &tools,
            &source,
            &selection,
            Quality::Small,
            None,
            None,
            |image| {
                emitted.push(image);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].timecode, "0f-01s01f");
        let plan = contact_sheet::plan_sheet(3, [320, 180]).unwrap();
        for (index, blue) in [(0, true), (1, false), (2, false)] {
            let [x, y] = plan.cell_position(index).unwrap();
            let pixel = emitted[0].pixels.pixel(x + 160, y + 100).unwrap();
            assert!(if blue {
                pixel.blue() > 240 && pixel.red() < 5
            } else {
                pixel.red() > 240 && pixel.blue() < 5
            });
        }
        let mut calls = 0;
        let error = render_sheets(
            &tools,
            &source,
            &selection,
            Quality::Fullres,
            Some(1),
            None,
            |_| {
                calls += 1;
                Err("stop writing".into())
            },
        )
        .unwrap_err();
        assert_eq!(error, "stop writing");
        assert_eq!(calls, 1);
        assert!(
            render_sheets(
                &tools,
                &source,
                &selection,
                Quality::Fullres,
                None,
                Some(&AtomicBool::new(true)),
                |_| panic!("cancelled renderer emitted a sheet")
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let args = vec![
            source.to_string_lossy().into_owned(),
            "--time=1.5".into(),
            "0".into(),
            "-0.5".into(),
            "--quality=small".into(),
            "--per-sheet=2".into(),
            "--output".into(),
            folder.join("sheets").to_string_lossy().into_owned(),
        ];
        let mut records = Vec::new();
        command(&args, &tools, None, |record| {
            records.push(record);
            Ok(())
        })
        .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["timecode"], "0f-01s01f");
        assert_eq!(records[1]["timecode"], "01s01f");
        for record in &records {
            let path = Path::new(record["path"].as_str().unwrap());
            assert!(path.is_absolute());
            assert!(Pixmap::load_png(path).is_ok());
        }
        let first_path = Path::new(records[0]["path"].as_str().unwrap());
        std::fs::write(first_path, b"old preview").unwrap();
        command(&args, &tools, None, |_| Ok(())).unwrap();
        assert!(Pixmap::load_png(first_path).is_ok());
        let saved = std::fs::read(first_path).unwrap();
        assert!(write_png(&emitted[0].pixels, first_path, Some(&AtomicBool::new(true))).is_err());
        assert_eq!(std::fs::read(first_path).unwrap(), saved);
        assert!(write_png(&emitted[0].pixels, &folder, None).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let separate_args = vec![
            source.to_string_lossy().into_owned(),
            "--time".into(),
            "1.5".into(),
            "0".into(),
            "1.5".into(),
            "--separate".into(),
            "--output".into(),
            folder.join("frames").to_string_lossy().into_owned(),
        ];
        let mut separate_records = Vec::new();
        command(&separate_args, &tools, None, |record| {
            separate_records.push(record);
            Ok(())
        })
        .unwrap();
        assert_eq!(separate_records.len(), 3);
        assert_eq!(separate_records[0], separate_records[2]);
        for (index, record) in separate_records.iter().enumerate() {
            let pixels = Pixmap::load_png(Path::new(record["path"].as_str().unwrap())).unwrap();
            assert_eq!([pixels.width(), pixels.height()], [320, 180]);
            let pixel = pixels.pixel(0, 0).unwrap();
            assert!(if index == 1 {
                pixel.red() > 240
            } else {
                pixel.blue() > 240
            });
        }
        assert_eq!(
            pick_informative_times(&tools, &source, None, None, 30, None).unwrap(),
            [0.0, 1.5]
        );
        assert_eq!(
            pick_informative_times(&tools, &source, Some(1.0), None, 30, None).unwrap(),
            [1.0]
        );
        let auto_args = vec![
            source.to_string_lossy().into_owned(),
            "--auto".into(),
            "--count=1".into(),
            "-S".into(),
            "--output".into(),
            folder.join("auto").to_string_lossy().into_owned(),
        ];
        let mut auto_records = Vec::new();
        command(&auto_args, &tools, None, |record| {
            auto_records.push(record);
            Ok(())
        })
        .unwrap();
        assert_eq!(auto_records.len(), 1);
        assert_eq!(auto_records[0]["timecode"], "0f");
        let canonical = std::fs::canonicalize(folder).unwrap();
        assert!(canonical.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()));
        std::fs::remove_dir_all(canonical).unwrap();
    }
    #[test]
    fn cli_validates_options_before_io_and_accepts_negative_variadic_times() {
        let args = ["clip.mp4", "--time", "8s", "--count=2"].map(String::from);
        assert!(parse_request(&args).is_err());
        for flags in [
            vec!["--count=0"],
            vec!["--count=101"],
            vec!["--start=1"],
            vec!["--quality=invalid"],
            vec!["--per-sheet=13"],
            vec!["--separate", "--per-sheet=2"],
            vec!["--auto", "--time=0"],
        ] {
            let args = std::iter::once("clip.mp4")
                .chain(flags)
                .map(String::from)
                .collect::<Vec<_>>();
            assert!(parse_request(&args).is_err());
        }
        let args = ["clip.mp4", "--time=8", "-1", "-1f", "--quality", "medium"].map(String::from);
        let request = parse_request(&args).unwrap();
        assert_eq!(request.selection.times.unwrap(), [8.0, -1.0, -1.0 / 30.0]);
        assert_eq!(request.quality, Quality::Medium);
        let request = parse_request(&["clip.mp4".into(), "-S".into()]).unwrap();
        assert!(request.separate);
        assert_eq!(request.quality, Quality::Small);
        let request =
            parse_request(&["clip.mp4".into(), "-S".into(), "-q=fullres".into()]).unwrap();
        assert_eq!(request.quality, Quality::Fullres);
    }
    #[test]
    fn selection_resolves_negative_times_preserves_order_and_samples_window() {
        let explicit = Selection {
            times: Some(vec![8.0, -1.0, 0.0, -10.0]),
            ..Default::default()
        };
        assert_eq!(explicit.resolve(10.0).unwrap(), [8.0, 9.0, 0.0, 0.0]);
        assert_eq!(Selection::default().resolve(10.0).unwrap(), [0.0]);
        let counted = Selection {
            count: Some(4),
            start: Some(2.0),
            end: Some(6.0),
            ..Default::default()
        };
        assert_eq!(counted.resolve(10.0).unwrap(), [2.0, 3.0, 4.0, 5.0]);
        assert_eq!(
            Selection {
                end: Some(100.0),
                ..counted.clone()
            }
            .resolve(10.0)
            .unwrap(),
            [2.0, 4.0, 6.0, 8.0]
        );
        for bad in [
            Selection {
                count: Some(0),
                ..Default::default()
            },
            Selection {
                count: Some(101),
                ..Default::default()
            },
            Selection {
                times: Some(vec![f64::NAN]),
                ..Default::default()
            },
            Selection {
                times: Some(vec![-11.0]),
                ..Default::default()
            },
            Selection {
                times: Some(vec![11.0]),
                ..Default::default()
            },
            Selection {
                start: Some(1.0),
                ..Default::default()
            },
            Selection {
                count: Some(1),
                times: Some(vec![0.0]),
                ..Default::default()
            },
            Selection {
                start: Some(10.0),
                ..counted
            },
        ] {
            assert!(bad.resolve(10.0).is_err());
        }
        assert_eq!(
            Selection {
                count: Some(101),
                uncapped: true,
                ..Default::default()
            }
            .resolve(10.0)
            .unwrap()
            .len(),
            101
        );
    }
    #[test]
    fn quality_caps_pixels_without_enlarging_small_sources() {
        assert_eq!(
            Quality::Fullres.dimensions([3840, 2160]).unwrap(),
            [3840, 2160]
        );
        assert_eq!(Quality::Small.dimensions([64, 32]).unwrap(), [64, 32]);
        assert_eq!(Quality::Small.dimensions([3840, 2160]).unwrap(), [512, 288]);
        assert_eq!(
            Quality::Medium.dimensions([2160, 3840]).unwrap(),
            [576, 1024]
        );
        assert!(Quality::Large.dimensions([0, 1]).is_err());
    }
}

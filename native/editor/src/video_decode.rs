// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Sequential local video decoding with bounded memory and one codec process.
use crate::{
    Result,
    media::{Probe, SourceVersion, Tools},
};
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Deliver selected presentation-order frame indices from one FFmpeg session.
/// Intermediate frames are discarded; decoder storage is two frame buffers.
pub fn selected_frames(
    tools: &Tools,
    source: &Path,
    probe: &Probe,
    indices: &[usize],
    dimensions: [u32; 2],
    cancel: Option<&AtomicBool>,
    emit: impl FnMut(usize, tiny_skia::Pixmap) -> Result<()> + Send,
) -> Result<()> {
    decode_selected(
        tools, source, probe, indices, dimensions, false, None, cancel, emit,
    )
}

pub fn selected_frames_cover(
    tools: &Tools,
    source: &Path,
    probe: &Probe,
    indices: &[usize],
    dimensions: [u32; 2],
    cancel: Option<&AtomicBool>,
    emit: impl FnMut(usize, tiny_skia::Pixmap) -> Result<()> + Send,
) -> Result<()> {
    decode_selected(
        tools, source, probe, indices, dimensions, true, None, cancel, emit,
    )
}

/// Reuse a previously read clock after verifying its source version.
pub fn selected_frames_with_clock(
    tools: &Tools,
    source: &Path,
    probe: &Probe,
    indices: &[usize],
    dimensions: [u32; 2],
    cover: bool,
    clock: &crate::video_clock::VideoClock,
    cancel: Option<&AtomicBool>,
    emit: impl FnMut(usize, tiny_skia::Pixmap) -> Result<()> + Send,
) -> Result<()> {
    decode_selected(
        tools,
        source,
        probe,
        indices,
        dimensions,
        cover,
        Some(clock),
        cancel,
        emit,
    )
}

fn decode_selected(
    tools: &Tools,
    source: &Path,
    probe: &Probe,
    indices: &[usize],
    dimensions: [u32; 2],
    cover: bool,
    clock: Option<&crate::video_clock::VideoClock>,
    cancel: Option<&AtomicBool>,
    mut emit: impl FnMut(usize, tiny_skia::Pixmap) -> Result<()> + Send,
) -> Result<()> {
    if indices.is_empty()
        || indices.windows(2).any(|pair| pair[0] >= pair[1])
        || indices.last().is_some_and(|index| *index >= 1_000_000)
    {
        return Err("Video decode indices must be nonempty, increasing and below 1000000".into());
    }
    let pixels = u64::from(dimensions[0]) * u64::from(dimensions[1]);
    let (source_width, source_height) = probe.display_dimensions()?;
    if !probe.has_video
        || probe.still
        || pixels == 0
        || pixels > 16_000_000
        || u64::from(source_width) * u64::from(source_height) > 16_000_000
        || u64::from(probe.width) * u64::from(probe.height) > 16_000_000
    {
        return Err("Video stream exceeds supported raster dimensions".into());
    }
    let version = SourceVersion::read(source)?;
    if probe.version.as_ref() != Some(&version) {
        return Err("Video changed since probing".into());
    }
    if clock.is_some_and(|clock| clock.version.as_ref() != Some(&version)) {
        return Err("Video clock does not match the current source version".into());
    }
    let mut first = 0;
    let mut seek = None;
    if indices[0] > 0 {
        let loaded;
        let clock = if let Some(clock) = clock {
            clock
        } else {
            loaded = crate::video_clock::probe_clock(tools, source, cancel)?;
            &loaded
        };
        let requested = *clock
            .timestamps
            .get(indices[0])
            .ok_or("Requested video frame does not exist")?;
        first = clock.timestamps.partition_point(|time| *time < requested);
        if first > 0 {
            let previous = clock.timestamps[first - 1];
            // Seek inside the preceding presentation interval. This avoids
            // rounding a six-decimal FFprobe timestamp past the desired frame.
            let midpoint = previous + (requested - previous) / 2.0;
            if midpoint >= 0.0 && midpoint > previous && midpoint < requested {
                seek = Some(midpoint);
            } else {
                first = 0;
            }
        }
    }
    let mut process = crate::media::command(&tools.ffmpeg);
    let filter = if cover {
        let scale = (dimensions[0] as f64 / source_width as f64)
            .max(dimensions[1] as f64 / source_height as f64);
        let width = (source_width as f64 * scale).ceil() as u32;
        let height = (source_height as f64 * scale).ceil() as u32;
        if u64::from(width) * u64::from(height) > 16_000_000 {
            return Err("Video cover scaling exceeds supported raster dimensions".into());
        }
        format!(
            "scale={width}:{height}:flags=bilinear,setsar=1,format=rgba,crop={}:{}",
            dimensions[0], dimensions[1]
        )
    } else {
        format!(
            "scale={}:{}:flags=bilinear,setsar=1,format=rgba",
            dimensions[0], dimensions[1]
        )
    };
    process.args([
        "-v",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-protocol_whitelist",
        "file,pipe",
    ]);
    if let Some(time) = seek {
        process.args(["-ss", &format!("{time:.9}"), "-seek_timestamp", "1"]);
    }
    process
        .arg("-i")
        .arg(source)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf"])
        .arg(filter)
        .args(["-fps_mode", "passthrough", "-frames:v"])
        .arg((indices.last().unwrap() - first + 1).to_string())
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
    crate::media::checked_stream_cancellable(
        &mut process,
        cancel,
        Duration::from_secs(120),
        |mut stdout| {
            let mut bytes = vec![0u8; pixels as usize * 4];
            let mut wanted = 0;
            for index in first..=indices[indices.len() - 1] {
                for chunk in bytes.chunks_mut(1024 * 1024) {
                    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                        return Err("Video stream cancelled".into());
                    }
                    stdout
                        .read_exact(chunk)
                        .map_err(|error| format!("Incomplete video frame {index}: {error}"))?;
                }
                if index == indices[wanted] {
                    let frame = crate::texture::premultiply(
                        bytes.clone(),
                        dimensions[0],
                        dimensions[1],
                        cancel,
                    )?;
                    emit(index, frame)?;
                    wanted += 1;
                    if wanted == indices.len() {
                        break;
                    }
                }
            }
            Ok(())
        },
    )?;
    if SourceVersion::read(source)? != version {
        return Err("Video changed while decoding frames".into());
    }
    Ok(())
}

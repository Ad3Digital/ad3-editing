// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Bounded presentation timestamps for native frame selection, including VFR.
use crate::{
    Result,
    media::{SourceVersion, Tools},
};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

#[derive(Debug)]
pub struct VideoClock {
    pub timestamps: Vec<f64>,
    pub(crate) version: Option<SourceVersion>,
}

impl VideoClock {
    /// Last frame presented at or before the requested track time.
    pub fn at(&self, time: f64) -> Option<(usize, f64)> {
        if !time.is_finite() {
            return None;
        }
        let end = self
            .timestamps
            .partition_point(|timestamp| *timestamp <= time);
        let index = end.checked_sub(1)?;
        Some((index, self.timestamps[index]))
    }
}

fn read_clock(reader: impl Read, cancel: Option<&AtomicBool>) -> Result<VideoClock> {
    let mut reader = BufReader::new(reader);
    let mut timestamps = Vec::new();
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("Video clock cancelled".into());
        }
        let mut line = Vec::new();
        let read = reader
            .by_ref()
            .take(4097)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if read > 4096 {
            return Err("Video clock metadata line exceeds 4096 bytes".into());
        }
        let line = std::str::from_utf8(&line)
            .map_err(|error| error.to_string())?
            .trim();
        if line.is_empty() {
            continue;
        }
        // FFprobe may append side-data descriptions to a frame's CSV row.
        let timestamp = line
            .split(',')
            .next()
            .unwrap()
            .parse::<f64>()
            .map_err(|_| "Video frame has no usable presentation timestamp")?;
        if !timestamp.is_finite()
            || timestamps
                .last()
                .is_some_and(|previous| timestamp < *previous)
        {
            return Err("Video presentation timestamps must be finite and ordered".into());
        }
        if timestamps.len() >= 1_000_000 {
            return Err("Video clock exceeds 1000000 frames".into());
        }
        timestamps.push(timestamp);
    }
    if timestamps.is_empty() {
        return Err("Video clock has no frames".into());
    }
    Ok(VideoClock {
        timestamps,
        version: None,
    })
}

pub fn probe_clock(tools: &Tools, path: &Path, cancel: Option<&AtomicBool>) -> Result<VideoClock> {
    let version = SourceVersion::read(path)?;
    let mut process = crate::media::command(&tools.ffprobe);
    process
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp_time:frame_side_data=",
            "-of",
            "csv=p=0",
        ])
        .arg(path);
    let mut clock = crate::media::checked_stream_cancellable(
        &mut process,
        cancel,
        Duration::from_secs(120),
        |stdout| read_clock(stdout, cancel),
    )?;
    if SourceVersion::read(path)? != version {
        return Err("Video changed while reading presentation timestamps".into());
    }
    clock.version = Some(version);
    Ok(clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_retains_irregular_frame_times_and_selects_presented_frame() {
        let clock =
            read_clock(&b"-0.25\n0.0,side data\n\n0.2\n1.75\n1.75\n2.0\n"[..], None).unwrap();
        assert_eq!(clock.timestamps, [-0.25, 0.0, 0.2, 1.75, 1.75, 2.0]);
        assert_eq!(clock.at(0.5), Some((2, 0.2)));
        assert_eq!(clock.at(1.75), Some((4, 1.75)));
        assert_eq!(clock.at(5.0), Some((5, 2.0)));
        assert_eq!(clock.at(-1.0), None);
        assert_eq!(clock.at(f64::NAN), None);
        for bytes in [b"".as_slice(), b"N/A\n", b"NaN\n", b"2\n1\n"] {
            assert!(read_clock(bytes, None).is_err());
        }
        assert!(read_clock(&b"0\n"[..], Some(&AtomicBool::new(true))).is_err());
        assert!(read_clock(std::io::repeat(b'0'), None).is_err());
    }
}

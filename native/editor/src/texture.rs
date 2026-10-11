// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Source pixels and a bounded, shared cache. Composition/geometry never runs
//! in the decoder. Image files use Rust codecs; FFmpeg supplies video pixels.
use crate::{
    Result,
    encoder::cancelled,
    media::{Probe, SourceVersion as Version, Tools, checked_output_cancellable_limit, command},
    timing::round_frame,
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, metadata::Orientation};
use std::{
    collections::VecDeque,
    fs,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};
use tiny_skia::{IntSize, Pixmap};

const MAX_PIXELS: u64 = 4096 * 4096;
const CACHE_BYTES: usize = 128 * 1024 * 1024;
const CACHE_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    Cover,
    Contain,
    Fill,
}
impl Fit {
    /// The reconciler removes ScaleMode for missing/unrecognized props, and
    /// renderMedia's missing-store fallback is COVER (enum zero).
    pub fn from_prop(value: Option<&str>) -> Self {
        match value {
            Some("contain") => Self::Contain,
            Some("fill") => Self::Fill,
            _ => Self::Cover,
        }
    }
    pub fn rectangle(self, source: [f64; 2], target: [f64; 2]) -> [f64; 4] {
        if self == Self::Fill {
            return [0.0, 0.0, target[0], target[1]];
        }
        let x = target[0] / source[0];
        let y = target[1] / source[1];
        let scale = if self == Self::Contain {
            x.min(y)
        } else {
            x.max(y)
        };
        let width = source[0] * scale;
        let height = source[1] * scale;
        [
            (target[0] - width) / 2.0,
            (target[1] - height) / 2.0,
            width,
            height,
        ]
    }
}

#[derive(Clone, Debug)]
pub struct TextureRequest {
    pub path: PathBuf,
    pub probe: Probe,
    pub frame: TextureFrame,
}
#[derive(Clone, Copy, Debug)]
pub enum TextureFrame {
    Image,
    Video(f64),
    /// Resolve metadata-dependent clocks at pixel rendering time. A media
    /// failure must not poison the scene's structural compositor state.
    VideoClock {
        time: f64,
        origin: f64,
        rate: f64,
        fps: f64,
    },
}

struct Entry {
    version: Version,
    time: Option<u64>,
    pixels: Arc<Pixmap>,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
    clocks: VecDeque<(Version, Arc<crate::video_clock::VideoClock>)>,
    clock_bytes: usize,
}
pub struct TextureStore {
    tools: Tools,
    cache: Mutex<Cache>,
    sessions: Mutex<VecDeque<crate::video_session::VideoSession>>,
    budget: usize,
}
impl TextureStore {
    pub fn new(tools: Tools) -> Self {
        Self {
            tools,
            cache: Mutex::new(Cache::default()),
            sessions: Mutex::new(VecDeque::new()),
            budget: CACHE_BYTES,
        }
    }
    pub fn cached_bytes(&self) -> usize {
        self.cache.lock().map(|c| c.bytes).unwrap_or(0)
    }
    pub fn frame(
        &self,
        request: &TextureRequest,
        cancel: Option<&AtomicBool>,
    ) -> Result<Arc<Pixmap>> {
        if cancelled(cancel).is_err() {
            self.sessions
                .lock()
                .map_err(|_| "Video session lock failed")?
                .clear();
            return Err("Video texture cancelled".into());
        }
        let version = Version::read(&request.path)?;
        if !matches!(request.frame, TextureFrame::Image) && request.probe.version.is_none() {
            return Err("Video texture requires versioned probe metadata".into());
        }
        if request
            .probe
            .version
            .as_ref()
            .is_some_and(|expected| *expected != version)
        {
            return Err("Texture source changed since probing; request a new frame plan".into());
        }
        let source = match request.frame {
            TextureFrame::Image => None,
            TextureFrame::Video(time) => Some(time),
            TextureFrame::VideoClock {
                time,
                origin,
                rate,
                fps,
            } => Some(source_time(time, origin, rate, fps, &request.probe)?),
        };
        let clock = if source.is_some() {
            dimensions(request.probe.width, request.probe.height)?;
            let (display_width, display_height) = request.probe.display_dimensions()?;
            dimensions(display_width, display_height)?;
            let cached = {
                let mut cache = self.cache.lock().map_err(|_| "Texture cache lock failed")?;
                if let Some(index) = cache.clocks.iter().position(|(key, _)| *key == version) {
                    let entry = cache.clocks.remove(index).unwrap();
                    let clock = entry.1.clone();
                    cache.clocks.push_back(entry);
                    Some(clock)
                } else {
                    None
                }
            };
            Some(if let Some(clock) = cached {
                clock
            } else {
                let actual = self.tools.probe_cancellable(&version.path, cancel)?;
                if actual.width != request.probe.width
                    || actual.height != request.probe.height
                    || actual.display_dimensions()? != request.probe.display_dimensions()?
                {
                    return Err(
                        "Video texture dimensions exceeded its limit or changed since probing"
                            .into(),
                    );
                }
                let clock = Arc::new(crate::video_clock::probe_clock(
                    &self.tools,
                    &version.path,
                    cancel,
                )?);
                if clock.version.as_ref() != Some(&version) {
                    return Err("Texture changed while reading video clock".into());
                }
                let mut cache = self.cache.lock().map_err(|_| "Texture cache lock failed")?;
                let bytes = clock.timestamps.len() * std::mem::size_of::<f64>();
                cache.clocks.retain(|(key, _)| key.path != version.path);
                cache.clock_bytes = cache
                    .clocks
                    .iter()
                    .map(|(_, clock)| clock.timestamps.len() * 8)
                    .sum();
                while cache.clocks.len() >= 8 || cache.clock_bytes + bytes > 16 * 1024 * 1024 {
                    if let Some((_, oldest)) = cache.clocks.pop_front() {
                        cache.clock_bytes -= oldest.timestamps.len() * 8;
                    } else {
                        break;
                    }
                }
                cache.clock_bytes += bytes;
                cache.clocks.push_back((version.clone(), clock.clone()));
                clock
            })
        } else {
            None
        };
        let presented = match (source, clock.as_ref()) {
            (Some(time), Some(clock)) => {
                Some(clock.at(time).ok_or("No video frame at texture time")?)
            }
            _ => None,
        };
        let source = presented.map(|(_, time)| time);
        let time = source.map(|v| {
            if v == 0.0 {
                0.0_f64.to_bits()
            } else {
                v.to_bits()
            }
        });
        {
            let mut cache = self.cache.lock().map_err(|_| "Texture cache lock failed")?;
            // Discard old versions of a changed file before admitting another.
            let mut index = 0;
            while index < cache.entries.len() {
                if cache.entries[index].version.path == version.path
                    && cache.entries[index].version != version
                {
                    let entry = cache.entries.remove(index).unwrap();
                    cache.bytes -= entry.pixels.data().len();
                } else {
                    index += 1;
                }
            }
            if let Some(index) = cache
                .entries
                .iter()
                .position(|e| e.version == version && e.time == time)
            {
                let entry = cache.entries.remove(index).unwrap();
                let pixels = entry.pixels.clone();
                cache.entries.push_back(entry);
                return Ok(pixels);
            }
        }
        // Do not hold the cache lock across codec I/O or pixel conversion.
        let mut prefetched = Vec::new();
        let pixels = Arc::new(match source {
            None => decode_image(&version.path, cancel)?
                .ok_or("Image format is not ported to the Rust decoder yet")?,
            Some(_) => {
                let mut decoded = None;
                let (width, height) = request.probe.display_dimensions()?;
                let first = presented.unwrap().0;
                let frame_bytes = width as usize * height as usize * 4;
                let count = (self.budget.min(32 * 1024 * 1024) / frame_bytes).clamp(1, 4);
                let clock = clock.as_ref().unwrap();
                let end = (first + count).min(clock.timestamps.len());
                let mut sessions = self
                    .sessions
                    .lock()
                    .map_err(|_| "Video session lock failed")?;
                let mut session = if let Some(index) = sessions
                    .iter()
                    .position(|session| session.can_advance(&version, first))
                {
                    sessions.remove(index).unwrap()
                } else {
                    sessions.retain(|session| session.version().path != version.path);
                    let estimate = frame_bytes * 4
                        + (clock.timestamps.len() - first) * std::mem::size_of::<usize>();
                    while !sessions.is_empty()
                        && (sessions.len() >= 4
                            || sessions
                                .iter()
                                .map(|session| session.memory_bytes())
                                .sum::<usize>()
                                + estimate
                                > 128 * 1024 * 1024)
                    {
                        sessions.pop_front();
                    }
                    crate::video_session::VideoSession::start(
                        self.tools.clone(),
                        version.path.clone(),
                        request.probe.clone(),
                        clock.clone(),
                        first,
                    )?
                };
                for expected in first..end {
                    let next = session.frame_at(expected, cancel);
                    let (index, pixels) = match next {
                        Ok(Some(frame)) => frame,
                        Ok(None) => {
                            return Err("Video session ended before requested frame".into());
                        }
                        Err(error) => {
                            if cancelled(cancel).is_err() {
                                sessions.clear();
                            }
                            return Err(error);
                        }
                    };
                    if index != expected {
                        return Err("Video session frame order changed".into());
                    }
                    if index == first {
                        decoded = Some(pixels);
                    } else {
                        let stamp = clock.timestamps[index];
                        prefetched.push((
                            if stamp == 0.0 {
                                0.0f64.to_bits()
                            } else {
                                stamp.to_bits()
                            },
                            Arc::new(pixels),
                        ));
                    }
                }
                sessions.push_back(session);
                decoded.ok_or("Video decoder returned no texture")?
            }
        });
        cancelled(cancel)?;
        if Version::read(&request.path)? != version {
            return Err("Texture source changed during decoding".into());
        }
        let bytes = pixels.data().len();
        if bytes <= self.budget {
            let mut cache = self.cache.lock().map_err(|_| "Texture cache lock failed")?;
            if let Some(existing) = cache
                .entries
                .iter()
                .find(|e| e.version == version && e.time == time)
            {
                return Ok(existing.pixels.clone());
            }
            while cache.bytes + bytes > self.budget || cache.entries.len() >= CACHE_ENTRIES {
                if let Some(entry) = cache.entries.pop_front() {
                    cache.bytes -= entry.pixels.data().len();
                } else {
                    break;
                }
            }
            cache.bytes += bytes;
            cache.entries.push_back(Entry {
                version: version.clone(),
                time,
                pixels: pixels.clone(),
            });
            for (stamp, pixels) in prefetched {
                if cache
                    .entries
                    .iter()
                    .any(|entry| entry.version == version && entry.time == Some(stamp))
                {
                    continue;
                }
                let bytes = pixels.data().len();
                if bytes > self.budget {
                    continue;
                }
                while cache.bytes + bytes > self.budget || cache.entries.len() >= CACHE_ENTRIES {
                    if let Some(entry) = cache.entries.pop_front() {
                        cache.bytes -= entry.pixels.data().len();
                    } else {
                        break;
                    }
                }
                cache.bytes += bytes;
                cache.entries.push_back(Entry {
                    version: version.clone(),
                    time: Some(stamp),
                    pixels,
                });
            }
        }
        Ok(pixels)
    }
}

fn dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("Texture dimensions exceed the 16-megapixel decoder limit".into());
    }
    Ok(())
}
struct CancelFile<'a> {
    file: fs::File,
    cancel: Option<&'a AtomicBool>,
}
impl Read for CancelFile<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        cancelled(self.cancel).map_err(io::Error::other)?;
        self.file.read(bytes)
    }
}
impl Seek for CancelFile<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        cancelled(self.cancel).map_err(io::Error::other)?;
        self.file.seek(position)
    }
}
type PictureReader<'a> = ImageReader<BufReader<CancelFile<'a>>>;
fn reader<'a>(path: &Path, cancel: Option<&'a AtomicBool>) -> Result<Option<PictureReader<'a>>> {
    let file = CancelFile {
        file: fs::File::open(path).map_err(|e| e.to_string())?,
        cancel,
    };
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(
        reader.format(),
        Some(
            ImageFormat::Png
                | ImageFormat::Jpeg
                | ImageFormat::WebP
                | ImageFormat::Gif
                | ImageFormat::Bmp
                | ImageFormat::Tiff
        )
    ) {
        return Ok(None);
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(MAX_PIXELS as u32);
    limits.max_image_height = Some(MAX_PIXELS as u32);
    reader.limits(limits);
    Ok(Some(reader))
}
fn oriented_dimensions(size: (u32, u32), orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (size.1, size.0),
        _ => (size.0, size.1),
    }
}
pub fn probe_image(path: &Path, cancel: Option<&AtomicBool>) -> Result<Option<Probe>> {
    cancelled(cancel)?;
    let version = Version::read(path)?;
    let Some(reader) = reader(&version.path, cancel)? else {
        return Ok(None);
    };
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let size = decoder.dimensions();
    dimensions(size.0, size.1)?;
    let size = oriented_dimensions(size, decoder.orientation().map_err(|e| e.to_string())?);
    cancelled(cancel)?;
    if Version::read(path)? != version {
        return Err("Media source changed during image probing".into());
    }
    Ok(Some(Probe {
        width: size.0,
        height: size.1,
        has_video: true,
        still: true,
        version: Some(version),
        ..Default::default()
    }))
}
fn decode_image(path: &Path, cancel: Option<&AtomicBool>) -> Result<Option<Pixmap>> {
    let Some(reader) = reader(path, cancel)? else {
        return Ok(None);
    };
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions();
    dimensions(width, height)?;
    if decoder.total_bytes() > 128 * 1024 * 1024 {
        return Err("Decoded image exceeds memory limit".into());
    }
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    cancelled(cancel)?;
    pixels.apply_orientation(orientation);
    let pixels = pixels.into_rgba8();
    let (width, height) = pixels.dimensions();
    Ok(Some(premultiply(pixels.into_raw(), width, height, cancel)?))
}
pub(crate) fn premultiply(
    mut bytes: Vec<u8>,
    width: u32,
    height: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    dimensions(width, height)?;
    if bytes.len() != width as usize * height as usize * 4 {
        return Err("Decoder returned an incomplete texture".into());
    }
    for row in bytes.chunks_mut(width as usize * 4) {
        cancelled(cancel)?;
        for pixel in row.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(
        bytes,
        IntSize::from_wh(width, height).ok_or("Invalid texture size")?,
    )
    .ok_or("Invalid premultiplied texture".into())
}
/// Read only thumbnail-sized RGBA across the codec boundary. Source metadata
/// and the same decoder limits still apply; this bypasses full-frame caching.
pub fn decode_video_thumbnail(
    tools: &Tools,
    path: &Path,
    probe: &Probe,
    time: f64,
    size: [u32; 2],
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    cancelled(cancel)?;
    let version = Version::read(path)?;
    if probe.version.as_ref() != Some(&version) {
        return Err("Thumbnail source changed since probing".into());
    }
    let pixels = decode_video_sized(tools, path, probe, time, Some(size), cancel)?;
    if Version::read(path)? != version {
        return Err("Thumbnail source changed during decoding".into());
    }
    Ok(pixels)
}

fn decode_video_sized(
    tools: &Tools,
    path: &Path,
    probe: &Probe,
    time: f64,
    thumbnail: Option<[u32; 2]>,
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    if !time.is_finite() || time < 0.0 {
        return Err("Invalid source frame time".into());
    }
    let (source_width, source_height) = probe.display_dimensions()?;
    dimensions(source_width, source_height)?;
    dimensions(probe.width, probe.height)?;
    let [width, height] = thumbnail.unwrap_or([source_width, source_height]);
    dimensions(width, height)?;
    let mut decode = command(&tools.ffmpeg);
    decode
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-ss",
            &time.to_string(),
            "-seek_timestamp",
            "1",
            "-i",
        ])
        .arg(path);
    if thumbnail.is_some() {
        let fit = Fit::Cover.rectangle(
            [f64::from(source_width), f64::from(source_height)],
            [f64::from(width), f64::from(height)],
        );
        let scaled_width = fit[2].ceil() as u32;
        let scaled_height = fit[3].ceil() as u32;
        dimensions(scaled_width, scaled_height)?;
        decode.args(["-vf", &format!("scale={scaled_width}:{scaled_height}:flags=bilinear,setsar=1,format=rgba,crop={width}:{height}")]);
    } else if probe.pixel_aspect > 0.0 && (probe.pixel_aspect - 1.0).abs() > 1e-6 {
        // Normalize the decoded display pixels, including quarter-turn rotation,
        // before Rust applies the composition's objectFit and transforms.
        decode.args([
            "-vf",
            &format!("scale={width}:{height}:flags=bilinear,setsar=1"),
        ]);
    }
    decode.args([
        "-map",
        "0:V:0",
        "-an",
        "-sn",
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "pipe:1",
    ]);
    let output = checked_output_cancellable_limit(
        &mut decode,
        cancel,
        Duration::from_secs(120),
        u64::from(width) * u64::from(height) * 4,
    )?;
    premultiply(output.stdout, width, height, cancel)
}

/// Original playback rounds local time at the composition FPS, then the video
/// decoder converts it to the source FPS. Keep the two roundings separate.
pub fn source_time(
    global_time: f64,
    origin: f64,
    rate: f64,
    fps: f64,
    probe: &Probe,
) -> Result<f64> {
    if ![global_time, origin, rate, fps, probe.frame_rate]
        .iter()
        .all(|n| n.is_finite())
        || fps <= 0.0
        || rate <= 0.0
        || probe.frame_rate <= 0.0
    {
        return Err("Invalid video source clock".into());
    }
    let local = round_frame((round_frame(global_time * fps) - origin * fps) * rate);
    let mut frame = round_frame(local / fps * probe.frame_rate).max(0.0);
    if let Some(duration) = probe.duration {
        frame = frame.min((round_frame(duration * probe.frame_rate) - 1.0).max(0.0));
    }
    let time = frame / probe.frame_rate + probe.start_time.max(0.0);
    if !time.is_finite() {
        return Err("Video source clock overflows".into());
    }
    Ok(time)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;
    #[test]
    #[ignore = "requires local FFmpeg and FFprobe"]
    fn video_read_ahead_supplies_later_frames_without_starting_codecs() {
        let folder = std::env::temp_dir().join(format!(
            "ad3-prefetch-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&folder).unwrap();
        let source = folder.join("video.mkv");
        let tools = Tools::default();
        crate::media::checked_output(
            command(&tools.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=red:s=64x32:r=4:d=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:s=64x32:r=4:d=2",
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
        let mut request = TextureRequest {
            path: source.clone(),
            probe: tools.probe(&source).unwrap(),
            frame: TextureFrame::Video(0.0),
        };
        let mut store = TextureStore::new(tools);
        let first = store.frame(&request, None).unwrap();
        assert!(first.pixel(32, 16).unwrap().red() > 240);
        assert_eq!(store.cached_bytes(), 4 * 64 * 32 * 4);
        let second_source = folder.join("second.mkv");
        fs::copy(&source, &second_source).unwrap();
        let mut second = TextureRequest {
            path: second_source.clone(),
            probe: store.tools.probe(&second_source).unwrap(),
            frame: TextureFrame::Video(0.0),
        };
        assert!(
            store
                .frame(&second, None)
                .unwrap()
                .pixel(32, 16)
                .unwrap()
                .red()
                > 240
        );
        assert_eq!(store.sessions.lock().unwrap().len(), 2);
        let saved_tools = store.tools.clone();
        store.tools.ffmpeg = folder.join("missing-ffmpeg.exe");
        store.tools.ffprobe = folder.join("missing-ffprobe.exe");
        request.frame = TextureFrame::Video(1.5);
        let later = store.frame(&request, None).unwrap();
        assert!(later.pixel(32, 16).unwrap().blue() > 240);
        // Expire the active decoder deterministically instead of waiting 90s.
        let old_worker = {
            let mut sessions = store.sessions.lock().unwrap();
            let session = sessions
                .iter_mut()
                .find(|session| session.version().path == fs::canonicalize(&source).unwrap())
                .unwrap();
            session.expire_for_test();
            session.worker_id()
        };
        store.tools = saved_tools.clone();
        request.frame = TextureFrame::Video(2.5);
        {
            let mut cache = store.cache.lock().unwrap();
            cache.entries.clear();
            cache.bytes = 0;
        }
        assert!(
            store
                .frame(&request, None)
                .unwrap()
                .pixel(32, 16)
                .unwrap()
                .blue()
                > 240
        );
        let new_worker = store
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|session| session.version().path == fs::canonicalize(&source).unwrap())
            .unwrap()
            .worker_id();
        assert_ne!(old_worker, new_worker);
        store.tools.ffmpeg = folder.join("missing-ffmpeg.exe");
        store.tools.ffprobe = folder.join("missing-ffprobe.exe");
        second.frame = TextureFrame::Video(1.0);
        assert!(
            store
                .frame(&second, None)
                .unwrap()
                .pixel(32, 16)
                .unwrap()
                .blue()
                > 240
        );
        assert!(store.frame(&request, Some(&AtomicBool::new(true))).is_err());
        assert!(store.sessions.lock().unwrap().is_empty());
        store.tools = saved_tools;
        request.frame = TextureFrame::Video(0.0);
        assert_eq!(first.data(), store.frame(&request, None).unwrap().data());
        store.tools = Tools::default();
        for index in 0..5 {
            let path = folder.join(format!("pool-{index}.mkv"));
            fs::copy(&source, &path).unwrap();
            let request = TextureRequest {
                probe: store.tools.probe(&path).unwrap(),
                path,
                frame: TextureFrame::Video(0.0),
            };
            store.frame(&request, None).unwrap();
            assert!(store.sessions.lock().unwrap().len() <= 4);
        }
        assert_eq!(store.sessions.lock().unwrap().len(), 4);
        drop(store);
        let canonical = fs::canonicalize(folder).unwrap();
        assert!(canonical.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        fs::remove_dir_all(canonical).unwrap();
    }
    #[test]
    fn texture_lru_evicts_without_invalidating_issued_frames() {
        let folder = std::env::temp_dir().join(format!(
            "ad3-texture-lru-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&folder).unwrap();
        for name in ["a.png", "b.png"] {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
                .save(folder.join(name))
                .unwrap();
        }
        let mut store = TextureStore::new(Tools::default());
        let token = AtomicBool::new(false);
        let input = reader(&folder.join("a.png"), Some(&token))
            .unwrap()
            .unwrap();
        token.store(true, Ordering::Relaxed);
        // The header was opened before cancellation; decoder construction must
        // still check the token when it reads/seeks the already-open file.
        assert!(input.into_inner().seek(SeekFrom::Start(0)).is_err());
        assert!(
            probe_image(&folder.join("a.png"), Some(&token))
                .err()
                .unwrap()
                .contains("cancelled")
        );
        store.budget = 16;
        let a = TextureRequest {
            path: folder.join("a.png"),
            probe: Probe::default(),
            frame: TextureFrame::Image,
        };
        let b = TextureRequest {
            path: folder.join("b.png"),
            ..a.clone()
        };
        let original = store.frame(&a, None).unwrap();
        store.frame(&b, None).unwrap();
        assert_eq!(store.cached_bytes(), 16);
        let reloaded = store.frame(&a, None).unwrap();
        assert!(!Arc::ptr_eq(&original, &reloaded));
        assert_eq!(original.data(), reloaded.data());
        assert_eq!(store.cached_bytes(), 16);
        fs::remove_file(folder.join("a.png")).unwrap();
        fs::remove_file(folder.join("b.png")).unwrap();
        fs::remove_dir(folder).unwrap();
    }
}

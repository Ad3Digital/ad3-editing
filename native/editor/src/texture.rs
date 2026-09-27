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
}
pub struct TextureStore {
    tools: Tools,
    cache: Mutex<Cache>,
    budget: usize,
}
impl TextureStore {
    pub fn new(tools: Tools) -> Self {
        Self {
            tools,
            cache: Mutex::new(Cache::default()),
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
        cancelled(cancel)?;
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
        let pixels = Arc::new(match source {
            None => decode_image(&version.path, cancel)?
                .ok_or("Image format is not ported to the Rust decoder yet")?,
            Some(time) => decode_video(&self.tools, &version.path, &request.probe, time, cancel)?,
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
                version,
                time,
                pixels: pixels.clone(),
            });
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
pub(crate) fn probe_image(path: &Path, cancel: Option<&AtomicBool>) -> Result<Option<Probe>> {
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
fn premultiply(
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
fn decode_video(
    tools: &Tools,
    path: &Path,
    probe: &Probe,
    time: f64,
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    if !time.is_finite() || time < 0.0 {
        return Err("Invalid source frame time".into());
    }
    let (width, height) = probe.display_dimensions()?;
    dimensions(width, height)?;
    dimensions(probe.width, probe.height)?;
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
    if probe.pixel_aspect > 0.0 && (probe.pixel_aspect - 1.0).abs() > 1e-6 {
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

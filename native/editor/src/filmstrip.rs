//! Filmstrip geometry and sampling contract from runtime/media/transcode.ts.
//! Pixel decoding and timestamp ruler drawing are integrated separately.
#[cfg(test)]
use crate::texture::decode_video_thumbnail;
use crate::{Result, encoder::cancelled, media::Tools};
use std::{path::Path, sync::atomic::AtomicBool};
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};

pub const HELP: &str = "dapi-rust media filmstrip <video-or-library-ref> [options]\nAlias: film\n\n  -s, --start <time>    Start in source seconds (default: 0)\n  -e, --end <time>      End in source seconds (default: video duration)\n  -x, --scale <factor>  Thumbnail scale, clamped to 0.25-4 (default: 1)\n  -o, --output <png>    Output PNG (default: unique system-temp file)\n      --project <dir>   Resolve assets.yml library paths/IDs in this project\n  -h, --help            Show this help without decoding\n\nTime accepts seconds, 45f at 30 fps, MM:SS or HH:MM:SS.\nOutputs JSON with the absolute PNG path. Existing files are refused.\n";

pub fn command(
    args: &[String],
    tools: &Tools,
    cancel: Option<&AtomicBool>,
) -> Result<std::path::PathBuf> {
    let request = prepare_preview(args, "filmstrip", HELP, cancel)?;
    let canvas = render(
        tools,
        &request.source,
        request.start,
        request.end,
        request.scale,
        cancel,
    )?;
    write_png(&canvas, &request.output, cancel)?;
    Ok(request.output)
}

pub(crate) struct PreviewRequest {
    pub source: std::path::PathBuf,
    pub output: std::path::PathBuf,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub scale: Option<f64>,
}

pub(crate) fn prepare_preview(
    args: &[String],
    kind: &str,
    help: &str,
    cancel: Option<&AtomicBool>,
) -> Result<PreviewRequest> {
    let (path, flags) = args.split_first().ok_or(help)?;
    let mut start = None;
    let mut end = None;
    let mut scale = None;
    let mut output = None;
    let mut project = None;
    let mut index = 0;
    while index < flags.len() {
        let (flag, inline) = flags[index]
            .split_once('=')
            .map_or((flags[index].as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        index += 1;
        if !matches!(
            flag,
            "-s" | "--start" | "-e" | "--end" | "-x" | "--scale" | "-o" | "--output" | "--project"
        ) {
            return Err(format!("Unknown {kind} option: {flag}\n{help}"));
        }
        let value = if let Some(value) = inline {
            value
        } else {
            let value = flags
                .get(index)
                .ok_or_else(|| format!("Missing value for {flag}"))?;
            index += 1;
            value.as_str()
        };
        match flag {
            "-s" | "--start" => start = Some(crate::timing::parse_time(value)?),
            "-e" | "--end" => end = Some(crate::timing::parse_time(value)?),
            "-x" | "--scale" => {
                let value = value.parse::<f64>().map_err(|_| "Invalid preview scale")?;
                if !value.is_finite() || value <= 0.0 {
                    return Err("Preview scale must be positive and finite".into());
                }
                scale = Some(value);
            }
            "-o" | "--output" => output = Some(std::path::PathBuf::from(value)),
            "--project" => project = Some(std::path::PathBuf::from(value)),
            _ => unreachable!(),
        }
    }
    if start.is_some_and(|time| time < 0.0) || end.is_some_and(|time| time < 0.0) {
        return Err("Preview start/end must be non-negative times".into());
    }
    if let (Some(start), Some(end)) = (start, end)
        && start >= end
    {
        return Err("Preview start must be less than end".into());
    }
    cancelled(cancel)?;
    let output = output.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "ad3-{kind}-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    });
    let output = std::path::absolute(output).map_err(|error| error.to_string())?;
    if output.exists() {
        return Err("Preview output already exists".into());
    }
    let source = if let Some(project) = project {
        if !project.is_dir() {
            return Err("Preview project must be an existing directory".into());
        }
        crate::media::resolve_source(path, &project)?
    } else {
        path.into()
    };
    Ok(PreviewRequest {
        source,
        output,
        start,
        end,
        scale,
    })
}

pub(crate) fn write_png(canvas: &Pixmap, output: &Path, cancel: Option<&AtomicBool>) -> Result<()> {
    let png = canvas.encode_png().map_err(|error| error.to_string())?;
    cancelled(cancel)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        use std::io::Write;
        for chunk in png.chunks(1024 * 1024) {
            cancelled(cancel)?;
            file.write_all(chunk).map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())
    })();
    drop(file);
    if let Err(error) = result {
        crate::render::remove_owned_file(output).map_err(|cleanup| {
            format!("{error}; could not remove incomplete preview: {cleanup}")
        })?;
        return Err(error);
    }
    Ok(())
}

pub fn format_timestamp(seconds: f64, frame_rate: f64) -> Result<String> {
    if !seconds.is_finite() || seconds < 0.0 || !frame_rate.is_finite() || frame_rate <= 0.0 {
        return Err("Invalid filmstrip timestamp".into());
    }
    let fps = frame_rate.round().max(1.0);
    let frames = (seconds * fps).round();
    if fps > 9_007_199_254_740_991.0 || frames > 9_007_199_254_740_991.0 {
        return Err("Filmstrip timestamp exceeds the precise frame clock".into());
    }
    let fps = fps as u64;
    let frames = frames as u64;
    let seconds = frames / fps;
    Ok(format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        frames % fps
    ))
}

/// Draw the original row ruler: bold leading label, smaller subsequent labels,
/// two-pixel ticks and ten-pixel spacing to prevent overlapping timestamps.
pub fn draw_ruler(
    canvas: &mut Pixmap,
    layout: &FilmstripLayout,
    frame_rate: f64,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    cancelled(cancel)?;
    if [canvas.width(), canvas.height()] != layout.canvas {
        return Err("Filmstrip ruler canvas does not match its layout".into());
    }
    let mut fonts = crate::fonts::FontBook::default();
    let mut white = tiny_skia::Paint::default();
    white.set_color_rgba8(255, 255, 255, 255);
    let ruler_height = f64::from(layout.ruler_height);
    let row_width = f64::from(layout.canvas[0]);
    let tick_height = (ruler_height * 0.4).round();
    let mut label_end = f64::NEG_INFINITY;
    for index in 0..layout.cells() {
        cancelled(cancel)?;
        let column = index % layout.columns;
        let row_top = f64::from(index / layout.columns * layout.row_height);
        let column_x = f64::from(column * layout.thumbnail[0]);
        if column == 0 {
            label_end = f64::NEG_INFINITY;
        }
        let tick_x = column_x + 0.5;
        if tick_x >= label_end {
            let mut path = tiny_skia::PathBuilder::new();
            path.move_to(tick_x as f32, (row_top + ruler_height - tick_height) as f32);
            path.line_to(tick_x as f32, (row_top + ruler_height) as f32);
            if let Some(path) = path.finish() {
                canvas.stroke_path(
                    &path,
                    &white,
                    &tiny_skia::Stroke {
                        width: 2.0,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
        let time = layout.cell(index).ok_or("Invalid filmstrip ruler cell")?.1;
        let label = format_timestamp(time, frame_rate)?;
        let spec = crate::fonts::FontSpec {
            size: (ruler_height * if column == 0 { 0.9 } else { 0.8 }).round(),
            weight: if column == 0 { 700.0 } else { 400.0 },
            ..Default::default()
        };
        let shaped = fonts.shape(&label, &spec, crate::text_layout::Baseline::Middle, cancel)?;
        let label_x = column_x + 5.0;
        let width = shaped.metrics.width;
        if label_x < label_end + 10.0 || label_x + width > row_width {
            continue;
        }
        if let Some(path) = &shaped.path {
            canvas.fill_path(
                path,
                &white,
                tiny_skia::FillRule::Winding,
                Transform::from_translate(
                    label_x as f32,
                    (row_top + ruler_height / 2.0 + 2.0) as f32,
                ),
                None,
            );
        }
        label_end = label_x + width;
    }
    cancelled(cancel)?;
    Ok(())
}

/// Decode source frames into the native grid. Empty cells beyond the asset's
/// duration remain black, including short windows with the minimum FPS step.
/// Ruler drawing and output publishing are separate from this pixel stage.
pub fn render_frames(
    tools: &Tools,
    path: &Path,
    start: Option<f64>,
    end: Option<f64>,
    scale: Option<f64>,
    cancel: Option<&AtomicBool>,
) -> Result<(Pixmap, FilmstripLayout)> {
    cancelled(cancel)?;
    let probe = tools.probe_cancellable(path, cancel)?;
    if !probe.has_video || probe.still {
        return Err("Filmstrip needs a video asset; use waveform for audio".into());
    }
    let dimensions = probe.display_dimensions()?;
    let duration = probe
        .duration
        .ok_or("Filmstrip needs video duration metadata")?;
    let layout = FilmstripLayout::new(
        [dimensions.0, dimensions.1],
        duration,
        probe.frame_rate,
        start,
        end,
        scale,
    )?;
    let mut canvas = Pixmap::new(layout.canvas[0], layout.canvas[1])
        .ok_or("Cannot allocate filmstrip canvas")?;
    canvas.fill(Color::BLACK);
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };
    let clock = crate::video_clock::probe_clock(tools, path, cancel)?;
    let origin = clock.timestamps[0];
    let mut positions = std::collections::BTreeMap::<usize, Vec<[u32; 2]>>::new();
    for index in 0..layout.cells() {
        cancelled(cancel)?;
        let (position, time) = layout.cell(index).ok_or("Invalid filmstrip cell")?;
        if time >= duration {
            continue;
        }
        let time = time + origin;
        if !time.is_finite() {
            return Err("Filmstrip source clock overflows".into());
        }
        if let Some((frame, _)) = clock.at(time) {
            positions.entry(frame).or_default().push(position);
        }
    }
    let indices = positions.keys().copied().collect::<Vec<_>>();
    if !indices.is_empty() {
        crate::video_decode::selected_frames_with_clock(
            tools,
            path,
            &probe,
            &indices,
            layout.thumbnail,
            true,
            &clock,
            cancel,
            |index, thumbnail| {
                for position in &positions[&index] {
                    cancelled(cancel)?;
                    canvas.draw_pixmap(
                        position[0] as i32,
                        position[1] as i32,
                        thumbnail.as_ref(),
                        &paint,
                        Transform::identity(),
                        None,
                    );
                }
                Ok(())
            },
        )?;
    }
    cancelled(cancel)?;
    Ok((canvas, layout))
}

pub fn render(
    tools: &Tools,
    path: &Path,
    start: Option<f64>,
    end: Option<f64>,
    scale: Option<f64>,
    cancel: Option<&AtomicBool>,
) -> Result<Pixmap> {
    let (mut canvas, layout) = render_frames(tools, path, start, end, scale, cancel)?;
    draw_ruler(&mut canvas, &layout, layout.frame_rate, cancel)?;
    Ok(canvas)
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilmstripLayout {
    pub canvas: [u32; 2],
    pub thumbnail: [u32; 2],
    pub columns: u32,
    pub rows: u32,
    pub row_height: u32,
    pub ruler_height: u32,
    pub start: f64,
    pub end: f64,
    pub seconds_per_column: f64,
    pub frame_rate: f64,
}

impl FilmstripLayout {
    pub fn new(
        dimensions: [u32; 2],
        duration: f64,
        fps: f64,
        start: Option<f64>,
        end: Option<f64>,
        scale: Option<f64>,
    ) -> Result<Self> {
        if dimensions.contains(&0)
            || !duration.is_finite()
            || duration <= 0.0
            || !fps.is_finite()
            || fps <= 0.0
        {
            return Err(
                "Filmstrip requires positive video dimensions, duration and frame rate".into(),
            );
        }
        let scale = scale.unwrap_or(1.0);
        if !scale.is_finite() || scale <= 0.0 {
            return Err("Filmstrip scale must be a positive finite number".into());
        }
        let start = start.unwrap_or(0.0);
        let end = end.unwrap_or(duration);
        if !start.is_finite() || !end.is_finite() {
            return Err("Filmstrip window must be finite".into());
        }
        // Match the runtime's actual resolveWindow behavior: clamp first,
        // then reject empty/crossed windows, rather than extrapolating media.
        let start = start.clamp(0.0, duration);
        let end = end.clamp(start, duration);
        if end <= start {
            return Err("Filmstrip window is empty".into());
        }
        let height_tokens = (7.0 * scale.clamp(0.25, 4.0)).round().clamp(1.0, 51.0) as u32;
        let aspect = f64::from(dimensions[0]) / f64::from(dimensions[1]);
        let width_tokens = (f64::from(height_tokens) * aspect).floor().clamp(1.0, 64.0) as u32;
        let columns = 64 / width_tokens;
        let rows = 52 / (height_tokens + 1);
        let seconds_per_column = ((end - start) / f64::from(columns * rows)).max(1.0 / fps);
        if !seconds_per_column.is_finite()
            || !(start + f64::from(columns * rows - 1) * seconds_per_column).is_finite()
        {
            return Err("Filmstrip sampling exceeds the finite media clock".into());
        }
        Ok(Self {
            canvas: [width_tokens * columns * 28, (height_tokens + 1) * rows * 28],
            thumbnail: [width_tokens * 28, height_tokens * 28],
            columns,
            rows,
            row_height: (height_tokens + 1) * 28,
            ruler_height: 28,
            start,
            end,
            seconds_per_column,
            frame_rate: fps,
        })
    }

    pub fn cells(&self) -> u32 {
        self.columns * self.rows
    }

    pub fn cell(&self, index: u32) -> Option<([u32; 2], f64)> {
        (index < self.cells()).then(|| {
            (
                [
                    (index % self.columns) * self.thumbnail[0],
                    (index / self.columns) * self.row_height + self.ruler_height,
                ],
                self.start + f64::from(index) * self.seconds_per_column,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_rejects_negative_and_crossed_windows_before_media_io() {
        let tools = Tools {
            ffmpeg: "missing-filmstrip-ffmpeg".into(),
            ffprobe: "missing-filmstrip-ffprobe".into(),
        };
        for flags in [
            vec!["--start", "-1"],
            vec!["--end=-30f"],
            vec!["-s", "00:02", "-e", "00:01"],
            vec!["--start=1", "--end=1"],
        ] {
            let args = std::iter::once("missing-video.mkv")
                .chain(flags)
                .map(String::from)
                .collect::<Vec<_>>();
            let error = command(&args, &tools, None).unwrap_err();
            assert!(
                error.contains("non-negative") || error.contains("less than end"),
                "{error}"
            );
        }
    }

    #[test]
    fn timestamps_match_original_fixed_width_formatter_and_ruler_draws_native_labels() {
        for (seconds, fps, expected) in [
            (0.0, 30.0, "00:00:00:00"),
            (8.0 + 10.0 / 30.0, 30.0, "00:00:08:10"),
            (65.0, 30.0, "00:01:05:00"),
            (3601.0, 29.97, "01:00:01:00"),
            (1.0 / 60.0, 30.0, "00:00:00:01"),
            (360000.0, 30.0, "100:00:00:00"),
        ] {
            assert_eq!(format_timestamp(seconds, fps).unwrap(), expected);
        }
        assert!(format_timestamp(f64::INFINITY, 30.0).is_err());
        assert!(format_timestamp(1e20, 30.0).is_err());
        let layout = FilmstripLayout::new([1920, 1080], 30.0, 30.0, None, None, None).unwrap();
        let mut canvas = Pixmap::new(layout.canvas[0], layout.canvas[1]).unwrap();
        canvas.fill(Color::BLACK);
        draw_ruler(&mut canvas, &layout, 30.0, None).unwrap();
        for row in 0..layout.rows {
            let top = row * layout.row_height;
            let mut white = 0;
            for y in top..top + layout.ruler_height {
                for x in 5..200 {
                    if canvas.pixel(x, y).unwrap().red() > 0 {
                        white += 1;
                    }
                }
            }
            assert!(white > 50, "Missing ruler label at row {row}");
            assert_eq!(
                canvas
                    .pixel(200, top + layout.ruler_height + 20)
                    .unwrap()
                    .red(),
                0
            );
        }
        let before = canvas.data().to_vec();
        assert!(draw_ruler(&mut canvas, &layout, 30.0, Some(&AtomicBool::new(true))).is_err());
        assert_eq!(canvas.data(), before);
    }

    #[test]
    #[ignore = "requires installed FFmpeg and FFprobe"]
    fn native_filmstrip_decodes_changing_video_pixels_and_leaves_ruler_and_missing_cells_black() {
        let folder = std::env::temp_dir().join(format!(
            "ad3-filmstrip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("red blue {source}.mkv");
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
                    "color=red:s=64x32:r=2:d=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:s=64x32:r=2:d=1",
                    "-filter_complex",
                    "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                    "-map",
                    "[v]",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&path),
        )
        .unwrap();
        let (canvas, layout) = render_frames(&tools, &path, None, None, None, None).unwrap();
        assert_eq!([canvas.width(), canvas.height()], layout.canvas);
        let center = |index| {
            let (position, _) = layout.cell(index).unwrap();
            canvas
                .pixel(
                    position[0] + layout.thumbnail[0] / 2,
                    position[1] + layout.thumbnail[1] / 2,
                )
                .unwrap()
        };
        assert!(center(0).red() > 240 && center(0).blue() < 5);
        assert!(center(2).blue() > 240 && center(2).red() < 5);
        assert_eq!(center(4).red(), 0);
        assert_eq!(center(4).blue(), 0);
        assert_eq!(canvas.pixel(0, 0).unwrap().alpha(), 255);
        assert_eq!(canvas.pixel(0, 0).unwrap().red(), 0);
        let (window, window_layout) =
            render_frames(&tools, &path, Some(1.0), Some(2.0), Some(4.0), None).unwrap();
        let (position, time) = window_layout.cell(0).unwrap();
        assert_eq!(time, 1.0);
        assert!(
            window
                .pixel(position[0] + 10, position[1] + 10)
                .unwrap()
                .blue()
                > 240
        );
        assert!(
            render_frames(
                &tools,
                &path,
                None,
                None,
                None,
                Some(&AtomicBool::new(true))
            )
            .is_err()
        );
        assert!(path.is_file());
        let output = folder.join("filmstrip.png");
        let args = vec![
            path.to_string_lossy().into_owned(),
            "--start=1".into(),
            "--end=2".into(),
            "--scale=4".into(),
            "--output".into(),
            output.to_string_lossy().into_owned(),
        ];
        assert_eq!(command(&args, &tools, None).unwrap(), output);
        let png = Pixmap::load_png(&output).unwrap();
        assert_eq!([png.width(), png.height()], window_layout.canvas);
        assert!(png.pixel(10, 50).unwrap().blue() > 240);
        let saved = std::fs::read(&output).unwrap();
        assert!(command(&args, &tools, None).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), saved);
        let source_bytes = std::fs::read(&path).unwrap();
        let manifest = serde_yaml::to_string(&serde_json::json!({"assets": [
            {"id": "movie", "path": "videos/clip", "source": "red blue {source}.mkv"},
            {"path": "unrelated", "source": "missing.mp4"}
        ]}))
        .unwrap();
        std::fs::write(folder.join("assets.yml"), &manifest).unwrap();
        for (reference, filename) in [("videos/clip", "library.png"), ("movie", "id.png")] {
            let library_output = folder.join(filename);
            let args = vec![
                reference.into(),
                "--project".into(),
                folder.to_string_lossy().into_owned(),
                "--start=1".into(),
                "--end=2".into(),
                "--scale=4".into(),
                "--output".into(),
                library_output.to_string_lossy().into_owned(),
            ];
            command(&args, &tools, None).unwrap();
            let png = Pixmap::load_png(&library_output).unwrap();
            assert!(png.pixel(10, 50).unwrap().blue() > 240);
        }
        assert_eq!(std::fs::read(&path).unwrap(), source_bytes);
        assert_eq!(
            std::fs::read_to_string(folder.join("assets.yml")).unwrap(),
            manifest
        );
        let cover_path = folder.join("cover.mkv");
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
                    "color=red:s=64x32:r=10:d=0.1",
                    "-vf",
                    "drawbox=x=16:y=0:w=32:h=32:color=blue:t=fill",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&cover_path),
        )
        .unwrap();
        let mut cover_probe = tools.probe(&cover_path).unwrap();
        let mut streamed_cover = Vec::new();
        crate::video_decode::selected_frames_cover(
            &tools,
            &cover_path,
            &cover_probe,
            &[0],
            [8, 8],
            None,
            |_, pixels| {
                streamed_cover.push(pixels);
                Ok(())
            },
        )
        .unwrap();
        let thumbnail =
            decode_video_thumbnail(&tools, &cover_path, &cover_probe, 0.0, [8, 8], None).unwrap();
        assert_eq!([thumbnail.width(), thumbnail.height()], [8, 8]);
        assert_eq!(streamed_cover[0].data(), thumbnail.data());
        for x in [0, 3, 7] {
            assert!(
                thumbnail.pixel(x, 4).unwrap().blue() > 200,
                "Cover should crop red side bands"
            );
        }
        assert!(
            decode_video_thumbnail(&tools, &cover_path, &cover_probe, 0.0, [0, 8], None).is_err()
        );
        cover_probe.version = None;
        assert!(
            decode_video_thumbnail(&tools, &cover_path, &cover_probe, 0.0, [8, 8], None).is_err()
        );
        let landscape = folder.join("landscape.mp4");
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
                    "color=red:s=64x32:r=10:d=0.1",
                    "-vf",
                    "drawbox=x=32:y=0:w=32:h=32:color=blue:t=fill",
                    "-c:v",
                    "libx264",
                    "-crf",
                    "0",
                ])
                .arg(&landscape),
        )
        .unwrap();
        let rotated = folder.join("rotated.mp4");
        crate::media::checked_output(
            crate::media::command(&tools.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-display_rotation:v:0",
                    "90",
                    "-i",
                ])
                .arg(&landscape)
                .args(["-c", "copy"])
                .arg(&rotated),
        )
        .unwrap();
        let rotated_probe = tools.probe(&rotated).unwrap();
        assert_eq!(rotated_probe.rotation.abs(), 90.0);
        assert_eq!(rotated_probe.display_dimensions().unwrap(), (32, 64));
        let portrait =
            decode_video_thumbnail(&tools, &rotated, &rotated_probe, 0.0, [8, 16], None).unwrap();
        assert!(portrait.pixel(4, 2).unwrap().blue() > 240);
        assert!(portrait.pixel(4, 13).unwrap().red() > 240);
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert!(canonical.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()));
        std::fs::remove_dir_all(canonical).unwrap();
    }

    #[test]
    fn geometry_matches_original_token_grid_at_landscape_portrait_and_scale_limits() {
        for (dimensions, scale, canvas, thumbnail, columns, rows) in [
            ([1920, 1080], 1.0, [1680, 1344], [336, 196], 5, 6),
            ([1080, 1920], 1.0, [1764, 1344], [84, 196], 21, 6),
            ([1920, 1080], 0.25, [1764, 1428], [84, 56], 21, 17),
            ([1920, 1080], 4.0, [1372, 812], [1372, 784], 1, 1),
            ([u32::MAX, 1], 1.0, [1792, 1344], [1792, 196], 1, 6),
            ([1, u32::MAX], 1.0, [1792, 1344], [28, 196], 64, 6),
        ] {
            let layout =
                FilmstripLayout::new(dimensions, 30.0, 30.0, None, None, Some(scale)).unwrap();
            assert_eq!(layout.canvas, canvas);
            assert_eq!(layout.thumbnail, thumbnail);
            assert_eq!((layout.columns, layout.rows), (columns, rows));
            let (position, _) = layout.cell(layout.cells() - 1).unwrap();
            assert_eq!(position[0] + thumbnail[0], canvas[0]);
            assert_eq!(position[1] + thumbnail[1], canvas[1]);
            assert!(layout.cell(layout.cells()).is_none());
        }
        assert_eq!(
            FilmstripLayout::new([1920, 1080], 30.0, 30.0, None, None, Some(0.01)).unwrap(),
            FilmstripLayout::new([1920, 1080], 30.0, 30.0, None, None, Some(0.25)).unwrap()
        );
    }

    #[test]
    fn sampling_preserves_window_origin_and_at_least_one_source_frame_between_cells() {
        let layout =
            FilmstripLayout::new([1920, 1080], 120.0, 24.0, Some(12.0), Some(42.0), None).unwrap();
        assert_eq!(layout.seconds_per_column, 1.0);
        assert_eq!(layout.cell(0), Some(([0, 28], 12.0)));
        assert_eq!(layout.cell(5), Some(([0, 252], 17.0)));
        assert_eq!(layout.cell(29).unwrap().1, 41.0);
        let short =
            FilmstripLayout::new([1920, 1080], 1.0, 24.0, Some(0.0), Some(0.01), None).unwrap();
        assert_eq!(short.seconds_per_column, 1.0 / 24.0);
        // Original timestamps can extend past the window/asset when the
        // minimum frame interval dominates; the decoder handles missing cells.
        assert!(short.cell(29).unwrap().1 > 1.0);
        let clamped =
            FilmstripLayout::new([1920, 1080], 30.0, 30.0, Some(-2.0), Some(100.0), None).unwrap();
        assert_eq!((clamped.start, clamped.end), (0.0, 30.0));
        for (duration, fps, start, end, scale) in [
            (0.0, 30.0, None, None, None),
            (30.0, 0.0, None, None, None),
            (30.0, 30.0, Some(f64::NAN), None, None),
            (30.0, 30.0, Some(30.0), None, None),
            (30.0, 30.0, Some(20.0), Some(10.0), None),
            (30.0, 30.0, None, None, Some(-1.0)),
            (30.0, 30.0, None, None, Some(f64::INFINITY)),
        ] {
            assert!(FilmstripLayout::new([1920, 1080], duration, fps, start, end, scale).is_err());
        }
    }
}

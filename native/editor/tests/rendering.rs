// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    media::{MediaLibrary, Tools, checked_output, checked_output_cancellable, command},
    project::ProjectRenderer,
    render::RenderPlan,
    source::SourceDocument,
    timing::Timeline,
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn preset_and_keyframe_motion_is_sampled_for_capture_reverse_seek_and_export() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder = std::env::temp_dir().join(format!(
        "ad3-rust-motion-render-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&folder).unwrap();
    let source = r#"export default <stage><scene id="s" active width={64} height={32} workarea={[0,1]}>
      <rect id="r" width={16} height={16} fill="red" end={1}>
        <animation type="fade" duration={1}/>
        <keyframeTrack property="x"><keyframe value={0}/><keyframe time={0.9} value={40}/></keyframeTrack>
      </rect>
      <audio id="a" src="tone.wav" end={1}><animation type="gain" duration={1}/></audio>
    </scene></stage>"#;
    let document = SourceDocument::parse(folder.join("index.tsx"), source.into()).unwrap();
    let tools = Tools::default();
    checked_output(
        command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-ar",
                "48000",
                "-c:a",
                "pcm_f32le",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let mut renderer = ProjectRenderer::new(&document, &folder, tools.clone(), false).unwrap();
    let black = renderer
        .frame_plan("s", 0.0, 10.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert!(
        black
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] < 3 && p[1] < 3 && p[2] < 3)
    );
    let first = renderer
        .frame_plan("s", 0.4, 10.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    let last = renderer
        .frame_plan("s", 0.9, 10.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    let reverse = renderer
        .frame_plan("s", 0.4, 10.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(first, reverse);
    let pixel = (8 * 64 + 48) * 4;
    assert!(last[pixel] > 250 && last[pixel + 1] < 3);
    let output = folder.join("motion.mp4");
    let mut updates = Vec::new();
    let report = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &output,
        10.0,
        None,
        |n, total| updates.push((n, total)),
    )
    .unwrap();
    assert_eq!(report.frames, 10);
    assert_eq!(
        updates.len(),
        10,
        "Motion must not take the static single-plan export path"
    );
    assert!(report.warnings.is_empty());
    let decoded = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-ss", "0.9", "-i"])
            .arg(&output)
            .args([
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ]),
    )
    .unwrap();
    assert_eq!(decoded.stdout.len(), 64 * 32 * 4);
    assert!(decoded.stdout[pixel] > 220 && decoded.stdout[pixel + 1] < 20);
    assert!((tools.probe(&output).unwrap().duration.unwrap() - 1.0).abs() < 0.002);
    assert!(tools.probe(&output).unwrap().has_audio);
    let audio = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"]),
    )
    .unwrap();
    let samples = audio
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|n| f64::from(f32::from_le_bytes(*n)))
        .collect::<Vec<_>>();
    let rms = |from: usize, to: usize| {
        (samples[from..to].iter().map(|n| n * n).sum::<f64>() / (to - from) as f64).sqrt()
    };
    let quiet = rms(1440, 2880);
    let middle = rms(15000, 18000);
    let loud = rms(39000, 42000);
    assert!(
        quiet < 0.001 && loud > 0.02 && loud > middle * 1.5,
        "gain RMS {quiet} -> {middle} -> {loud}"
    );
    let late = SourceDocument::parse(
        folder.join("late.tsx"),
        r#"export default <stage>
      <scene id="s" active width={32} height={16} fill="red" workarea={[9960004/120,9960011/120]}/>
    </stage>"#
            .into(),
    )
    .unwrap();
    let late_output = folder.join("late.mp4");
    let late_report = ProjectRenderer::export(
        &late,
        &folder,
        tools.clone(),
        &late_output,
        120.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(late_report.frames, 7);
    assert_eq!(late_report.video_duration, 7.0 / 120.0);
    let late_frames = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&late_output)
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]),
    )
    .unwrap();
    assert_eq!(late_frames.stdout.len(), 7 * 32 * 16 * 4);
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-motion-render-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn native_export_preserves_trims_speed_audio_workarea_and_composition() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder =
        std::env::temp_dir().join(format!("ad3-rust-render-{}-{stamp}", std::process::id()));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
    // The glyph paint is color (not rect fill), has no default, ignores color
    // alpha, and applies the separate opacity property.
    let glyphs = r##"export default () => <stage><scene id="s" active width={160} height={90} fill="black" workarea={[0,1]}>
      <text id="visible" x={5} y={5} fontFamily="Arial" fontSize={32} color="#00f0" opacity={0.5} end={1}>RUST</text>
      <text id="unpainted" x={5} y={50} end={1}>Invisible</text>
    </scene></stage>"##;
    let document = SourceDocument::parse(folder.join("glyphs.tsx"), glyphs.into()).unwrap();
    let library = MediaLibrary::load(&document.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&document.root, &library.durations).unwrap();
    let plan = RenderPlan::build(&document.root, &timeline, &library, "s", 30.0, None).unwrap();
    assert_eq!(plan.graph.matches("drawtext").count(), 1);
    let glyph_frame = plan.frame(&tools, &folder).unwrap();
    assert!(
        glyph_frame
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[2] > 100 && pixel[2] < 150 && pixel[0] < 10)
    );
    let colors = r#"export default <stage><scene id="s" active width={32} height={16} fill={255} workarea={[0,1]}>
      <rect id="left" width={16} height={16} fill="rgba(255,0,0,0.1)" opacity={0.5}/>
      <rect id="right" x={16} width={16} height={16} fill="hsl(120 100% 50%)"/>
    </scene></stage>"#;
    let document = SourceDocument::parse(folder.join("colors.tsx"), colors.into()).unwrap();
    let library = MediaLibrary::load(&document.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&document.root, &library.durations).unwrap();
    let frame = RenderPlan::build(&document.root, &timeline, &library, "s", 30.0, None)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    let left = &frame[(8 * 32 + 8) * 4..(8 * 32 + 8) * 4 + 3];
    assert!(
        (122..=132).contains(&left[0]) && left[1] < 4 && (122..=132).contains(&left[2]),
        "{left:?}"
    );
    let right = &frame[(8 * 32 + 24) * 4..(8 * 32 + 24) * 4 + 3];
    assert!(right[0] < 4 && right[1] > 250 && right[2] < 4, "{right:?}");
    let groups = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,1]}>
      <group opacity={0.5}>
        <rect width={32} height={16} fill="red"/>
        <group><rect x={16} width={16} height={16} fill="blue"/></group>
      </group>
    </scene></stage>"#;
    let document = SourceDocument::parse(folder.join("groups.tsx"), groups.into()).unwrap();
    let library = MediaLibrary::load(&document.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&document.root, &library.durations).unwrap();
    let frame = RenderPlan::build(&document.root, &timeline, &library, "s", 30.0, None)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    let overlap = &frame[(8 * 32 + 24) * 4..(8 * 32 + 24) * 4 + 3];
    assert!(
        (60..=68).contains(&overlap[0]) && overlap[1] < 4 && (124..=132).contains(&overlap[2]),
        "group per-draw alpha {overlap:?}"
    );
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let trigger = cancel.clone();
    let cancel_thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        trigger.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let error = checked_output_cancellable(
        command(&tools.ffmpeg).args([
            "-v",
            "error",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-f",
            "null",
            "-",
        ]),
        Some(&cancel),
        std::time::Duration::from_secs(5),
    )
    .unwrap_err();
    cancel_thread.join().unwrap();
    assert!(error.contains("cancelled"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let source = folder.join("take.mp4");
    checked_output(
        command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=4",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source),
    )
    .unwrap();
    let text = r#"export default () => <stage><scene id="main" active width={160} height={90} fill="black" workarea={[0.25,0.75]}>
        <video id="take" src="take.mp4" sourceIn={1} sourceOut={3} playbackRate={2} width={160} height={90} volume={-6}/>
        <rect id="overlay" width={8} height={8} fill="blue" end={1}/>
    </scene></stage>"#;
    let entry = folder.join("index.tsx");
    fs::write(&entry, text).unwrap();
    let document = SourceDocument::open(&folder).unwrap();
    let library = MediaLibrary::load(&document.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&document.root, &library.durations).unwrap();
    let plan = RenderPlan::build(&document.root, &timeline, &library, "main", 30.0, None).unwrap();
    let export = folder.join("result.mp4");
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert!(
        plan.export_cancellable(&tools, &folder, &export, Some(&cancelled))
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(!export.exists());
    assert!(
        plan.export(&tools, &folder, &folder.join("missing/result.mp4"))
            .is_err()
    );
    assert!(
        RenderPlan::build(
            &document.root,
            &timeline,
            &library,
            "main",
            30.0,
            Some([0.0, 1e12])
        )
        .is_err()
    );
    plan.export(&tools, &folder, &export).unwrap();
    let probe = tools.probe(&export).unwrap();
    assert_eq!(
        (probe.width, probe.height, probe.has_audio),
        (160, 90, true)
    );
    assert!((probe.duration.unwrap() - 0.5).abs() < 0.04, "{probe:?}");
    assert_eq!(probe.frame_rate, 30.0);
    assert_eq!(probe.sample_rate, 48000);
    let a = tools.rgba(&export, 0.0, 160, 90).unwrap();
    let b = tools.rgba(&export, 0.3, 160, 90).unwrap();
    assert_ne!(a, b, "Exported source must advance");
    let pixel = &a[(2 * 160 + 2) * 4..][..4];
    assert!(
        pixel[2] > 150 && pixel[0] < 80 && pixel[1] < 80,
        "Overlay pixel: {pixel:?}"
    );
    let samples = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&export)
            .args(["-map", "0:a:0", "-ac", "1", "-f", "f32le", "pipe:1"]),
    )
    .unwrap()
    .stdout;
    let samples = samples
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect::<Vec<_>>();
    let rms = (samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    assert!(
        rms > 0.025 && rms < 0.065,
        "Decibel gain must be audible and attenuated: rms={rms}"
    );
    let before = fs::read(&export).unwrap();
    assert!(plan.export(&tools, &folder, &export).is_err());
    assert_eq!(fs::read(&export).unwrap(), before);
    let frame = RenderPlan::build(
        &document.root,
        &timeline,
        &library,
        "main",
        30.0,
        Some([0.25, 0.25 + 1.0 / 30.0]),
    )
    .unwrap();
    assert_eq!(frame.frame(&tools, &folder).unwrap().len(), 160 * 90 * 4);
    frame
        .capture(&tools, &folder, &folder.join("capture.png"))
        .unwrap();
    assert!(folder.join("capture.png").metadata().unwrap().len() > 100);
    let silent = folder.join("silent.mp4");
    checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-n", "-i"])
            .arg(&source)
            .args(["-an", "-c:v", "copy"])
            .arg(&silent),
    )
    .unwrap();
    let bad = SourceDocument::parse(
        folder.join("bad.tsx"),
        r#"export default () => <stage><scene id="s"><audio src="silent.mp4"/></scene></stage>"#
            .into(),
    )
    .unwrap();
    assert!(
        MediaLibrary::load(&bad.root, &folder, &tools)
            .unwrap_err()
            .contains("no audio stream")
    );
    assert_eq!(
        fs::read_dir(folder.join(".diffusion/native-render"))
            .unwrap()
            .count(),
        0,
        "Render jobs must clean their own files"
    );
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-render-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn reactive_export_advances_video_and_mixes_dynamic_gain_with_frozen_modules() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder = std::env::temp_dir().join(format!(
        "ad3-rust-reactive-render-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
    checked_output(
        command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-n",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let source = r#"
        import {useTicker} from '@diffusionstudio/jsx';
        import {color} from './palette';
        export default ()=>{
            const t=useTicker();
            return <stage><scene id="s" active width={160} height={90} fill="black" workarea={[0.2,1.2]}>
                <rect id="moving" width={20} height={20} x={t.time()<0.7?0:100} fill={t.playing()?color:'green'} end={2}/>
                <audio id="tone" src="tone.wav" volume={t.time()<0.7?0:-12} end={2}/>
            </scene></stage>;
        }
    "#;
    fs::write(folder.join("index.tsx"), source).unwrap();
    fs::write(folder.join("palette.ts"), "export const color='red';").unwrap();
    let document = SourceDocument::open(&folder).unwrap();
    fs::write(folder.join("palette.ts"), "export const color='blue';").unwrap();
    let output = folder.join("reactive.mp4");
    let mut frames = Vec::new();
    let report = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &output,
        10.0,
        None,
        |frame, total| frames.push((frame, total)),
    )
    .unwrap();
    assert_eq!(report.frames, 10);
    assert_eq!(frames.last(), Some(&(10, 10)));
    assert!(report.warnings.is_empty());
    let probe = tools.probe(&output).unwrap();
    assert_eq!(
        (probe.width, probe.height, probe.frame_rate, probe.has_audio),
        (160, 90, 10.0, true)
    );
    assert!((probe.duration.unwrap() - 1.0).abs() < 0.03);
    let first = tools.rgba(&output, 0.0, 160, 90).unwrap();
    let later = tools.rgba(&output, 0.6, 160, 90).unwrap();
    let repeated = folder.join("repeated.mp4");
    ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &repeated,
        10.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        first,
        tools.rgba(&repeated, 0.0, 160, 90).unwrap(),
        "A second export must mount fresh state"
    );
    let left = (5 * 160 + 5) * 4;
    let right = (5 * 160 + 105) * 4;
    assert!(
        first[left] > 180 && first[left + 2] < 30,
        "Frozen palette must stay red"
    );
    assert!(
        later[left] < 30 && later[right] > 180,
        "Ticker must move the rectangle"
    );
    let pcm = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-map", "0:a:0", "-ac", "1", "-f", "f32le", "pipe:1"]),
    )
    .unwrap()
    .stdout;
    let samples = pcm
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes) as f64)
        .collect::<Vec<_>>();
    let rms = |range: std::ops::Range<usize>| {
        (samples[range.clone()].iter().map(|v| v * v).sum::<f64>() / range.len() as f64).sqrt()
    };
    let loud = rms(4800..14400);
    let quiet = rms(33600..43200);
    assert!(
        (quiet / loud - 10_f64.powf(-12.0 / 20.0)).abs() < 0.04,
        "Gain ratio {}",
        quiet / loud
    );
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancelled = folder.join("cancelled.mp4");
    let error = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &cancelled,
        10.0,
        Some(cancel.clone()),
        |frame, _| {
            if frame == 1 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
    )
    .unwrap_err();
    assert!(error.contains("cancelled"), "{error}");
    assert!(!cancelled.exists());
    assert_eq!(
        fs::read_dir(folder.join(".diffusion/native-render"))
            .unwrap()
            .count(),
        0
    );
    let before = fs::read(&output).unwrap();
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            10.0,
            None,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(&output).unwrap(), before);
    // Capture mounts authored seconds into its output-rate world first.
    // This 2/30-second workarea rounds to two whole frames at 24 fps.
    let short = source.replace("[0.2,1.2]", "[0,2/30]").replace(
        "<audio id=\"tone\" src=\"tone.wav\" volume={t.time()<0.7?0:-12} end={2}/>",
        "",
    );
    fs::write(folder.join("short.tsx"), short).unwrap();
    let short_doc = SourceDocument::open(&folder.join("short.tsx")).unwrap();
    let fractional = folder.join("fractional.mp4");
    let short_report = ProjectRenderer::export(
        &short_doc,
        &folder,
        tools.clone(),
        &fractional,
        24.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(short_report.frames, 2);
    assert!((short_report.duration - 2.0 / 24.0).abs() < 1e-8);
    assert!((short_report.workarea_duration - 2.0 / 24.0).abs() < 1e-8);
    assert!(
        (tools.probe(&fractional).unwrap().duration.unwrap() - short_report.duration).abs() < 0.002
    );
    assert_eq!(
        fs::read_to_string(folder.join("index.tsx")).unwrap(),
        source
    );
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-reactive-render-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

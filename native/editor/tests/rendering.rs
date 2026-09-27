// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    media::{MediaLibrary, Tools, checked_output, checked_output_cancellable, command},
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

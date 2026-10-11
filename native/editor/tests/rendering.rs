// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    media::{MediaLibrary, Tools, checked_output, checked_output_cancellable, command},
    project::ProjectRenderer,
    render::RenderPlan,
    source::SourceDocument,
    texture::TextureFrame,
    timing::Timeline,
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

// Pixel regressions request small canvases explicitly; production exports use
// the original encoder's 1080p default when no resolution is saved.
fn export_height(folder: &std::path::Path, scene: &str, height: u32) {
    let path = folder.join("package.json");
    let mut config: serde_json::Value = fs::read(&path)
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .unwrap_or_else(|| serde_json::json!({}));
    config["diffusion"]["export"][scene]["video"]["resolution"] = serde_json::json!(height);
    fs::write(path, config.to_string()).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn cli_export_uses_saved_fps_audio_and_explicit_fps_override() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-cli-export-prefs-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
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
                "sine=frequency=440:duration=0.2",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let source = r#"export default <stage><scene id="s" active width={32} height={16} fill="red" workarea={[0,1/120]}><audio src="tone.wav"/></scene></stage>;"#;
    fs::write(folder.join("index.tsx"), source).unwrap();
    let package = r#"{"diffusion":{"export":{"s":{"video":{"fps":120,"resolution":16},"audio":{"enabled":false,"codec":"unused"}}}}}"#;
    fs::write(folder.join("package.json"), package).unwrap();
    for (name, override_fps, frames) in [("saved", None, 1), ("override", Some("240"), 2)] {
        let output = folder.join(format!("{name}.mp4"));
        let mut cli = std::process::Command::new(env!("CARGO_BIN_EXE_dapi-rust"));
        cli.arg("export").arg(&folder).arg(&output);
        if let Some(fps) = override_fps {
            cli.arg(fps);
        }
        let result = cli.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(
            report["fps"],
            if override_fps.is_some() { 240.0 } else { 120.0 }
        );
        assert_eq!(report["frames"], frames);
        assert!(!tools.probe(&output).unwrap().has_audio);
    }
    fs::write(
        folder.join("package.json"),
        package.replace("\"fps\":120", "\"fps\":0"),
    )
    .unwrap();
    let invalid = folder.join("invalid.mp4");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_dapi-rust"))
        .arg("export")
        .arg(&folder)
        .arg(&invalid)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!invalid.exists());
    assert_eq!(
        fs::read_to_string(folder.join("index.tsx")).unwrap(),
        source
    );
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn audio_only_capture_preserves_reactive_gain_workarea_and_cancellation() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-audio-only-live-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
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
                "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let source = r#"import {useTicker} from '@ad3-editing/jsx';
        export default ()=>{ const t=useTicker(); return <stage><scene id="s" active width={32} height={16} workarea={[0.2,1.2]}>
        <audio id="a" src="tone.wav" volume={t.time()<0.7?0:-12} end={2}/>
        </scene></stage>; };"#;
    fs::write(folder.join("index.tsx"), source).unwrap();
    let package = r#"{"diffusion":{"export":{"s":{"video":{"enabled":false}}}}}"#;
    fs::write(folder.join("package.json"), package).unwrap();
    let document = SourceDocument::open(&folder).unwrap();
    let output = folder.join("live.mp4");
    let report = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &output,
        30.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert!((report.workarea_duration - 1.0).abs() < 1e-9);
    let decoded = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1"]),
    )
    .unwrap();
    let samples: Vec<_> = decoded
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes) as f64)
        .collect();
    let rms = |from: usize, to: usize| {
        (samples[from..to]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f64>()
            / (to - from) as f64)
            .sqrt()
    };
    let ratio = rms(4800, 14400) / rms(33600, 43200);
    assert!(
        (3.4..4.6).contains(&ratio),
        "Trimmed output must retain the 0 to -12 dB change halfway through: {ratio}"
    );
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancellation = cancelled.clone();
    let incomplete = folder.join("cancelled.mp4");
    let result = ProjectRenderer::export(
        &document,
        &folder,
        tools,
        &incomplete,
        30.0,
        Some(cancelled),
        |frame, _| {
            if frame == 5 {
                cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
    );
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(!incomplete.exists());
    assert_eq!(
        fs::read_dir(folder.join(".diffusion/native-render"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_to_string(folder.join("index.tsx")).unwrap(),
        source
    );
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        package
    );
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn video_disabled_exports_only_audio_and_skips_graphics_settings() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-audio-only-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
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
                "sine=frequency=440:duration=0.5",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,0.5]}><audio src="tone.wav"/><rect width={32} height={16} unportedRenderProp="ignored"/></scene></stage>;"#;
    let document = SourceDocument::parse(folder.join("index.tsx"), source.into()).unwrap();
    let package = r#"{"diffusion":{"export":{"s":{"video":{"enabled":false,"codec":"unknown","bitrate":0,"resolution":0}}}}}"#;
    fs::write(folder.join("package.json"), package).unwrap();
    for (extension, codec) in [
        ("mp4", "aac"),
        ("mov", "aac"),
        ("webm", "opus"),
        ("ogg", "opus"),
        ("webm", "vorbis"),
        ("ogg", "vorbis"),
        ("ogg", "flac"),
    ] {
        let settings = if matches!(codec, "vorbis" | "flac") {
            let mut settings: serde_json::Value = serde_json::from_str(package).unwrap();
            settings["diffusion"]["export"]["s"]["audio"] = serde_json::json!({"codec":codec});
            if codec == "flac" {
                settings["diffusion"]["export"]["s"]["audio"]["bitrate"] =
                    serde_json::json!("unused");
            }
            settings.to_string()
        } else {
            package.into()
        };
        fs::write(folder.join("package.json"), &settings).unwrap();
        let output = folder.join(format!("audio-only-{codec}.{extension}"));
        let mut progress = Vec::new();
        let report = ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |frame, total| progress.push((frame, total)),
        )
        .unwrap();
        assert_eq!((report.width, report.height, report.frames), (0, 0, 0));
        assert_eq!(progress.last(), Some(&(15, 15)));
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let streams = value["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0]["codec_type"], "audio");
        assert_eq!(streams[0]["codec_name"], codec);
        let decoded = checked_output(
            command(&tools.ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&output)
                .args(["-vn", "-ac", "1", "-f", "f32le", "pipe:1"]),
        )
        .unwrap();
        let samples = decoded.stdout.as_chunks::<4>().0;
        let rms = (samples
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes) as f64)
            .map(|sample| sample * sample)
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        assert!(
            rms > 0.02,
            "Audio-only output must retain the audible source: {rms}"
        );
        assert_eq!(
            fs::read_to_string(folder.join("package.json")).unwrap(),
            settings
        );
    }
    fs::write(folder.join("package.json"), package).unwrap();
    let empty = folder.join("disabled.mp4");
    assert!(
        ProjectRenderer::export_with_audio(
            &document,
            &folder,
            tools,
            &empty,
            30.0,
            false,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("Enable video or audio")
    );
    assert!(!empty.exists());
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        package
    );
    assert_eq!(document.source, source);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn absent_export_resolution_defaults_to_1080p_with_or_without_package() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-default-resolution-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let document = SourceDocument::parse(folder.join("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={32} fill="red" workarea={[0,1/30]}/></stage>;"#.into()).unwrap();
    let tools = Tools::default();
    for with_package in [false, true] {
        if with_package {
            fs::write(folder.join("package.json"), "{}").unwrap();
        }
        let output = folder.join(format!("default-{with_package}.mp4"));
        let report = ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(
            (report.width, report.height, report.frames),
            (1080, 1080, 1)
        );
        let probe = tools.probe(&output).unwrap();
        assert_eq!((probe.width, probe.height), (1080, 1080));
        // x264 embeds the requested rate-control settings in its SEI message.
        // Measured bitrate on a static one-frame clip cannot prove the target.
        let stream = checked_output(
            command(&tools.ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&output)
                .args([
                    "-map",
                    "0:v:0",
                    "-c:v",
                    "copy",
                    "-bsf:v",
                    "h264_mp4toannexb",
                    "-f",
                    "h264",
                    "pipe:1",
                ]),
        )
        .unwrap();
        let settings = String::from_utf8_lossy(&stream.stdout);
        assert!(
            settings.contains("bitrate=10000"),
            "Missing default 10 Mbit/s encoder target"
        );
        assert!(
            settings.contains("rc=abr"),
            "Default export must use bitrate rate control"
        );
    }
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn saved_audio_bitrate_changes_encoded_stream_and_disabled_audio_ignores_bad_setting() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-audio-bitrate-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let tools = Tools::default();
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
                "anoisesrc=duration=2:sample_rate=48000:seed=17",
                "-ac",
                "2",
            ])
            .arg(folder.join("noise.wav")),
    )
    .unwrap();
    let source = r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,2]}><audio src="noise.wav"/><rect width={32} height={16} fill="red"/></scene></stage>;"#;
    let document = SourceDocument::parse(folder.join("index.tsx"), source.into()).unwrap();
    let mut measured = Vec::new();
    for (bitrate, extension, sample_rate) in [
        (32000, "mp4", 44100),
        (192000, "mp4", 44100),
        (128000, "mp4", 32000),
        (64000, "webm", 24000),
        (96000, "mp4", 24000),
        (128000, "mov", 48000),
        (128000, "mp4", 48000),
        (128000, "mp4", 96000),
        (128000, "webm", 48000),
    ] {
        let channels = if sample_rate == 44100 { 2 } else { 1 };
        let audio_codec = match sample_rate {
            96000 => "flac",
            24000 => "opus",
            48000 if extension == "webm" => "vorbis",
            48000 => "mp3",
            _ => "aac",
        };
        let saved_bitrate = if audio_codec == "flac" {
            serde_json::json!("unused")
        } else {
            serde_json::json!(bitrate)
        };
        let package =
            serde_json::json!({"diffusion":{"export":{"s":{"video":{"resolution":16},"audio":{"bitrate":saved_bitrate,"sampleRate":sample_rate,"numberOfChannels":channels,"codec":audio_codec}}}}})
                .to_string();
        fs::write(folder.join("package.json"), &package).unwrap();
        let output = folder.join(format!("audio-{bitrate}-{sample_rate}.{extension}"));
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let audio = value["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .unwrap();
        assert_eq!(audio["codec_name"], audio_codec);
        assert_eq!(audio["channels"], channels);
        if audio_codec == "flac" {
            assert_eq!(audio["bits_per_raw_sample"], "32");
        }
        if audio_codec != "opus" {
            assert_eq!(audio["sample_rate"], sample_rate.to_string());
            if sample_rate == 44100 {
                measured.push(audio["bit_rate"].as_str().unwrap().parse::<u64>().unwrap());
            }
        } else {
            // Opus uses a 48 kHz decoder clock even with a 24 kHz encoder input.
            assert_eq!(audio["sample_rate"], "48000");
        }
        assert_eq!(
            fs::read_to_string(folder.join("package.json")).unwrap(),
            package
        );
    }
    assert!(
        measured[1] > measured[0] * 3,
        "Requested audio rates should change actual encoded bitrate: {measured:?}"
    );
    fs::write(
        folder.join("package.json"),
        r#"{"diffusion":{"export":{"s":{"audio":{"bitrate":0}}}}}"#,
    )
    .unwrap();
    let output = folder.join("invalid.mp4");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("audio bitrate")
    );
    assert!(!output.exists());
    fs::write(
        folder.join("package.json"),
        r#"{"diffusion":{"export":{"s":{"audio":{"sampleRate":44100}}}}}"#,
    )
    .unwrap();
    let unsupported = folder.join("unsupported.webm");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &unsupported,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("sampleRate")
    );
    assert!(!unsupported.exists());
    fs::write(
        folder.join("package.json"),
        r#"{"diffusion":{"export":{"s":{"audio":{"numberOfChannels":6}}}}}"#,
    )
    .unwrap();
    let surround = folder.join("surround.mp4");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &surround,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("numberOfChannels")
    );
    assert!(!surround.exists());
    fs::write(
        folder.join("package.json"),
        r#"{"diffusion":{"export":{"s":{"audio":{"codec":"opus"}}}}}"#,
    )
    .unwrap();
    let opus_mov = folder.join("opus.mov");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &opus_mov,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("codec")
    );
    assert!(!opus_mov.exists());
    let silent = folder.join("silent.mp4");
    export_height(&folder, "s", 16);
    ProjectRenderer::export_with_audio(
        &document,
        &folder,
        tools,
        &silent,
        30.0,
        false,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(document.source, source);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn saved_video_bitrate_exports_both_codecs_and_invalid_setting_creates_no_output() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-bitrate-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    let package = r#"{"diffusion":{"export":{"s":{"video":{"bitrate":500000,"resolution":24}}}}}"#;
    fs::write(folder.join("package.json"), package).unwrap();
    let document = SourceDocument::parse(folder.join("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,0.2]}><rect width={32} height={16} fill="red"/></scene></stage>;"#.into()).unwrap();
    let tools = Tools::default();
    for (extension, codec) in [("mov", "h264"), ("webm", "vp9")] {
        let output = folder.join(format!("export.{extension}"));
        let report = ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(report.frames, 6);
        assert_eq!((report.width, report.height), (48, 24));
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"][0]["codec_name"], codec);
        assert_eq!(value["streams"][0]["width"], 48);
        assert_eq!(value["streams"][0]["height"], 24);
    }
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        package
    );
    let vp8_package = package.replace("\"resolution\":24", "\"resolution\":24,\"codec\":\"vp8\"");
    let av1_package = package.replace("\"resolution\":24", "\"resolution\":24,\"codec\":\"av1\"");
    fs::write(folder.join("package.json"), &av1_package).unwrap();
    for extension in ["mp4", "webm"] {
        let output = folder.join(format!("av1.{extension}"));
        let report = ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!((report.width, report.height, report.frames), (48, 24, 6));
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"][0]["codec_name"], "av1");
        assert_eq!(value["streams"][0]["width"], 48);
        assert_eq!(value["streams"][0]["height"], 24);
    }
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        av1_package
    );
    let av1_mov = folder.join("av1.mov");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &av1_mov,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("codec")
    );
    assert!(!av1_mov.exists());
    let hevc_package = package.replace("\"resolution\":24", "\"resolution\":24,\"codec\":\"hevc\"");
    fs::write(folder.join("package.json"), &hevc_package).unwrap();
    for extension in ["mp4", "mov"] {
        let output = folder.join(format!("hevc.{extension}"));
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(value["streams"][0]["codec_name"], "hevc");
        assert_eq!(value["streams"][0]["codec_tag_string"], "hvc1");
        assert_eq!(value["streams"][0]["width"], 48);
        assert_eq!(value["streams"][0]["height"], 24);
        assert_eq!(value["streams"][0]["nb_frames"], "6");
    }
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        hevc_package
    );
    fs::write(folder.join("package.json"), &vp8_package).unwrap();
    let vp8 = folder.join("vp8.webm");
    ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &vp8,
        30.0,
        None,
        |_, _| {},
    )
    .unwrap();
    let probe = checked_output(
        command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(&vp8),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(value["streams"][0]["codec_name"], "vp8");
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        vp8_package
    );
    let incompatible = folder.join("incompatible.mp4");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &incompatible,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("codec")
    );
    assert!(!incompatible.exists());
    fs::write(folder.join("package.json"), package.replace("500000", "0")).unwrap();
    let output = folder.join("invalid.mp4");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("bitrate")
    );
    assert!(!output.exists());
    fs::write(
        folder.join("package.json"),
        package.replace("\"resolution\":24", "\"resolution\":0"),
    )
    .unwrap();
    let invalid_size = folder.join("invalid-resolution.mp4");
    assert!(
        ProjectRenderer::export(
            &document,
            &folder,
            tools,
            &invalid_size,
            30.0,
            None,
            |_, _| {}
        )
        .unwrap_err()
        .contains("resolution")
    );
    assert!(!invalid_size.exists());
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn synced_fractional_audio_origin_exports_silence_before_recording() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-fractional-sync-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    export_height(&folder, "s", 24);
    let mut state = 17u32;
    let levels = (0..1500)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            0.05 + 0.7 * (state >> 8) as f64 / 16777216.0
        })
        .collect::<Vec<_>>();
    for (name, bins) in [
        ("reference.wav", &levels[..]),
        ("audio.wav", &levels[201..1201]),
    ] {
        let mut data = Vec::new();
        for level in bins {
            for sample in 0..96 {
                let value = (32767.0
                    * level
                    * (std::f64::consts::TAU * 1000.0 * sample as f64 / 48000.0).sin())
                    as i16;
                data.extend(value.to_le_bytes());
                data.extend((-value).to_le_bytes());
            }
        }
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + data.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(48000u32.to_le_bytes());
        wav.extend(192000u32.to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((data.len() as u32).to_le_bytes());
        wav.extend(data);
        fs::write(folder.join(name), wav).unwrap();
    }
    let doc = SourceDocument::parse(folder.join("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={24} workarea={[0,3]}><audio id="audio" src="audio.wav" syncTo="reference"/><audio id="reference" src="reference.wav" muted/><rect id="r" width={16} height={16} fill="red"/></scene></stage>;"#.into()).unwrap();
    let tools = Tools::default();
    let mut renderer = ProjectRenderer::new(&doc, &folder, tools.clone(), false).unwrap();
    let span = renderer.timeline.get("audio").unwrap();
    assert!((span.origin - 0.402).abs() < 0.002);
    assert!(span.start < span.origin);
    renderer.frame_plan("s", 1.0, 30.0, false).unwrap();
    let output = folder.join("synced.mov");
    ProjectRenderer::export_with_audio(
        &doc,
        &folder,
        tools.clone(),
        &output,
        30.0,
        true,
        None,
        |_, _| {},
    )
    .unwrap();
    // Decode stereo channel 0 instead of downmixing the deliberately opposing channels.
    let pcm = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args([
                "-map",
                "0:a:0",
                "-af",
                "pan=mono|c0=c0",
                "-ar",
                "48000",
                "-f",
                "f32le",
                "pipe:1",
            ]),
    )
    .unwrap();
    let channel = pcm
        .stdout
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert!(channel.len() >= 3 * 48000);
    let rms = |slice: &[f32]| {
        (slice
            .iter()
            .map(|value| (*value as f64).powi(2))
            .sum::<f64>()
            / slice.len() as f64)
            .sqrt()
    };
    assert!(rms(&channel[4800..14400]) < 1e-5);
    assert!(rms(&channel[24000..48000]) > 0.01);
    assert!(doc.root.find("audio").unwrap().props.contains_key("syncTo"));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn runtime_skew_reaches_exported_pixels_without_changing_source() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-runtime-skew-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    export_height(&folder, "s", 24);
    let doc = SourceDocument::parse(folder.join("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={24} workarea={[0,0.1]}><rect id="r" x={8} y={8} width={12} height={8} fill="red"/></scene></stage>;"#.into()).unwrap();
    let source = doc.source.clone();
    let tools = Tools::default();
    let mut decoded = Vec::new();
    for (name, skew) in [
        ("plain.mov", std::collections::BTreeMap::new()),
        (
            "skew.mov",
            std::collections::BTreeMap::from([("r".into(), [45.0, 0.0])]),
        ),
    ] {
        let output = folder.join(name);
        ProjectRenderer::export_with_runtime(
            &doc,
            &folder,
            tools.clone(),
            &output,
            30.0,
            false,
            skew,
            None,
            |_, _| {},
        )
        .unwrap();
        let frame = checked_output(
            command(&tools.ffmpeg)
                .args(["-v", "error", "-i"])
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
        assert_eq!(frame.stdout.len(), 32 * 24 * 4);
        decoded.push(frame.stdout);
    }
    let difference: u64 = decoded[0]
        .iter()
        .zip(&decoded[1])
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    assert!(difference > 2000, "skew did not change encoded pixels");
    assert_eq!(doc.source, source);
    let resolved = folder.canonicalize().unwrap();
    assert_eq!(
        resolved.parent().unwrap(),
        std::env::temp_dir().canonicalize().unwrap()
    );
    assert!(
        folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-runtime-skew-")
    );
    fs::remove_dir_all(resolved).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn mov_and_webm_exports_use_matching_containers_and_codecs() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-rust-mov-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&folder).unwrap();
    export_height(&folder, "s", 16);
    let document = SourceDocument::parse(folder.join("index.tsx"), r#"export default <stage><scene id="s" active width={32} height={16} workarea={[0,0.2]}><rect id="r" width={32} height={16} fill="red"/></scene></stage>;"#.into()).unwrap();
    let output = folder.join("export.mov");
    let tools = Tools::default();
    let report = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &output,
        30.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(report.frames, 6);
    let probe = checked_output(
        command(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-show_format",
                "-show_streams",
                "-of",
                "json",
            ])
            .arg(&output),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(
        value["format"]["tags"]["major_brand"]
            .as_str()
            .unwrap()
            .trim(),
        "qt"
    );
    assert_eq!(value["streams"][0]["codec_name"], "h264");
    assert_eq!(value["streams"][0]["nb_frames"], "6");
    assert_eq!(value["streams"][0]["width"], 32);
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
                "sine=frequency=440:duration=0.2",
            ])
            .arg(folder.join("tone.wav")),
    )
    .unwrap();
    let with_audio = SourceDocument::parse(
        folder.join("index.tsx"),
        document.source.replace(
            "</scene>",
            "<audio id=\"a\" src=\"tone.wav\" end={0.2}/></scene>",
        ),
    )
    .unwrap();
    let output = folder.join("audio.mov");
    ProjectRenderer::export(
        &with_audio,
        &folder,
        tools.clone(),
        &output,
        30.0,
        None,
        |_, _| {},
    )
    .unwrap();
    let probe = checked_output(
        command(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-show_format",
                "-show_streams",
                "-of",
                "json",
            ])
            .arg(&output),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(
        value["format"]["tags"]["major_brand"]
            .as_str()
            .unwrap()
            .trim(),
        "qt"
    );
    assert!(
        value["streams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stream| stream["codec_name"] == "aac")
    );
    for (source, name) in [(&document, "silent.webm"), (&with_audio, "audio.webm")] {
        let output = folder.join(name);
        ProjectRenderer::export(
            source,
            &folder,
            tools.clone(),
            &output,
            30.0,
            None,
            |_, _| {},
        )
        .unwrap();
        let probe = checked_output(
            command(&tools.ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_format",
                    "-show_streams",
                    "-of",
                    "json",
                ])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert!(
            value["format"]["format_name"]
                .as_str()
                .unwrap()
                .contains("webm")
        );
        let streams = value["streams"].as_array().unwrap();
        assert!(streams.iter().any(|stream| stream["codec_name"] == "vp9"));
        if name == "audio.webm" {
            assert!(streams.iter().any(|stream| stream["codec_name"] == "opus"));
        }
    }
    let library = MediaLibrary::load(&with_audio.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&with_audio.root, &library.durations).unwrap();
    let output = folder.join("filter.webm");
    RenderPlan::build(&with_audio.root, &timeline, &library, "s", 30.0, None)
        .unwrap()
        .export(&tools, &folder, &output)
        .unwrap();
    let probe = checked_output(
        command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(output),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = value["streams"].as_array().unwrap();
    assert!(streams.iter().any(|stream| stream["codec_name"] == "vp9"));
    assert!(streams.iter().any(|stream| stream["codec_name"] == "opus"));
    let odd = SourceDocument::parse(
        folder.join("odd.tsx"),
        document
            .source
            .replace("width={32}", "width={33}")
            .replace("height={16}", "height={17}"),
    )
    .unwrap();
    export_height(&folder, "s", 17);
    let output = folder.join("odd.webm");
    ProjectRenderer::export(&odd, &folder, tools.clone(), &output, 30.0, None, |_, _| {}).unwrap();
    let probe = checked_output(
        command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(&output),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(value["streams"][0]["width"], 34);
    assert_eq!(value["streams"][0]["height"], 18);
    assert_eq!(value["streams"][0]["codec_name"], "vp9");
    let library = MediaLibrary::load(&odd.root, &folder, &tools).unwrap();
    let timeline = Timeline::resolve(&odd.root, &library.durations).unwrap();
    let plan = RenderPlan::build(&odd.root, &timeline, &library, "s", 30.0, None).unwrap();
    let output = folder.join("odd-filter.webm");
    plan.export(&tools, &folder, &output).unwrap();
    let probe = checked_output(
        command(&tools.ffprobe)
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(&output),
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(value["streams"][0]["width"], 33);
    assert_eq!(value["streams"][0]["height"], 17);
    assert!(
        plan.export(&tools, &folder, &folder.join("odd-filter.mp4"))
            .unwrap_err()
            .contains("even dimensions")
    );
    let mut resized = plan;
    resized.resize_output(17).unwrap();
    assert_eq!((resized.width, resized.height), (34, 18));
    let normalized = folder.join("normalized-filter.mp4");
    resized.export(&tools, &folder, &normalized).unwrap();
    let probe = tools.probe(&normalized).unwrap();
    assert_eq!((probe.width, probe.height), (34, 18));
    export_height(&folder, "s", 16);
    for extension in ["mp4", "mov", "webm"] {
        let output = folder.join(format!("disabled-audio.{extension}"));
        ProjectRenderer::export_with_audio(
            &with_audio,
            &folder,
            tools.clone(),
            &output,
            30.0,
            false,
            None,
            |_, _| {},
        )
        .unwrap();
        let probe = checked_output(
            command(&tools.ffprobe)
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&output),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let streams = value["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0]["codec_type"], "video");
        assert!(!with_audio.root.find("a").unwrap().flag("muted"));
    }
    let resolved = fs::canonicalize(&folder).unwrap();
    assert!(resolved.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        resolved
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("ad3-rust-mov-")
    );
    fs::remove_dir_all(resolved).unwrap();
}

#[test]
#[ignore = "requires installed FFmpeg and FFprobe; run with --ignored"]
fn native_video_textures_keep_source_clock_alpha_transform_and_export_frames() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder = std::env::temp_dir().join(format!(
        "ad3-rust-texture-video-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&folder).unwrap();
    export_height(&folder, "s", 48);
    for index in 0..8u8 {
        image::RgbaImage::from_pixel(16, 8, image::Rgba([index * 30, 40, 180, 128]))
            .save(folder.join(format!("frame{index:02}.png")))
            .unwrap();
    }
    let tools = Tools::default();
    checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-framerate", "4", "-i"])
            .arg(folder.join("frame%02d.png"))
            .args(["-c:v", "ffv1", "-pix_fmt", "bgra"])
            .arg(folder.join("source.mkv")),
    )
    .unwrap();
    // A one-frame Matroska track without DefaultDuration has avg_frame_rate=0/0.
    // Preserve every packet, replace only that optional EBML metadata element ID.
    checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(folder.join("source.mkv"))
            .args(["-frames:v", "1", "-c:v", "ffv1"])
            .arg(folder.join("one-frame.mkv")),
    )
    .unwrap();
    let mut no_rate = fs::read(folder.join("one-frame.mkv")).unwrap();
    let tag = no_rate
        .windows(3)
        .position(|bytes| bytes == [0x23, 0xe3, 0x83])
        .unwrap();
    no_rate[tag + 2] = 0x82;
    fs::write(folder.join("no-rate.mkv"), no_rate).unwrap();
    let raw = checked_output(
        command(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=avg_frame_rate",
                "-of",
                "json",
            ])
            .arg(folder.join("no-rate.mkv")),
    )
    .unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(raw["streams"][0]["avg_frame_rate"], "0/0");
    let recovered_rate = tools.probe(&folder.join("no-rate.mkv")).unwrap();
    assert_eq!(recovered_rate.frame_rate, 4.0);
    let no_rate_doc=SourceDocument::parse(folder.join("no-rate.tsx"),r#"export default <stage><scene id="s" active width={16} height={8} workarea={[0,0.25]}><video src="no-rate.mkv" width={16} height={8}/></scene></stage>"#.into()).unwrap();
    let mut no_rate_renderer =
        ProjectRenderer::new(&no_rate_doc, &folder, tools.clone(), true).unwrap();
    let pixels = no_rate_renderer
        .frame_plan("s", 0.0, 4.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(&pixels[..4], &[0, 40, 179, 128]);
    let mask_doc=SourceDocument::parse(folder.join("masked.tsx"),r#"export default <stage><scene id="s" active width={16} height={8} workarea={[0,0.25]}><video src="no-rate.mkv" width={16} height={8}><rect mask x={4} width={4} height={8} end={0.125}/></video></scene></stage>"#.into()).unwrap();
    let mut mask_renderer = ProjectRenderer::new(&mask_doc, &folder, tools.clone(), true).unwrap();
    let masked = mask_renderer
        .frame_plan("s", 0.0, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(&masked[..4], &[0, 0, 0, 0]);
    assert_eq!(&masked[4 * 4..5 * 4], &pixels[4 * 4..5 * 4]);
    assert_eq!(&masked[8 * 4..9 * 4], &[0, 0, 0, 0]);
    let released = mask_renderer
        .frame_plan("s", 0.125, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(released, pixels);
    // Container start timestamps must not lengthen the source's content clock.
    checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(folder.join("source.mkv"))
            .args(["-c:v", "copy", "-output_ts_offset", "2"])
            .arg(folder.join("shifted.mkv")),
    )
    .unwrap();
    let shifted = tools.probe(&folder.join("shifted.mkv")).unwrap();
    assert_eq!(shifted.start_time, 2.0);
    assert_eq!(shifted.duration, Some(2.0));
    let texture_store = ad3_editor::texture::TextureStore::new(tools.clone());
    let shifted_time = ad3_editor::texture::source_time(0.5, 0.0, 1.0, 8.0, &shifted).unwrap();
    assert_eq!(shifted_time, 2.5);
    let shifted_pixels = texture_store
        .frame(
            &ad3_editor::texture::TextureRequest {
                path: folder.join("shifted.mkv"),
                probe: shifted,
                frame: TextureFrame::Video(shifted_time),
            },
            None,
        )
        .unwrap();
    assert_eq!(&shifted_pixels.data()[..4], &[30, 20, 90, 128]);
    // Pixel aspect and display rotation normalize the decoder bitmap before fit.
    checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-nostdin", "-n", "-i"])
            .arg(folder.join("source.mkv"))
            .args([
                "-vf", "setsar=2", "-c:v", "libx264", "-pix_fmt", "yuv444p", "-crf", "0",
            ])
            .arg(folder.join("sar.mp4")),
    )
    .unwrap();
    let sar = tools.probe(&folder.join("sar.mp4")).unwrap();
    assert_eq!(sar.display_dimensions().unwrap(), (32, 8));
    checked_output(
        command(&tools.ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-n",
                "-display_rotation",
                "90",
                "-i",
            ])
            .arg(folder.join("sar.mp4"))
            .args(["-c:v", "copy"])
            .arg(folder.join("rotated.mp4")),
    )
    .unwrap();
    let rotated = tools.probe(&folder.join("rotated.mp4")).unwrap();
    assert_eq!(rotated.display_dimensions().unwrap(), (8, 32));
    let rotated_pixels = texture_store
        .frame(
            &ad3_editor::texture::TextureRequest {
                path: folder.join("rotated.mp4"),
                probe: rotated,
                frame: TextureFrame::Video(0.5),
            },
            None,
        )
        .unwrap();
    assert_eq!((rotated_pixels.width(), rotated_pixels.height()), (8, 32));
    assert!((58..=62).contains(&rotated_pixels.data()[0]));
    let fit_doc=SourceDocument::parse(folder.join("rotated.tsx"),r#"export default <stage><scene id="s" active width={16} height={16} workarea={[0,1]}><video src="rotated.mp4" width={16} height={16} objectFit="contain"/></scene></stage>"#.into()).unwrap();
    let mut fit_renderer = ProjectRenderer::new(&fit_doc, &folder, tools.clone(), true).unwrap();
    let fit_pixels = fit_renderer
        .frame_plan("s", 0.5, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(fit_pixels[(8 * 16 + 3) * 4 + 3], 0);
    assert_eq!(fit_pixels[(8 * 16 + 7) * 4 + 3], 255);
    let document=SourceDocument::parse(folder.join("index.tsx"),r#"export default <stage>
      <scene id="s" active width={64} height={48} workarea={[0,0.5]}>
        <video id="v" src="source.mkv" x={16} y={12} width={16} height={8} rotation={90} cornerRadius={2}
          sourceIn={0.5} sourceOut={1.5} playbackRate={2} opacity={0.5} objectFit="fill"/>
      </scene></stage>"#.into()).unwrap();
    let mut renderer = ProjectRenderer::new(&document, &folder, tools.clone(), true).unwrap();
    let plan = renderer.frame_plan("s", 0.0, 8.0, false).unwrap();
    assert!(plan.is_native());
    let first = plan.frame(&tools, &folder).unwrap();
    let middle = renderer
        .frame_plan("s", 0.25, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    let reverse = renderer
        .frame_plan("s", 0.0, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(first, reverse);
    let at = (12 * 64 + 24) * 4;
    assert!(
        (58..=62).contains(&first[at]) && (118..=122).contains(&middle[at]),
        "{:?} {:?}",
        &first[at..at + 4],
        &middle[at..at + 4]
    );
    assert_eq!(first[at + 3], 64);
    assert_eq!(first[(12 * 64 + 18) * 4 + 3], 0);
    let end = renderer
        .frame_plan("s", 0.5, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert!(end.iter().all(|n| *n == 0));
    let output = folder.join("native.mp4");
    let report = ProjectRenderer::export(
        &document,
        &folder,
        tools.clone(),
        &output,
        8.0,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(report.frames, 4);
    assert_eq!((report.width, report.height), (64, 48));
    let decoded = checked_output(
        command(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]),
    )
    .unwrap()
    .stdout;
    let stride = 64 * 48 * 4;
    assert_eq!(decoded.len(), stride * 4);
    assert!(
        decoded[at] < 25 && decoded[3 * stride + at] > decoded[at] + 12,
        "{} -> {}",
        decoded[at],
        decoded[3 * stride + at]
    );
    assert!(decoded[(12 * 64 + 18) * 4] < 10);
    let pending = renderer.frame_plan("s", 0.0, 8.0, false).unwrap();
    // Same coded size, different stream timestamp: a stale plan must not reuse
    // its old clock against newly decoded source bytes, even on a cache hit.
    fs::copy(folder.join("shifted.mkv"), folder.join("source.mkv")).unwrap();
    assert!(
        pending
            .frame(&tools, &folder)
            .err()
            .unwrap()
            .contains("changed since probing")
    );
    let fresh = renderer
        .frame_plan("s", 0.0, 8.0, false)
        .unwrap()
        .frame(&tools, &folder)
        .unwrap();
    assert_eq!(fresh, first);
    assert_eq!(renderer.library.probes["v"].start_time, 2.0);
    // Simulate a stream frame exceeding its initial probe dimensions: enforce
    // the expected raw byte count at the pipe reader, before pixel allocation.
    let mut mismatched = tools.probe(&folder.join("source.mkv")).unwrap();
    mismatched.width = 2;
    mismatched.height = 2;
    let request = ad3_editor::texture::TextureRequest {
        path: folder.join("source.mkv"),
        probe: mismatched,
        frame: TextureFrame::Video(2.0),
    };
    let error = ad3_editor::texture::TextureStore::new(tools.clone())
        .frame(&request, None)
        .err()
        .unwrap();
    assert!(error.contains("exceeded its limit"), "{error}");
    let mut too_large = request.clone();
    too_large.probe.width = 4096;
    too_large.probe.height = 4097;
    assert!(
        ad3_editor::texture::TextureStore::new(tools.clone())
            .frame(&too_large, None)
            .err()
            .unwrap()
            .contains("16-megapixel")
    );
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-texture-video-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

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
    export_height(&folder, "s", 32);
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
    export_height(&folder, "s", 16);
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
    export_height(&folder, "s", 90);
    export_height(&folder, "main", 90);
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
    export_height(&folder, "s", 90);
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
        import {useTicker} from '@ad3-editing/jsx';
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

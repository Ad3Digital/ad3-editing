// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    Result,
    media::{MediaLibrary, Tools},
    project::ProjectRenderer,
    source::SourceDocument,
    timing::{Timeline, parse_time},
};
use serde_json::{Value, json};
use std::{env, path::Path};

const HELP: &str = "AD3 Editing — native Rust CLI (migration in progress)

  dapi-rust inspect <project>                   Read the JSX document as JSON
  dapi-rust controls <project>                  List @inspect variables
  dapi-rust control <project> <file> <name> <json>  Set an @inspect initializer
  dapi-rust context <project>                   Resolve sources and timeline
  dapi-rust check <project>                     Check timing and sequence overlaps
  dapi-rust set <project> <id> <prop> <json>     Edit one JSX property
  dapi-rust unset <project> <id> <prop>          Remove one JSX property
  dapi-rust split <project> <id> <scene-time>   Split without losing source alignment
  dapi-rust cut <project> <scene> <time> [ids...]  Cut visible clips at the playhead
  dapi-rust close-gap <project> <scene> <sequence> <in> <out>  Close an empty interval
  dapi-rust restack <project> <front|back> <ids...>  Change sibling draw order
  dapi-rust duplicate <project> <ids...>        Duplicate selected subtrees
  dapi-rust group <project> <ids...>            Wrap selected siblings in a group
  dapi-rust sequence <project> <ids...>         Wrap siblings and resolve overlaps
  dapi-rust scene <project> <time> <ids...>     Frame selected siblings in a scene
  dapi-rust nudge <project> <time> <dx> <dy> <ids...>  Move sampled positions
  dapi-rust keyframe <project> <id> <prop> <time> <json>  Toggle a keyframe
  dapi-rust remove-track <project> <id> <prop>  Remove a property's keyframe track
  dapi-rust trim <project> <id> <in> <out>       Trim using scene times
  dapi-rust ripple <project> <scene> <left|right> <time> [ids...]  Ripple trim at playhead
  dapi-rust remove <project> <id>              Remove a source element
  dapi-rust export <project> <output> [fps]     Export using saved scene preferences
  dapi-rust capture <project> <scene> <time> <output.png>  Capture at workarea-relative time
  dapi-rust media probe <file>                 Read FFprobe metadata
  dapi-rust media sync <audio> <reference>     Measure recording alignment locally
  dapi-rust media waveform <file> [options]    Render waveform, rulers and silence ranges
  dapi-rust media filmstrip <file> [options]   Render video thumbnails and time rulers
  dapi-rust media grab <file> [options]        Render labelled contact sheets
  dapi-rust fonts [options]                   List local fonts without a project

Time accepts seconds, 45f (at 30 fps), MM:SS or HH:MM:SS.
Projects use their existing package.json main or index.tsx.
TSX modules and the native signal/ticker host are supported; migration is ongoing.
FFmpeg/FFprobe use PATH or AD3_FFMPEG_PATH / AD3_FFPROBE_PATH.
";

fn run(args: &[String]) -> Result<Value> {
    let tools = Tools::default();
    if let [group, action, rest @ ..] = args
        && group == "media"
    {
        return match (action.as_str(), rest) {
            ("filmstrip" | "film", args) => {
                let path = ad3_editor::filmstrip::command(args, &tools, None)?;
                Ok(json!({"path": path}))
            }
            ("sync", [audio, reference]) => {
                let result = ad3_editor::audio_sync::align_files(
                    &tools,
                    Path::new(audio),
                    Path::new(reference),
                    None,
                )?;
                Ok(json!({"offsetSeconds": result.offset_seconds, "confidence": result.confidence}))
            }
            ("probe", [path]) => {
                serde_json::to_value(tools.probe(Path::new(path))?).map_err(|e| e.to_string())
            }
            ("waveform", args) => {
                let legacy;
                let args = if let [path, output] = args
                    && !output.starts_with('-')
                {
                    legacy = vec![path.clone(), "--output".into(), output.clone()];
                    legacy.as_slice()
                } else {
                    args
                };
                let (path, silences) = ad3_editor::waveform::command(args, &tools, None)?;
                Ok(json!({"path": path, "silences": silences}))
            }
            _ => Err(HELP.into()),
        };
    }
    let [action, path, rest @ ..] = args else {
        return Err(HELP.into());
    };
    let document = SourceDocument::open(Path::new(path))?;
    if action == "inspect" && rest.is_empty() {
        return Ok(json!({"entry": document.path, "document": document.root,
            "sourceEditable": document.module_sources.is_empty(),
            "modules": document.module_sources.keys().collect::<Vec<_>>() }));
    }
    if action == "controls" && rest.is_empty() {
        return Ok(json!({"controls": document.controls()?}));
    }
    if action == "duplicate" {
        if rest.is_empty() {
            return Err("Duplicate requires at least one node id".into());
        }
        let (next, copies) = document.duplicate(rest)?;
        if !copies.is_empty() {
            next.save_over(&document)?;
        }
        return Ok(json!({"ok": true, "entry": next.path, "copies": copies}));
    }
    if action == "group" {
        if rest.is_empty() {
            return Err("Group requires at least one node id".into());
        }
        let (next, group) = document.group(rest)?;
        if group.is_some() {
            next.save_over(&document)?;
        }
        return Ok(json!({"ok": true, "entry": next.path, "group": group}));
    }
    let next = match (action.as_str(), rest) {
        ("control", [file, name, value]) => Some(document.set_control(
            Path::new(file),
            name,
            serde_json::from_str(value).map_err(|e| e.to_string())?,
        )?),
        ("set", [id, name, value]) => Some(
            document.set(
                id,
                name,
                Some(
                    serde_json::from_str(value)
                        .map_err(|e| format!("Property value must be JSON: {e}"))?,
                ),
            )?,
        ),
        ("unset", [id, name]) => Some(document.set(id, name, None)?),
        ("remove-track", [id, name]) => Some(document.remove_keyframe_track(id, name)?),
        ("restack", [side, ids @ ..]) => {
            if ids.is_empty() || !matches!(side.as_str(), "front" | "back") {
                return Err("Restack requires front/back and at least one node id".into());
            }
            Some(document.restack(ids, side == "front")?)
        }
        ("remove", [id]) => Some(document.remove(id)?),
        _ => None,
    };
    if let Some(next) = next {
        next.save_over(&document)?;
        return Ok(json!({"ok": true, "entry": next.path}));
    }
    let project = if Path::new(path).is_dir() {
        Path::new(path)
    } else {
        document.path.parent().ok_or("Missing project directory")?
    };
    // Export resolves timing only in its output-rate world. A 30 fps preflight
    // would reject sub-frame workareas that are valid at a higher output rate.
    if action == "export" && (rest.len() == 1 || rest.len() == 2) {
        let output = &rest[0];
        let scene = document
            .root
            .children
            .iter()
            .rfind(|node| node.tag == "scene" && node.flag("active"))
            .ok_or("No active scene")?;
        let config = if project.join("package.json").exists() {
            Some(ad3_editor::project_config::ProjectConfig::load(project)?)
        } else {
            None
        };
        let fps_override = rest
            .get(1)
            .map(|v| v.parse::<f64>())
            .transpose()
            .map_err(|_| "Invalid FPS")?;
        let fps = match fps_override {
            Some(fps) => fps,
            None => config
                .as_ref()
                .map_or(Ok(30.0), |config| config.export_frame_rate(&scene.id))?,
        };
        let audio_enabled = config
            .as_ref()
            .map_or(Ok(true), |config| config.audio_enabled(&scene.id))?;
        let report = ProjectRenderer::export_with_audio(
            &document,
            project,
            tools,
            Path::new(output),
            fps,
            audio_enabled,
            None,
            |_, _| {},
        )?;
        return Ok(
            json!({"path":output,"width":report.width,"height":report.height,"duration":report.duration,"workareaDuration":report.workarea_duration,"videoDuration":report.video_duration,"fps":report.fps,"frames":report.frames,"warnings":report.warnings}),
        );
    }
    let library = MediaLibrary::load(&document.root, project, &tools)?;
    let offsets = ad3_editor::audio_sync::AudioSyncCache::default().offsets(
        &document.root,
        &library,
        &tools,
        None,
    )?;
    let timeline = Timeline::resolve_with_media_sync(&document.root, &library, 30.0, &offsets)?;
    match (action.as_str(), rest) {
        ("scene", [at, ids @ ..]) => {
            if ids.is_empty() {
                return Err("Scene requires at least one node id".into());
            }
            let at = parse_time(at)?;
            let mut renderer = ProjectRenderer::new(&document, project, tools.clone(), false)?;
            renderer.frame_plan(&timeline.active, at, 30.0, false)?;
            let geometry = renderer
                .edit_geometries(&timeline.active)
                .ok_or("Scene wrapping requires native frame geometry")?;
            let (next, scene) = document.wrap_scene(&timeline, ids, at, geometry)?;
            if scene.is_some() {
                next.save_over(&document)?;
            }
            Ok(json!({"ok": true, "entry": next.path, "scene": scene}))
        }
        ("sequence", ids) => {
            if ids.is_empty() {
                return Err("Sequence requires at least one node id".into());
            }
            let (next, sequence) = document.wrap_sequence(&timeline, ids)?;
            if sequence.is_some() {
                next.save_over(&document)?;
            }
            Ok(json!({"ok": true, "entry": next.path, "sequence": sequence}))
        }
        ("nudge", [at, dx, dy, ids @ ..]) => {
            if ids.is_empty() {
                return Err("Nudge requires at least one node id".into());
            }
            let dx = dx.parse::<f64>().map_err(|_| "Invalid horizontal nudge")?;
            let dy = dy.parse::<f64>().map_err(|_| "Invalid vertical nudge")?;
            let next = document.nudge(&timeline, ids, parse_time(at)?, [dx, dy])?;
            if next.source != document.source {
                next.save_over(&document)?;
            }
            Ok(json!({"ok":true,"entry":next.path}))
        }
        ("keyframe", [id, name, at, value]) => {
            let value = serde_json::from_str(value)
                .map_err(|error| format!("Keyframe value must be JSON: {error}"))?;
            let next = document.toggle_keyframe_at_playhead(
                &timeline,
                id,
                name,
                value,
                parse_time(at)?,
            )?;
            next.save_over(&document)?;
            Ok(json!({"ok":true,"entry":next.path}))
        }
        ("close-gap", [scene, sequence, start, end]) => {
            let next = document.close_gap(
                &timeline,
                scene,
                sequence,
                [parse_time(start)?, parse_time(end)?],
            )?;
            next.save_over(&document)?;
            Ok(json!({"ok":true,"entry":next.path}))
        }
        ("cut", [scene, at, ids @ ..]) => {
            let (next, units) =
                document.split_at_playhead(&timeline, scene, ids, parse_time(at)?)?;
            if !units.is_empty() {
                next.save_over(&document)?;
            }
            Ok(json!({"ok":true,"entry":next.path,"units":units}))
        }
        ("ripple", [scene, side, at, ids @ ..]) => {
            if side != "left" && side != "right" {
                return Err("Ripple side must be left or right".into());
            }
            let (next, units, time) =
                document.ripple_trim(&timeline, scene, ids, parse_time(at)?, side == "left")?;
            if !units.is_empty() {
                next.save_over(&document)?;
            }
            Ok(json!({"ok":true,"entry":next.path,"units":units,"time":time}))
        }
        ("capture", [scene, at, output]) => {
            let area = timeline.workareas.get(scene).ok_or("Scene not found")?;
            let relative_time = parse_time(at)?;
            let time = area[0] + relative_time;
            if relative_time < 0.0 || time >= area[1] {
                return Err("Capture time lies outside the workarea".into());
            }
            let mut renderer = ProjectRenderer::new(&document, project, tools, true)?;
            let plan = renderer.frame_plan(scene, time, 30.0, false)?;
            plan.capture(&renderer.tools, project, Path::new(output))?;
            Ok(
                json!({"path": output, "time": relative_time, "sceneTime": time,"warnings":renderer.warnings()}),
            )
        }
        ("context", []) => {
            Ok(json!({"projectDir": project, "entry": document.path, "timeline": timeline}))
        }
        ("check", []) => {
            let mut issues = Vec::new();
            for node in document.root.nodes() {
                if node.tag == "sequence" {
                    let mut clips = node
                        .children
                        .iter()
                        .filter_map(|child| timeline.spans.get(&child.id))
                        .collect::<Vec<_>>();
                    clips.sort_by(|a, b| a.start.total_cmp(&b.start));
                    for pair in clips.windows(2) {
                        if pair[0].end > pair[1].start {
                            issues.push(json!({"severity": "error", "node": pair[1].id, "message": "Sequence clips overlap"}));
                        }
                    }
                }
                if let Some(span) = timeline.spans.get(&node.id)
                    && span.end <= span.start
                {
                    issues.push(json!({"severity": "warning", "node": node.id, "message": "Node has no visible duration"}));
                }
            }
            Ok(
                json!({"issues": issues, "stats": {"nodes": document.root.nodes().len(), "scenes": timeline.workareas.len()}}),
            )
        }
        ("split", [id, at]) => {
            let next = document.split(&timeline, id, parse_time(at)?)?;
            next.save_over(&document)?;
            Ok(json!({"ok": true, "entry": next.path}))
        }
        ("trim", [id, start, end]) => {
            let next = document.trim(&timeline, id, parse_time(start)?, parse_time(end)?)?;
            next.save_over(&document)?;
            Ok(json!({"ok": true, "entry": next.path}))
        }
        _ => Err(HELP.into()),
    }
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h") {
        print!("{HELP}");
        return;
    }
    if args == ["--version"] {
        println!("dapi-rust {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let [group, action, rest @ ..] = args.as_slice()
        && group == "media"
        && matches!(action.as_str(), "filmstrip" | "film" | "waveform" | "grab")
        && rest
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print!(
            "{}",
            if action == "grab" {
                ad3_editor::grab::HELP
            } else if action == "waveform" {
                ad3_editor::waveform::HELP
            } else {
                ad3_editor::filmstrip::HELP
            }
        );
        return;
    }
    if let [group, action, rest @ ..] = args.as_slice()
        && group == "media"
        && action == "grab"
    {
        if let Err(error) = ad3_editor::grab::command(rest, &Tools::default(), None, |value| {
            use std::io::Write;
            writeln!(std::io::stdout().lock(), "{value}").map_err(|error| error.to_string())
        }) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args[0] == "fonts" {
        match ad3_editor::font_cli::output(
            &args[1..],
            &mut ad3_editor::fonts::FontBook::default(),
            None,
        ) {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    match run(&args) {
        Ok(value) => {
            println!("{value}");
            if value
                .get("issues")
                .and_then(Value::as_array)
                .is_some_and(|issues| issues.iter().any(|issue| issue["severity"] == "error"))
            {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

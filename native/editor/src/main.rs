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
  dapi-rust trim <project> <id> <in> <out>       Trim using scene times
  dapi-rust remove <project> <id>              Remove a source element
  dapi-rust export <project> <output.mp4> [fps] Export the active scene workarea
  dapi-rust capture <project> <scene> <time> <output.png>  Capture at workarea-relative time
  dapi-rust media probe <file>                 Read FFprobe metadata
  dapi-rust media waveform <file> <output.png> Render an audio waveform

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
            ("probe", [path]) => {
                serde_json::to_value(tools.probe(Path::new(path))?).map_err(|e| e.to_string())
            }
            ("waveform", [path, output]) => {
                tools.waveform(Path::new(path), Path::new(output))?;
                Ok(json!({"path": output}))
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
    let library = MediaLibrary::load(&document.root, project, &tools)?;
    let timeline = Timeline::resolve(&document.root, &library.durations)?;
    match (action.as_str(), rest) {
        ("export", [output]) | ("export", [output, _]) => {
            let fps = rest
                .get(1)
                .map(|v| v.parse::<f64>())
                .transpose()
                .map_err(|_| "Invalid FPS")?
                .unwrap_or(30.0);
            let report = ProjectRenderer::export(
                &document,
                project,
                tools,
                Path::new(output),
                fps,
                None,
                |_, _| {},
            )?;
            Ok(
                json!({"path":output,"width":report.width,"height":report.height,"duration":report.duration,"workareaDuration":report.workarea_duration,"videoDuration":report.video_duration,"fps":report.fps,"frames":report.frames,"warnings":report.warnings}),
            )
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

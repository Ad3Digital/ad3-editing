// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    source::{History, SourceDocument},
    timing::Timeline,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn parse(source: &str) -> SourceDocument {
    SourceDocument::parse(PathBuf::from("index.tsx"), source.into()).unwrap()
}

const PROJECT: &str = r##"// edição humana — preserve este comentário
const size = { width: 1920, height: 1080 };
export default function Project() {
  const rate = 2;
  return <stage><scene id="main" active width={size.width} height={size.height} workarea={[1, 7]}>
    <sequence id="track">
      <video id="take" src="take.mp4" start={1} sourceIn={2} sourceOut={14} playbackRate={rate} volume={-6} />
      <text id="title" start={2} end={4} fill="#fff">Ação &amp; café</text>
    </sequence>
  </scene></stage>;
}
"##;

#[test]
fn source_roundtrip_split_rate_workarea_and_caps() {
    let document = parse(PROJECT);
    let mut durations = BTreeMap::from([("take".into(), 12.0)]);
    let timeline = Timeline::resolve(&document.root, &durations).unwrap();
    let clip = timeline.get("take").unwrap();
    assert_eq!((clip.start, clip.end, clip.origin), (1.0, 6.0, 0.0));
    assert_eq!(timeline.workareas["main"], [1.0, 7.0]);
    let split = document.split(&timeline, "take", 3.0).unwrap();
    durations.insert("take-2".into(), 12.0);
    let timeline = Timeline::resolve(&split.root, &durations).unwrap();
    assert_eq!(
        (
            timeline.get("take").unwrap().end,
            timeline.get("take-2").unwrap().start
        ),
        (3.0, 3.0)
    );
    assert_eq!(timeline.get("take-2").unwrap().source_at(3.0), 6.0);
    assert_eq!(timeline.get("take-2").unwrap().end, 6.0);
    assert!(
        split
            .source
            .contains("// edição humana — preserve este comentário")
    );
    assert!(
        split
            .source
            .contains("<text id=\"title\" start={2} end={4} fill=\"#fff\">Ação &amp; café</text>")
    );
    assert_eq!(split.root.find("title").unwrap().text, "Ação & café");
    let trimmed = split.trim(&timeline, "take-2", 4.0, 5.0).unwrap();
    let trim_timeline = Timeline::resolve(&trimmed.root, &durations).unwrap();
    let clip = trim_timeline.get("take-2").unwrap();
    assert_eq!(
        (clip.start, clip.end, clip.source_in, clip.source_out),
        (4.0, 5.0, 8.0, 10.0)
    );
    let changed = trimmed
        .set("title", "name", Some(json!("aspas \" e < > { } café")))
        .unwrap();
    assert_eq!(
        changed.root.find("title").unwrap().string("name"),
        Some("aspas \" e < > { } café")
    );
}

#[test]
fn parent_rates_and_trims_gate_children_without_changing_origin() {
    let document = parse(
        r#"export default () => <stage><scene id="s" active width={100} height={100}>
      <group id="g" start={2} sourceIn={4} sourceOut={10} playbackRate={2} volume={-6}>
        <rect id="r" start={4} end={12} volume={-3}/>
      </group>
    </scene></stage>"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let span = timeline.get("r").unwrap();
    assert_eq!(
        (
            span.start,
            span.end,
            span.playback_rate,
            span.source_in,
            span.source_out,
            span.volume_db
        ),
        (2.0, 5.0, 2.0, 0.0, 6.0, -9.0)
    );
}

#[test]
fn dynamic_projects_fail_without_producing_a_partial_document() {
    for source in [
        "export default () => <stage>{clips.map(c => <video src={c.src}/>)}</stage>",
        "export default function P(){ if (true) return <stage/>; return <stage/>; }",
        "import './side-effect'; export default () => <stage/>;",
        "export default () => <stage><scene id='same'/><scene id='same'/></stage>",
    ] {
        assert!(
            SourceDocument::parse("index.tsx".into(), source.into()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn atomic_save_undo_redo_and_external_edit_conflict() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder =
        std::env::temp_dir().join(format!("ad3-rust-editing-{}-{unique}", std::process::id()));
    fs::create_dir(&folder).unwrap();
    let path = folder.join("index.tsx");
    fs::write(&path, PROJECT).unwrap();
    let mut history = History::new(SourceDocument::open(&folder).unwrap());
    history
        .commit(
            history
                .document
                .set("title", "x", Some(json!(100)))
                .unwrap(),
        )
        .unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("x={100}"));
    let before = history.document.source.clone();
    assert!(
        history
            .restore::<()>(false, |_| Err("Original media is now missing".into()))
            .is_err()
    );
    assert_eq!(history.document.source, before);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    history.undo().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), PROJECT);
    history.redo().unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("x={100}"));
    fs::write(&path, "// changed by a human").unwrap();
    assert!(history.undo().unwrap_err().contains("changed on disk"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "// changed by a human");
    // Only remove the unique test directory created above.
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-editing-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

#[test]
fn static_text_uses_javascript_primitive_and_array_spelling() {
    let document = parse(
        r#"export default () => <stage><scene id="s"><text id="t">{3}{[1,2] + ""}{({a: 1}) + ""}{-0}{1e21}{[1] + 2}{[] + []}{({a: 1}) + 2}</text></scene></stage>"#,
    );
    assert_eq!(
        document.root.find("t").unwrap().text,
        "31,2[object Object]01e+2112[object Object]2"
    );
}

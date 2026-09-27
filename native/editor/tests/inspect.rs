use ad3_editor::{
    evaluator::LiveProject,
    inspect,
    source::{History, SourceDocument},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ad3-rust-inspect-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, file: &str, source: &str) {
        let path = self.0.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let path = fs::canonicalize(&self.0).unwrap();
        assert!(path.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ad3-rust-inspect-")
        );
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn annotated_bindings_are_live_and_shadowed_locals_types_and_derived_values_are_preserved() {
    let source = r##"
        /** @inspect number min=0 max=200 step=0.5 path="Layout / Padding" */
        const padding: number = 24;
        /** @inspect color label="Accent" */
        const accentColor = '#ff5500';
        /** @inspect select options="short,long" */
        const mode = 'short';
        // This prose mentions @inspect number, without declaring a control.
        const untouched = 9;
        const derived = padding * 2;
        type Padding = typeof padding;
        const shadow = (padding: number) => ({padding});
        let mounts = 0;
        export default () => {
            mounts++;
            return <stage><scene id="s" active config={{padding, derived, untouched, shadow: shadow(3).padding, mounts}}>
                <rect id="r" x={padding as number} fill={accentColor}/>
                <text id="t">{mode}</text>
            </scene></stage>;
        };
    "##;
    let file = std::path::absolute("inspect-test.tsx").unwrap();
    let mut live = LiveProject::mount(&file, source, false).unwrap();
    let controls = live.inspect_controls().unwrap();
    assert_eq!(controls.len(), 3);
    boa_engine::gc::force_collect();
    assert_eq!(controls[0].label, "Padding");
    assert_eq!(controls[0].group, ["Layout"]);
    assert_eq!(controls[0].declaration.step, Some(0.5));
    live.set_inspect(&file, "padding", json!(42), false)
        .unwrap();
    live.set_inspect(&file, "accentColor", json!("#00ff00"), true)
        .unwrap();
    live.set_inspect(&file, "mode", json!("long"), true)
        .unwrap();
    let root = live.sample(1.0, false).unwrap();
    assert_eq!(root.find("r").unwrap().props["x"], json!(42.0));
    assert_eq!(root.find("r").unwrap().props["fill"], "#00ff00");
    assert_eq!(root.find("t").unwrap().text, "long");
    assert_eq!(
        root.find("s").unwrap().props["config"],
        json!({"padding":42.0,"derived":48.0,"untouched":9.0,"shadow":3.0,"mounts":1.0})
    );
    let controls = live.inspect_controls().unwrap();
    assert_eq!(controls[0].initial, json!(24));
    assert_eq!(controls[0].committed, json!(24));
    assert_eq!(controls[0].value, json!(42));
    assert!(
        live.set_inspect(&file, "padding", json!("bad"), true)
            .is_err()
    );
    assert!(
        live.set_inspect(&file, "mode", json!("missing"), true)
            .is_err()
    );
    live.set_inspect(&file, "padding", controls[0].committed.clone(), false)
        .unwrap();
    assert_eq!(
        live.sample(2.0, false).unwrap().find("r").unwrap().props["x"],
        json!(24.0)
    );
    drop(live);
    boa_engine::gc::force_collect();
    let mut reloaded = LiveProject::mount(&file, source, false).unwrap();
    assert_eq!(reloaded.inspect_controls().unwrap().len(), 3);
    assert_eq!(reloaded.inspect_controls().unwrap()[0].value, json!(24));
}

#[test]
fn invalid_annotations_fail_and_literal_spans_keep_original_utf8_source() {
    for declaration in [
        "/** @inspect number */ let x = 1;",
        "/** @inspect number */ const x = 1, y = 2;",
        "/** @inspect number */ const {x} = {x:1};",
        "/** @inspect number */ const x = 1 + 2;",
        "/** @inspect number */ export const x = 1;",
        "/** @inspect number */ const x = 1; export {x};",
        "function a(){\n/** @inspect number */\nconst x = 1;}",
        "/** @inspect number min=1 min=2 */ const x = 1;",
        "/** @inspect text min=0 */ const x = 'x';",
        "/** @inspect number nope=1 */ const x = 1;",
        "/** @inspect number path=Layout label=Size */ const x = 1;",
        "/** @inspect select options=one */ const x = 'one';",
        "/** @inspect select options=one,two */ const x = 'three';",
        "/** @inspect text */ const x = `template`;",
        "/** @inspect number */ const x = 1; x++;",
    ] {
        assert!(
            inspect::bindings(Path::new("index.tsx"), declaration).is_err(),
            "{declaration}"
        );
    }
    let source = "// Português 😀\n/**\n * @inspect number min=0x10\n */\nconst amount = -24;";
    let bindings = inspect::bindings(Path::new("index.tsx"), source).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(&source[bindings[0].initializer.clone()], "-24");
    assert_eq!(bindings[0].declaration.min, Some(16.0));
    assert_eq!(bindings[0].initial, json!(-24.0));
}

#[test]
fn control_edits_write_only_the_imported_literal_and_undo_detects_dependency_changes() {
    let project = Project::new();
    let entry = "import {Card} from './ui/card'; export default () => <stage><scene id='s' active><Card/></scene></stage>;";
    let card = "// Olá 😀\n/** @inspect number path=Layout/Size */\nconst size = 24;\nexport const Card = () => <rect id='r' width={size} height={size}/>;\n";
    project.write("index.tsx", entry);
    project.write("ui/card.tsx", card);
    let document = SourceDocument::open(&project.0).unwrap();
    assert_eq!(document.controls().unwrap().len(), 1);
    let next = document
        .set_control(Path::new("ui/card.tsx"), "size", json!(72))
        .unwrap();
    assert_eq!(
        fs::read_to_string(project.0.join("ui/card.tsx")).unwrap(),
        card
    );
    assert_eq!(next.root.find("r").unwrap().props["width"], json!(72.0));
    let mut history = History::new(document);
    history.commit(next).unwrap();
    assert_eq!(
        fs::read_to_string(project.0.join("index.tsx")).unwrap(),
        entry
    );
    assert_eq!(
        fs::read_to_string(project.0.join("ui/card.tsx")).unwrap(),
        card.replace("size = 24", "size = 72")
    );
    history.undo().unwrap();
    assert_eq!(
        fs::read_to_string(project.0.join("ui/card.tsx")).unwrap(),
        card
    );
    history.redo().unwrap();
    project.write("index.tsx", &(entry.to_owned() + "\n// external"));
    assert!(history.undo().unwrap_err().contains("changed on disk"));
    assert!(
        fs::read_to_string(project.0.join("ui/card.tsx"))
            .unwrap()
            .contains("size = 72")
    );
}

#[test]
fn static_annotations_use_the_runtime_and_source_edits_validate_types() {
    let project = Project::new();
    project.write("index.tsx", "/** @inspect boolean */ const visible = true; export default () => <stage><scene id='s' active enabled={visible}/></stage>;");
    let document = SourceDocument::open(&project.0).unwrap();
    assert!(!document.module_sources.is_empty());
    assert!(
        document
            .set_control(Path::new("index.tsx"), "visible", json!("false"))
            .is_err()
    );
    let next = document
        .set_control(Path::new("index.tsx"), "visible", json!(false))
        .unwrap();
    next.save_over(&document).unwrap();
    assert_eq!(
        SourceDocument::open(&project.0)
            .unwrap()
            .root
            .find("s")
            .unwrap()
            .props["enabled"],
        false
    );
}

#[test]
fn escaped_helper_names_do_not_collide_with_the_compiler_import() {
    let source = r#"
        const __ad3\u0049nspect0 = 7;
        const __ad3Inspect1 = 8;
        /** @inspect number */
        const width = 24;
        export default () => <stage><scene id="s" config={[__ad3\u0049nspect0, __ad3Inspect1, width]}/></stage>;
    "#;
    let file = std::path::absolute("escaped-inspect.tsx").unwrap();
    let mut live = LiveProject::mount(&file, source, false).unwrap();
    live.set_inspect(&file, "width", json!(32), false).unwrap();
    assert_eq!(
        live.sample(0.0, false).unwrap().find("s").unwrap().props["config"],
        json!([7.0, 8.0, 32.0])
    );
}

#[test]
fn source_transactions_reject_package_targets_and_multi_file_or_graph_changes() {
    let project = Project::new();
    let folder = if cfg!(windows) {
        "NODE_MODULES"
    } else {
        "node_modules"
    };
    project.write("index.tsx", &format!("import {{Card}} from './{folder}/card'; export default () => <stage><scene><Card/></scene></stage>;"));
    let file = format!("{folder}/card.tsx");
    let card = "/** @inspect number */ const size = 10; export const Card = () => <rect id='r' width={size}/>;";
    project.write(&file, card);
    let document = SourceDocument::open(&project.0).unwrap();
    assert!(
        document
            .set_control(Path::new(&file), "size", json!(20))
            .unwrap_err()
            .contains("node_modules")
    );
    let mut graph = document.clone();
    graph
        .module_sources
        .insert(project.0.join("new.ts"), "export default 1".into());
    assert!(
        graph
            .save_over(&document)
            .unwrap_err()
            .contains("module graph")
    );
    let mut multiple = document.clone();
    multiple.source.push_str("\n// edited entry");
    multiple
        .module_sources
        .insert(multiple.path.clone(), multiple.source.clone());
    let imported = fs::canonicalize(project.0.join(&file)).unwrap();
    multiple
        .module_sources
        .insert(imported, format!("{card}\n// edited import"));
    assert!(
        multiple
            .save_over(&document)
            .unwrap_err()
            .contains("one module")
    );
    assert_eq!(fs::read_to_string(project.0.join(&file)).unwrap(), card);
}

#[cfg(windows)]
#[test]
fn source_save_preserves_creation_time_and_named_streams() {
    use std::os::windows::fs::MetadataExt;
    let project = Project::new();
    project.write("index.tsx", "/** @inspect number */ const size = 10; export default () => <stage><scene id='s' width={size}/></stage>;");
    let file = project.0.join("index.tsx");
    let stream = PathBuf::from(format!("{}:ad3-native-meta", file.display()));
    fs::write(&stream, "keep this stream").unwrap();
    let creation = fs::metadata(&file).unwrap().creation_time();
    let document = SourceDocument::open(&project.0).unwrap();
    let next = document
        .set_control(Path::new("index.tsx"), "size", json!(20))
        .unwrap();
    next.save_over(&document).unwrap();
    assert_eq!(fs::metadata(&file).unwrap().creation_time(), creation);
    assert_eq!(fs::read_to_string(&stream).unwrap(), "keep this stream");
    assert!(
        fs::read_dir(project.0.join(".diffusion"))
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "native-source.lock")
    );
}

#[cfg(unix)]
#[test]
fn source_save_preserves_executable_and_group_modes() {
    use std::os::unix::fs::PermissionsExt;
    let project = Project::new();
    project.write("index.tsx", "/** @inspect number */ const size = 10; export default () => <stage><scene id='s' width={size}/></stage>;");
    let file = project.0.join("index.tsx");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o750)).unwrap();
    let document = SourceDocument::open(&project.0).unwrap();
    document
        .set_control(Path::new("index.tsx"), "size", json!(20))
        .unwrap()
        .save_over(&document)
        .unwrap();
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

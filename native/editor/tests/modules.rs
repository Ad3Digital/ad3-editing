use ad3_editor::{evaluator::evaluate, source::SourceDocument};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let name = format!(
            "ad3-rust-modules-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
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
                .starts_with("ad3-rust-modules-")
        );
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn executes_typed_components_arrays_conditionals_and_javascript_math() {
    let source = r##"
        interface Clip { title: string, x: number }
        const clips: Clip[] = [{ title: 'Olá', x: 10 }, { title: 'Rust', x: 20 }];
        function Title({ title, x, children }: Clip & {children?: unknown}) {
            return <text id={title} {...{x: Math.floor(x * 1.5)}}>{title}{children}</text>;
        }
        export default function Project() {
            if (!clips.length) throw new Error('empty');
            return <stage><scene id="s" active width={320} height={180}>
                <>{clips.map(clip => <Title {...clip}> &amp; {3}</Title>)}</>
                {true && <rect id="r" fill="#123456" />}{false}{null}
            </scene></stage>;
        }
    "##;
    let result = evaluate(&PathBuf::from("index.tsx"), source).unwrap();
    assert_eq!(result.root.find("Olá").unwrap().text, "Olá & 3");
    assert_eq!(result.root.find("Rust").unwrap().props["x"], json!(30.0));
    assert_eq!(result.root.find("s").unwrap().children.len(), 3);
    assert_eq!(result.sources.len(), 1);
}

#[test]
fn opens_relative_modules_json_reexports_and_installed_esm_packages() {
    let project = Project::new();
    project.write(
        "index.tsx",
        r#"
        import { Card } from './components';
        import layout from './layout.json';
        import { height } from '@example/layout';
        import { width } from './reexport';
        export default () => <stage><scene id="s" active width={width} height={height}>
            {layout.map(item => <Card item={item}/>)}</scene></stage>;
    "#,
    );
    project.write(
        "components.tsx",
        "export const Card = ({item}) => <rect id={item.id} x={item.x} width={10} height={10}/>;",
    );
    project.write(
        "layout.json",
        r#"[{"id":"first","x":10},{"id":"second","x":20}]"#,
    );
    project.write("reexport.ts", "export { width } from './dimensions';");
    project.write(
        "dimensions.ts",
        "export const width = await Promise.resolve(320);",
    );
    project.write("node_modules/@example/layout/package.json", r#"{"name":"@example/layout","exports":{".":{"browser":"./browser.js","default":"./wrong.js"}}}"#);
    project.write(
        "node_modules/@example/layout/browser.js",
        "export const height = 180;",
    );
    project.write(
        "node_modules/@example/layout/wrong.js",
        "throw new Error('wrong export condition');",
    );
    let document = SourceDocument::open(&project.0).unwrap();
    assert_eq!(
        document.root.find("s").unwrap().props["width"],
        json!(320.0)
    );
    assert_eq!(
        document.root.find("s").unwrap().props["height"],
        json!(180.0)
    );
    assert_eq!(
        document.root.find("second").unwrap().props["x"],
        json!(20.0)
    );
    assert_eq!(document.module_sources.len(), 6);
    let original = fs::read_to_string(project.0.join("index.tsx")).unwrap();
    assert!(
        document
            .set("first", "x", Some(json!(999)))
            .unwrap_err()
            .contains("source mapping")
    );
    assert!(document.remove("first").is_err());
    assert!(document.activate_scene("s").is_err());
    assert_eq!(
        fs::read_to_string(project.0.join("index.tsx")).unwrap(),
        original
    );
}

#[test]
fn rejects_unported_hosts_callbacks_duplicate_ids_and_cycles() {
    for (source, message) in [
        (
            "import {createStore} from 'solid-js/store'; export default () => <stage/>;",
            "reactive host",
        ),
        (
            "export default () => <stage><scene><surface draw={() => 0}/></scene></stage>;",
            "Function-valued",
        ),
        (
            "export default () => <stage>{[1,2].map(() => <scene id='same'/>)}</stage>;",
            "Duplicate id",
        ),
        (
            "const cycle = {}; cycle.self = cycle; export default () => <stage camera={cycle}/>;",
            "cycle",
        ),
        (
            "export default () => <stage><scene width={Infinity}/></stage>;",
            "Non-finite",
        ),
        (
            "import x from './missing-file'; export default () => <stage/>;",
            "Could not resolve",
        ),
        (
            "export default () => <stage config={new Map([['x', 1]])}/>;",
            "plain data",
        ),
        (
            "export default () => <stage config={new Set([1])}/>;",
            "plain data",
        ),
        (
            "export default () => <stage config={new Date()}/>;",
            "plain data",
        ),
        (
            "export default () => <stage config={Promise.resolve(1)}/>;",
            "plain data",
        ),
        (
            "class Config { #x = 1 }; export default () => <stage config={new Config()}/>;",
            "plain data",
        ),
    ] {
        let result = evaluate(&PathBuf::from("index.tsx"), source);
        assert!(result.is_err(), "{source}");
        assert!(
            result.err().unwrap().contains(message),
            "{source}: expected {message}"
        );
    }
}

#[test]
fn bounds_loops_and_self_replenishing_promise_jobs() {
    let result = evaluate(
        &PathBuf::from("index.tsx"),
        "while(true) {} export default () => <stage/>;",
    );
    assert!(result.is_err());
    let result = evaluate(
        &PathBuf::from("index.tsx"),
        "function again(){Promise.resolve().then(again)} again(); export default () => <stage/>;",
    );
    assert!(result.err().unwrap().contains("job limit"));
}

#[test]
fn materialized_json_omits_optional_undefined_fields_without_editing_source() {
    let source = "const config = {kept: 1, optional: undefined}; export default () => <stage config={config}><scene width={undefined}>{undefined}</scene></stage>;";
    let result = evaluate(&PathBuf::from("index.tsx"), source).unwrap();
    assert_eq!(result.root.props["config"], json!({"kept": 1.0}));
    assert!(!result.root.children[0].props.contains_key("width"));
    assert!(result.root.children[0].children.is_empty());
    assert_eq!(result.sources.values().next().unwrap(), source);
}

#[test]
fn refuses_browser_remapping_tables_and_blocked_conditional_exports() {
    for (manifest, message) in [
        (
            r#"{"browser":{"./server.js":"./browser.js"},"module":"./server.js"}"#,
            "browser remapping",
        ),
        (
            r#"{"exports":{".":{"browser":null,"default":"./server.js"}}}"#,
            "explicitly blocked",
        ),
    ] {
        let project = Project::new();
        project.write("index.tsx", "import {width} from 'example'; export default () => <stage><scene width={width}/></stage>;");
        project.write("node_modules/example/package.json", manifest);
        project.write(
            "node_modules/example/server.js",
            "export const width = 320;",
        );
        project.write(
            "node_modules/example/browser.js",
            "export const width = 640;",
        );
        let result = SourceDocument::open(&project.0);
        assert!(result.unwrap_err().contains(message));
    }
}

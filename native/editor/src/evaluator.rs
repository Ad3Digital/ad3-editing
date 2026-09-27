// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native ECMAScript module evaluation. Oxc removes TS/JSX and Boa executes
//! user-authored code. No Node process, Electron, package installation or network.
use crate::{
    Result,
    document::{Location, Node, TAGS},
};
use boa_engine::{
    Context, JsNativeError, JsResult, JsValue, Module, NativeFunction, Source,
    builtins::promise::PromiseState,
    job::{Job, JobExecutor, SimpleJobExecutor},
    js_string,
    module::{ModuleLoader, ModuleRequest, Referrer},
    object::{JsObject, ObjectInitializer, builtins::JsArray},
    property::Attribute,
};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{JsxRuntime, TransformOptions, Transformer};
use serde_json::{Map, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

const MAX_MODULES: usize = 512;
const MAX_SOURCE: u64 = 16 * 1024 * 1024;
const MAX_NODES: usize = 100_000;

#[derive(Default)]
struct Jobs {
    inner: Rc<SimpleJobExecutor>,
    count: Cell<usize>,
    exhausted: Cell<bool>,
}
impl JobExecutor for Jobs {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        if self.count.get() >= 10_000 {
            self.exhausted.set(true);
            return;
        }
        self.count.set(self.count.get() + 1);
        self.inner.clone().enqueue_job(job, context);
    }
    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        self.inner.clone().run_jobs(context)?;
        if self.exhausted.get() {
            return Err(error("Project exceeded the 10,000 asynchronous job limit"));
        }
        Ok(())
    }
}

fn error(message: impl Into<String>) -> boa_engine::JsError {
    JsNativeError::typ().with_message(message.into()).into()
}

fn transpile(path: &Path, source: &str) -> Result<String> {
    if source.len() as u64 > MAX_SOURCE {
        return Err("Module exceeds the 16 MiB source limit".into());
    }
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).map_err(|e| e.to_string())?;
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!("{}: {:?}", path.display(), parsed.diagnostics));
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(format!("{}: {:?}", path.display(), semantic.diagnostics));
    }
    let mut options = TransformOptions::default();
    options.jsx.runtime = JsxRuntime::Classic;
    options.jsx.pragma = Some("__ad3Jsx".into());
    options.jsx.pragma_frag = Some("__ad3Fragment".into());
    options.jsx.development = false;
    options.typescript.jsx_pragma = "__ad3Jsx".into();
    options.typescript.jsx_pragma_frag = "__ad3Fragment".into();
    options.typescript.only_remove_type_imports = true;
    let transformed = Transformer::new(&allocator, path, &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.diagnostics.is_empty() {
        return Err(format!("{}: {:?}", path.display(), transformed.diagnostics));
    }
    Ok(Codegen::new().build(&program).code)
}

fn read(path: &Path) -> Result<String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_SOURCE {
        return Err(format!(
            "{} exceeds the module source limit",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn file_candidate(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return fs::canonicalize(path).ok();
    }
    // Resolve the local authoring extensions without a Node resolver process.
    for extension in ["tsx", "ts", "jsx", "js", "mjs", "json"] {
        let candidate = PathBuf::from(format!("{}.{extension}", path.display()));
        if candidate.is_file() {
            return fs::canonicalize(candidate).ok();
        }
    }
    None
}

fn resolve_file(path: &Path) -> Result<PathBuf> {
    if let Some(path) = file_candidate(path) {
        return Ok(path);
    }
    if path.is_dir() {
        let package = path.join("package.json");
        if package.is_file() {
            let package: Value =
                serde_json::from_str(&read(&package)?).map_err(|e| e.to_string())?;
            if package.get("browser").is_some_and(Value::is_object) {
                return Err("Package browser remapping tables are not ported yet".into());
            }
            for field in ["browser", "module", "main"] {
                if let Some(entry) = package.get(field).and_then(Value::as_str)
                    && let Some(path) = file_candidate(&path.join(entry))
                {
                    return Ok(path);
                }
            }
        }
        if let Some(path) = file_candidate(&path.join("index")) {
            return Ok(path);
        }
    }
    Err(format!("Could not resolve module {}", path.display()))
}

fn export_target(value: &Value) -> Result<Option<&str>> {
    match value {
        Value::String(target) => Ok(Some(target)),
        Value::Null => Err("Package export is explicitly blocked".into()),
        Value::Array(_) => Err("Package export fallback arrays are not ported yet".into()),
        Value::Object(conditions) => {
            for (key, value) in conditions {
                if matches!(key.as_str(), "browser" | "import" | "default")
                    && let Some(target) = export_target(value)?
                {
                    return Ok(Some(target));
                }
            }
            Ok(None)
        }
        _ => Err("Invalid package export target".into()),
    }
}

fn resolve(specifier: &str, referrer: &Path) -> Result<PathBuf> {
    if matches!(
        specifier,
        "solid-js" | "solid-js/store" | "@diffusionstudio/jsx"
    ) {
        return Err(format!(
            "Host module {specifier} requires the native reactive host, which is still being ported"
        ));
    }
    if specifier.contains("://") || specifier.starts_with("node:") {
        return Err(format!(
            "Remote modules and Node builtins are unavailable: {specifier}"
        ));
    }
    let parent = referrer.parent().ok_or("Module has no parent directory")?;
    if specifier.starts_with('.') || Path::new(specifier).is_absolute() {
        return resolve_file(&parent.join(specifier));
    }
    let components: Vec<_> = specifier.split('/').collect();
    let count = if specifier.starts_with('@') { 2 } else { 1 };
    if components.len() < count
        || components
            .iter()
            .any(|part| part.is_empty() || *part == "..")
    {
        return Err(format!("Invalid package specifier: {specifier}"));
    }
    let package_name = components[..count].join("/");
    let subpath = if components.len() == count {
        ".".into()
    } else {
        format!("./{}", components[count..].join("/"))
    };
    for ancestor in parent.ancestors() {
        let root = ancestor.join("node_modules").join(&package_name);
        let manifest = root.join("package.json");
        if !manifest.is_file() {
            continue;
        }
        let package: Value = serde_json::from_str(&read(&manifest)?).map_err(|e| e.to_string())?;
        if package.get("browser").is_some_and(Value::is_object) {
            return Err("Package browser remapping tables are not ported yet".into());
        }
        if let Some(exports) = package.get("exports") {
            let branch = if exports
                .as_object()
                .is_some_and(|map| map.keys().any(|key| key.starts_with('.')))
            {
                exports.get(&subpath)
            } else if subpath == "." {
                Some(exports)
            } else {
                None
            };
            let target = branch
                .map(export_target)
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    format!(
                        "Package does not export {specifier}; wildcard exports are not ported yet"
                    )
                })?;
            if !target.starts_with("./") || target.split('/').any(|part| part == "..") {
                return Err(format!("Invalid package export target: {target}"));
            }
            return resolve_file(&root.join(target));
        }
        return resolve_file(&if subpath == "." {
            root
        } else {
            root.join(&subpath)
        });
    }
    Err(format!(
        "Could not resolve {specifier:?}; install the package in the project first"
    ))
}

#[derive(Default)]
struct Loader {
    modules: RefCell<BTreeMap<PathBuf, Module>>,
    sources: RefCell<BTreeMap<PathBuf, String>>,
}

impl Loader {
    fn compile(&self, path: PathBuf, source: String, context: &mut Context) -> JsResult<Module> {
        if let Some(module) = self.modules.borrow().get(&path) {
            return Ok(module.clone());
        }
        if self.modules.borrow().len() >= MAX_MODULES {
            return Err(error("Project exceeds the 512 module limit"));
        }
        let code = if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let value: Value = serde_json::from_str(&source).map_err(|e| error(e.to_string()))?;
            // JSON.parse preserves own __proto__ keys, unlike an object literal.
            format!(
                "export default JSON.parse({});",
                serde_json::to_string(&value.to_string()).unwrap()
            )
        } else {
            transpile(&path, &source).map_err(error)?
        };
        let module = Module::parse(Source::from_bytes(&code).with_path(&path), None, context)?;
        self.sources.borrow_mut().insert(path.clone(), source);
        self.modules.borrow_mut().insert(path, module.clone());
        Ok(module)
    }
}

impl ModuleLoader for Loader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        let referrer = referrer
            .path()
            .ok_or_else(|| error("A module import requires a source path"))?;
        let path =
            resolve(&request.specifier().to_std_string_escaped(), referrer).map_err(error)?;
        for attribute in request.attributes() {
            if attribute.key() != &js_string!("type")
                || attribute.value() != &js_string!("json")
                || !path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                return Err(error("Unsupported or mismatched module import attribute"));
            }
        }
        if let Some(module) = self.modules.borrow().get(&path) {
            return Ok(module.clone());
        }
        let source = read(&path).map_err(error)?;
        self.compile(path, source, &mut context.borrow_mut())
    }
}

fn jsx(_: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let tag = arguments
        .first()
        .ok_or_else(|| error("JSX tag is missing"))?;
    let props = arguments
        .get(1)
        .and_then(JsValue::as_object)
        .unwrap_or_else(|| ObjectInitializer::new(context).build());
    let children = &arguments[arguments.len().min(2)..];
    if tag.is_null() {
        return Ok(JsArray::from_iter(children.iter().cloned(), context).into());
    }
    if let Some(component) = tag.as_callable() {
        if !children.is_empty() {
            let value = if children.len() == 1 {
                children[0].clone()
            } else {
                JsArray::from_iter(children.iter().cloned(), context).into()
            };
            props.set(js_string!("children"), value, true, context)?;
        }
        return component.call(&JsValue::undefined(), &[props.into()], context);
    }
    let name = tag
        .as_string()
        .ok_or_else(|| error("JSX tag must be a component or intrinsic"))?
        .to_std_string_escaped();
    if !TAGS.contains(&name.as_str()) {
        return Err(error(format!(
            "<{name}> needs the native HTML host, which is still being ported"
        )));
    }
    let children = if children.is_empty() {
        vec![props.get(js_string!("children"), context)?]
    } else {
        children.to_vec()
    };
    let children = JsArray::from_iter(children, context);
    Ok(ObjectInitializer::new(context)
        .property(js_string!("tag"), tag.clone(), Attribute::all())
        .property(js_string!("props"), props, Attribute::all())
        .property(js_string!("children"), children, Attribute::all())
        .build()
        .into())
}

fn node(value: &Value, serial: &mut usize, ids: &mut BTreeSet<String>) -> Result<Node> {
    *serial += 1;
    if *serial > MAX_NODES {
        return Err("Project exceeds the 100,000 node limit".into());
    }
    let tag = value["tag"]
        .as_str()
        .filter(|tag| TAGS.contains(tag))
        .ok_or("Component returned an invalid composition node")?
        .to_owned();
    let mut props = value["props"].as_object().cloned().unwrap_or_default();
    props.remove("children");
    let id = match props.get("id") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.clone(),
        Some(_) => return Err("Node id must be a nonempty string".into()),
        None => format!("native-eval-{serial}"),
    };
    if !ids.insert(id.clone()) {
        return Err(format!(
            "Duplicate id {id:?}; generated instances must currently have distinct ids"
        ));
    }
    let mut result = Node {
        id,
        tag,
        props,
        text: String::new(),
        children: Vec::new(),
        location: Location::default(),
    };
    append(&value["children"], &mut result, serial, ids)?;
    if !result.text.trim().is_empty() && !matches!(result.tag.as_str(), "text" | "textRange") {
        return Err(format!(
            "Text is only valid inside <text> / <textRange>, found in <{}>",
            result.tag
        ));
    }
    Ok(result)
}

fn append(
    value: &Value,
    parent: &mut Node,
    serial: &mut usize,
    ids: &mut BTreeSet<String>,
) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) => (),
        Value::String(text) => parent.text.push_str(text),
        Value::Number(number) => parent.text.push_str(
            dragonbox_ecma::Buffer::new().format(number.as_f64().ok_or("Invalid number")?),
        ),
        Value::Array(values) => {
            for value in values {
                append(value, parent, serial, ids)?;
            }
        }
        Value::Object(_) => parent.children.push(node(value, serial, ids)?),
    }
    Ok(())
}

pub struct Evaluation {
    pub root: Node,
    pub sources: BTreeMap<PathBuf, String>,
}

// Do not use JSON.stringify/toJSON: those can silently erase functions and
// change the composition. Read each data property once and reject unsupported
// native-host callbacks and cycles before a renderer receives a partial tree.
fn snapshot_json(
    value: &JsValue,
    context: &mut Context,
    ancestors: &mut Vec<JsObject>,
    count: &mut usize,
) -> JsResult<Value> {
    *count += 1;
    if *count > 1_000_000 || ancestors.len() > 128 {
        return Err(error("Composition data exceeds its nesting/size limit"));
    }
    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if let Some(value) = value.as_boolean() {
        return Ok(Value::Bool(value));
    }
    if let Some(value) = value.as_number() {
        return serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| error("Non-finite composition property"));
    }
    if let Some(value) = value.as_string() {
        return Ok(Value::String(value.to_std_string_escaped()));
    }
    if value.is_callable() {
        return Err(error(
            "Function-valued properties/children require the native reactive/drawing host, which is still being ported",
        ));
    }
    let object = value
        .as_object()
        .ok_or_else(|| error("Unsupported composition property type"))?;
    if ancestors.contains(&object) {
        return Err(error("Composition properties contain a cycle"));
    }
    ancestors.push(object.clone());
    let result = if object.is_array() {
        let length = object
            .get(js_string!("length"), context)?
            .to_length(context)?;
        if length > MAX_NODES as u64 {
            return Err(error("Composition array is too large"));
        }
        let mut values = Vec::with_capacity(length as usize);
        for index in 0..length {
            values.push(snapshot_json(
                &object.get(index, context)?,
                context,
                ancestors,
                count,
            )?);
        }
        Value::Array(values)
    } else {
        if !object.is_ordinary()
            || object.prototype().is_some_and(|prototype| {
                prototype != context.intrinsics().constructors().object().prototype()
            })
        {
            return Err(error(
                "Composition properties must contain plain data; native objects and class instances are not supported",
            ));
        }
        let mut values = Map::new();
        for key in object.own_property_keys(context)? {
            let property = object.get(key.clone(), context)?;
            if property.is_undefined() {
                continue;
            }
            values.insert(
                key.to_string(),
                snapshot_json(&property, context, ancestors, count)?,
            );
        }
        Value::Object(values)
    };
    ancestors.pop();
    Ok(result)
}

pub fn evaluate(path: &Path, source: &str) -> Result<Evaluation> {
    let path = fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .map_err(|e| e.to_string())?;
    let loader = Rc::new(Loader::default());
    let mut context = Context::builder()
        .module_loader(loader.clone())
        .job_executor(Rc::new(Jobs::default()))
        .can_block(false)
        .build()
        .map_err(|e| e.to_string())?;
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(1_000_000);
    context.runtime_limits_mut().set_recursion_limit(128);
    context
        .register_global_builtin_callable(
            js_string!("__ad3Jsx"),
            2,
            NativeFunction::from_fn_ptr(jsx),
        )
        .map_err(|e| e.to_string())?;
    context
        .register_global_property(
            js_string!("__ad3Fragment"),
            JsValue::null(),
            Attribute::empty(),
        )
        .map_err(|e| e.to_string())?;
    let module = loader
        .compile(path, source.into(), &mut context)
        .map_err(|e| e.to_string())?;
    let promise = module.load_link_evaluate(&mut context);
    context.run_jobs().map_err(|e| e.to_string())?;
    match promise.state() {
        PromiseState::Fulfilled(_) => (),
        PromiseState::Rejected(reason) => {
            return Err(reason
                .to_string(&mut context)
                .map_err(|e| e.to_string())?
                .to_std_string_escaped());
        }
        PromiseState::Pending => {
            return Err(
                "Module evaluation did not settle; asynchronous host work is not ported yet".into(),
            );
        }
    }
    let exported = module
        .namespace(&mut context)
        .get(js_string!("default"), &mut context)
        .map_err(|e| e.to_string())?;
    let rendered = if let Some(component) = exported.as_callable() {
        component
            .call(&JsValue::undefined(), &[], &mut context)
            .map_err(|e| e.to_string())?
    } else {
        exported
    };
    let value = snapshot_json(&rendered, &mut context, &mut Vec::new(), &mut 0)
        .map_err(|e| e.to_string())?;
    let root = node(&value, &mut 0, &mut BTreeSet::new())?;
    if root.tag != "stage" {
        return Err("Project must return a <stage>".into());
    }
    let sources = loader.sources.borrow().clone();
    Ok(Evaluation { root, sources })
}

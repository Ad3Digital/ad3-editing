// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native ECMAScript module evaluation. Oxc removes TS/JSX and Boa executes
//! user-authored code. No Node process, Electron, package installation or network.
use crate::reactive;
use crate::{Result, document::Node};
use boa_engine::{
    Context, JsNativeError, JsResult, JsValue, Module, NativeFunction, Source,
    builtins::promise::PromiseState,
    job::{Job, JobExecutor, SimpleJobExecutor},
    js_string,
    module::{ModuleLoader, ModuleRequest, Referrer},
    object::{JsObject, ObjectInitializer},
    property::Attribute,
};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use serde_json::{Map, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
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
    let lowered = crate::jsx::lower(path, source)?;
    let lowered = crate::inspect::compile(path, &lowered)?;
    let source = lowered.as_str();
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
        "solid-js" | "solid-js/store" | "@ad3-editing/jsx"
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
    hosts: RefCell<BTreeMap<String, Module>>,
    frozen: Option<BTreeMap<PathBuf, String>>,
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
        let specifier = request.specifier().to_std_string_escaped();
        if let Some(module) = self.hosts.borrow().get(&specifier) {
            if !request.attributes().is_empty() {
                return Err(error("Host modules do not accept import attributes"));
            }
            return Ok(module.clone());
        }
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
        let source = if let Some(frozen) = &self.frozen {
            frozen.get(&path).cloned().ok_or_else(|| {
                error(format!(
                    "Module is missing from the frozen project snapshot: {}",
                    path.display()
                ))
            })?
        } else {
            read(&path).map_err(error)?
        };
        self.compile(path, source, &mut context.borrow_mut())
    }
}

pub struct Evaluation {
    pub root: Node,
    pub sources: BTreeMap<PathBuf, String>,
}

// Do not use JSON.stringify/toJSON: those can silently erase functions and
// change the composition. Read each data property once and reject unsupported
// native-host callbacks and cycles before a renderer receives a partial tree.
pub(crate) fn snapshot_json(
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

/// A project module graph and reactive owner tree, kept alive across frames.
/// Boa executes author code in-process; all editor host behavior is Rust.
pub struct LiveProject {
    context: Context,
    loader: Rc<Loader>,
    jobs: Rc<Jobs>,
    host: reactive::Host,
    rendered: JsValue,
    last_time: Option<f64>,
    pub warnings: Vec<String>,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

// Authoring helper follows packages/jsx/src/time.ts, including parseFloat's
// permissive trailing text. CLI timing validation remains intentionally strict.
fn parse_time_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(value) = args.first() else {
        return Ok(JsValue::undefined());
    };
    let finite = |number: f64| {
        if number.is_finite() {
            JsValue::from(number)
        } else {
            JsValue::undefined()
        }
    };
    if let Some(number) = value.as_number() {
        return Ok(finite(number));
    }
    let Some(text) = value.as_string() else {
        return Ok(JsValue::undefined());
    };
    let text = text.to_std_string_escaped();
    let text = text.trim();
    if text.is_empty() {
        return Ok(JsValue::undefined());
    }
    if text.ends_with(['f', 'F']) {
        let body = &text[..text.len() - 1];
        let unsigned = body.strip_prefix('-').unwrap_or(body);
        let parts = unsigned.split('.').collect::<Vec<_>>();
        if parts.len() <= 2
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            return Ok(finite(body.parse::<f64>().unwrap_or(f64::NAN) / 30.0));
        }
    }
    if text.contains(':') {
        let negative = text.starts_with('-');
        let parts = text
            .strip_prefix('-')
            .unwrap_or(text)
            .split(':')
            .collect::<Vec<_>>();
        if !(2..=3).contains(&parts.len()) {
            return Ok(JsValue::undefined());
        }
        let mut seconds = 0.0;
        for part in parts {
            seconds = seconds * 60.0 + JsValue::from(js_string!(part)).to_number(context)?;
        }
        return Ok(finite(if negative { -seconds } else { seconds }));
    }
    let parse = context
        .global_object()
        .get(js_string!("parseFloat"), context)?;
    let value = reactive::callable(Some(&parse))?.call(
        &JsValue::undefined(),
        &[js_string!(text).into()],
        context,
    )?;
    Ok(finite(value.to_number(context)?))
}

fn install_hosts(loader: &Loader, context: &mut Context) -> JsResult<()> {
    let solid = ObjectInitializer::new(context).build();
    for (name, callback) in reactive::SOLID_FUNCTIONS
        .iter()
        .chain(crate::control::FUNCTIONS)
        .copied()
        .chain([
            (
                "mergeProps",
                crate::props::merge as boa_engine::native_function::NativeFunctionPointer,
            ),
            ("splitProps", crate::props::split),
        ])
    {
        let function = reactive::function(context, NativeFunction::from_fn_ptr(callback), 1);
        solid.set(js_string!(name), function, true, context)?;
    }
    let jsx = ObjectInitializer::new(context)
        .function(
            NativeFunction::from_fn_ptr(crate::inspect::host),
            js_string!("__inspect"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(reactive::use_ticker),
            js_string!("useTicker"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(parse_time_api),
            js_string!("parseTime"),
            1,
        )
        .property(js_string!("TIME_FPS"), 30, Attribute::empty())
        .build();
    for (specifier, global, object) in [
        ("solid-js", "__ad3Solid", solid),
        ("@ad3-editing/jsx", "__ad3Authoring", jsx),
    ] {
        let keys = object.own_property_keys(context)?;
        context.register_global_property(js_string!(global), object, Attribute::empty())?;
        let code = keys
            .into_iter()
            .map(|key| format!("export const {key} = {global}.{key};"))
            .collect::<String>();
        let module = Module::parse(Source::from_bytes(&code), None, context)?;
        loader.hosts.borrow_mut().insert(specifier.into(), module);
    }
    for (name, callback) in [
        (
            "__ad3Jsx",
            crate::materialize::jsx as boa_engine::native_function::NativeFunctionPointer,
        ),
        ("__ad3Dynamic", crate::materialize::dynamic),
        ("__ad3MergeProps", crate::props::merge),
    ] {
        context.register_global_builtin_callable(
            js_string!(name),
            4,
            NativeFunction::from_fn_ptr(callback),
        )?;
    }
    Ok(())
}

impl LiveProject {
    pub fn mount(path: &Path, source: &str, offline: bool) -> Result<Self> {
        Self::mount_frozen(path, source, offline, None)
    }

    pub fn mount_frozen(
        path: &Path,
        source: &str,
        offline: bool,
        frozen: Option<BTreeMap<PathBuf, String>>,
    ) -> Result<Self> {
        Self::mount_frozen_cancellable(path, source, offline, frozen, None)
    }

    pub fn mount_frozen_cancellable(
        path: &Path,
        source: &str,
        offline: bool,
        frozen: Option<BTreeMap<PathBuf, String>>,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self> {
        crate::stack::runtime(|| Self::mount_inner(path, source, offline, frozen, cancel))
    }
    fn mount_inner(
        path: &Path,
        source: &str,
        offline: bool,
        frozen: Option<BTreeMap<PathBuf, String>>,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<Self> {
        crate::encoder::cancelled(cancel.as_deref())?;
        let path = fs::canonicalize(path)
            .or_else(|_| std::path::absolute(path))
            .map_err(|e| e.to_string())?;
        let loader = Rc::new(Loader {
            frozen,
            ..Loader::default()
        });
        let jobs = Rc::new(Jobs::default());
        let mut context = Context::builder()
            .module_loader(loader.clone())
            .job_executor(jobs.clone())
            .can_block(false)
            .build()
            .map_err(|e| e.to_string())?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(1_000_000);
        context.runtime_limits_mut().set_recursion_limit(128);
        let host = reactive::install(&mut context, offline);
        crate::inspect::install(&mut context);
        install_hosts(&loader, &mut context).map_err(|e| e.to_string())?;
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
                    "Module evaluation did not settle; asynchronous host work is not ported yet"
                        .into(),
                );
            }
        }
        let exported = module
            .namespace(&mut context)
            .get(js_string!("default"), &mut context)
            .map_err(|e| e.to_string())?;
        host.borrow_mut().mounted = true;
        // Construct the owner now so even a failed component mount runs cleanup.
        let mut project = Self {
            context,
            loader,
            jobs,
            host,
            rendered: JsValue::undefined(),
            last_time: None,
            warnings: Vec::new(),
            cancel,
        };
        project.rendered = if let Some(component) = exported.as_callable() {
            component
                .call(&JsValue::undefined(), &[], &mut project.context)
                .map_err(|e| e.to_string())?
        } else {
            exported
        };
        project.rendered =
            crate::control::children_value(project.rendered.clone(), &mut project.context)
                .map_err(|e| e.to_string())?;
        crate::materialize::root(&project.rendered, &mut project.context)
            .map_err(|e| e.to_string())?;
        reactive::finish_mount(&mut project.context).map_err(|e| e.to_string())?;
        project.settle()?;
        Ok(project)
    }

    fn settle(&mut self) -> Result<()> {
        use std::time::{Duration, Instant};
        let started = Instant::now();
        loop {
            crate::encoder::cancelled(self.cancel.as_deref())?;
            self.context.run_jobs().map_err(|e| e.to_string())?;
            reactive::flush(&mut self.context).map_err(|e| e.to_string())?;
            let holds = std::mem::take(&mut self.host.borrow_mut().holds);
            if holds.is_empty() {
                break;
            }
            let mut pending = Vec::new();
            for hold in holds {
                match hold.state() {
                    PromiseState::Fulfilled(_) => (),
                    PromiseState::Rejected(reason) => self.warnings.push(format!(
                        "Ticker hold rejected: {}",
                        reason
                            .to_string(&mut self.context)
                            .map_err(|e| e.to_string())?
                            .to_std_string_escaped()
                    )),
                    PromiseState::Pending => pending.push(hold),
                }
            }
            if started.elapsed() >= Duration::from_secs(30) {
                if !pending.is_empty() {
                    self.warnings.push(format!(
                        "{} ticker hold(s) timed out after 30 seconds",
                        pending.len()
                    ));
                }
                break;
            }
            self.host.borrow_mut().holds.extend(pending);
            if !self.host.borrow().holds.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    }

    pub fn sample(&mut self, time: f64, playing: bool) -> Result<Node> {
        self.sample_at_rate(time, playing, 30.0)
    }
    pub fn sample_at_rate(&mut self, time: f64, playing: bool, fps: f64) -> Result<Node> {
        crate::stack::runtime(|| self.sample_inner(time, playing, fps))
    }
    fn sample_inner(&mut self, time: f64, playing: bool, fps: f64) -> Result<Node> {
        if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
            return Err("FPS must be in (0, 240]".into());
        }
        self.jobs.count.set(0);
        self.jobs.exhausted.set(false);
        let delta = self.last_time.map_or(0.0, |previous| time - previous);
        reactive::tick_at_rate(time, delta, playing, fps, &mut self.context)
            .map_err(|e| e.to_string())?;
        self.last_time = Some(time);
        for _ in 0..100 {
            self.settle()?;
            let revision = self.host.borrow().revision;
            let root = crate::materialize::root(&self.rendered, &mut self.context)
                .map_err(|e| e.to_string())?;
            self.settle()?;
            if revision == self.host.borrow().revision {
                return Ok(root);
            }
        }
        Err("Project did not stabilize after 100 render passes".into())
    }

    pub fn inspect_controls(&mut self) -> Result<Vec<crate::inspect::Control>> {
        crate::stack::runtime(|| {
            crate::inspect::controls(&mut self.context).map_err(|e| e.to_string())
        })
    }
    pub fn set_inspect(
        &mut self,
        file: &Path,
        name: &str,
        value: Value,
        commit: bool,
    ) -> Result<()> {
        crate::stack::runtime(|| {
            crate::inspect::set(file, name, value, commit, &mut self.context)
                .map_err(|e| e.to_string())?;
            self.settle()
        })
    }

    pub fn sources(&self) -> BTreeMap<PathBuf, String> {
        self.loader.sources.borrow().clone()
    }
    pub fn uses_ticker(&self) -> bool {
        self.host.borrow().uses_ticker()
    }
}

impl Drop for LiveProject {
    fn drop(&mut self) {
        let root = self.host.borrow().root;
        let _ = reactive::dispose_owner(root, &mut self.context);
    }
}

pub fn evaluate(path: &Path, source: &str) -> Result<Evaluation> {
    let mut project = LiveProject::mount(path, source, false)?;
    let root = project.sample(0.0, false)?;
    Ok(Evaluation {
        root,
        sources: project.sources(),
    })
}

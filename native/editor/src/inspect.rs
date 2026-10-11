// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Binding-aware @inspect compilation and the native live inspector registry.
use crate::{Result, reactive};
use boa_engine::{Context, JsResult, JsValue, js_string, object::JsObject};
use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::*};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Declaration {
    pub file: PathBuf,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Control {
    #[serde(flatten)]
    pub declaration: Declaration,
    pub label: String,
    pub group: Vec<String>,
    pub initial: Value,
    pub value: Value,
    pub committed: Value,
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub declaration: Declaration,
    pub initial: Value,
    pub initializer: Range<usize>,
}

impl Declaration {
    pub fn validate(&self, value: &Value) -> Result<()> {
        let valid = match self.kind.as_str() {
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "boolean" => value.is_boolean(),
            "text" | "color" | "font" => value.is_string(),
            "select" => value
                .as_str()
                .is_some_and(|v| self.options.iter().any(|choice| choice == v)),
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(format!(
                "@inspect {} needs a valid {} value",
                self.name, self.kind
            ))
        }
    }
}

fn annotation(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let line = line.trim_start();
        let line = line.strip_prefix('*').unwrap_or(line).trim_start();
        let rest = line.strip_prefix("@inspect")?;
        if rest.starts_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
            None
        } else {
            Some(rest.trim())
        }
    })
}

fn number(value: &str) -> Result<f64> {
    // Use the VM's exact StringToNumber operation, including large radix values.
    let n = js_string!(value).to_number();
    if n.is_finite() {
        Ok(n)
    } else {
        Err("@inspect option must be finite".into())
    }
}

fn parse_annotation(file: &Path, line: &str) -> Result<Declaration> {
    let end = line
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(line.len());
    let kind = &line[..end];
    if !["number", "color", "text", "font", "boolean", "select"].contains(&kind) {
        return Err(
            "@inspect needs a control type: number, color, text, font, boolean or select".into(),
        );
    }
    let mut rest = line[end..].trim();
    let mut pairs = BTreeMap::<String, String>::new();
    while !rest.is_empty() {
        let key_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let key = &rest[..key_end];
        if key.is_empty() || !rest[key_end..].starts_with('=') {
            return Err("@inspect options must use key=value".into());
        }
        rest = &rest[key_end + 1..];
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').ok_or("Unclosed @inspect option quote")?;
            value = quoted[..end].to_owned();
            rest = &quoted[end + 1..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if end == 0 {
                return Err("Empty @inspect option".into());
            }
            value = rest[..end].to_owned();
            rest = &rest[end..];
        }
        if pairs.insert(key.to_owned(), value).is_some() {
            return Err(format!("@inspect option {key} is given twice"));
        }
        rest = rest.trim_start();
    }
    for key in pairs.keys() {
        if !["min", "max", "step", "path", "label", "options"].contains(&key.as_str()) {
            return Err(format!("Unknown @inspect option {key}"));
        }
    }
    if pairs.contains_key("path") && pairs.contains_key("label") {
        return Err("@inspect path already names the label".into());
    }
    let mut declaration = Declaration {
        file: file.into(),
        name: String::new(),
        kind: kind.into(),
        path: Vec::new(),
        min: None,
        max: None,
        step: None,
        options: Vec::new(),
    };
    for (key, target) in [
        ("min", &mut declaration.min),
        ("max", &mut declaration.max),
        ("step", &mut declaration.step),
    ] {
        if let Some(value) = pairs.get(key) {
            if kind != "number" {
                return Err(format!("@inspect {key} only applies to number"));
            }
            *target = Some(number(value)?);
        }
    }
    if let Some(path) = pairs.get("path").or_else(|| pairs.get("label")) {
        declaration.path = path
            .split('/')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if declaration.path.is_empty() {
            return Err("@inspect path/label is empty".into());
        }
    }
    if let Some(options) = pairs.get("options") {
        if kind != "select" {
            return Err("@inspect options only applies to select".into());
        }
        declaration.options = options
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
    }
    if kind == "select" && declaration.options.len() < 2 {
        return Err("@inspect select needs at least two options".into());
    }
    Ok(declaration)
}

fn literal(expression: &Expression<'_>) -> Option<Value> {
    match expression {
        Expression::NumericLiteral(value) => {
            serde_json::Number::from_f64(value.value).map(Value::Number)
        }
        Expression::BooleanLiteral(value) => Some(Value::Bool(value.value)),
        Expression::StringLiteral(value) => Some(Value::String(value.value.to_string())),
        Expression::UnaryExpression(value) if matches!(value.operator.as_str(), "+" | "-") => {
            let Expression::NumericLiteral(n) = &value.argument else {
                return None;
            };
            serde_json::Number::from_f64(if value.operator.as_str() == "-" {
                -n.value
            } else {
                n.value
            })
            .map(Value::Number)
        }
        _ => None,
    }
}

fn transform(file: &Path, source: &str, rewrite: bool) -> Result<(String, Vec<Binding>)> {
    if !source.contains("@inspect") {
        return Ok((source.into(), Vec::new()));
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        source,
        SourceType::from_path(file).map_err(|e| e.to_string())?,
    )
    .parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!("{file:?}: {:?}", parsed.diagnostics));
    }
    let built = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    if !built.diagnostics.is_empty() {
        return Err(format!("{file:?}: {:?}", built.diagnostics));
    }
    let semantic = built.semantic;
    let nodes = semantic.nodes();
    // Decoded semantic names catch escaped identifiers. Probe at most n+1
    // names instead of rescanning the complete source once per underscore.
    let reserved = nodes
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::BindingIdentifier(id) => Some(id.name.as_str()),
            AstKind::IdentifierReference(id) => Some(id.name.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let helper = (0..=reserved.len())
        .map(|index| format!("__ad3Inspect{index}"))
        .find(|name| !reserved.contains(name.as_str()))
        .ok_or("Cannot allocate inspect helper")?;
    let mut bindings = Vec::new();
    let mut patches: Vec<(Range<usize>, String)> = Vec::new();
    for node in nodes.iter() {
        let AstKind::VariableDeclaration(declaration) = node.kind() else {
            continue;
        };
        let parent = nodes.parent_kind(node.id());
        let mut annotation_line = None;
        for comment in &parsed.program.comments {
            if comment.is_leading()
                && (comment.attached_to == declaration.span.start
                    || matches!(parent, AstKind::ExportDeclaration(_))
                        && comment.attached_to == parent.span().start)
            {
                let span = comment.content_span();
                if let Some(line) = annotation(&source[span.start as usize..span.end as usize]) {
                    annotation_line = Some(line);
                    break;
                }
            }
        }
        let Some(line) = annotation_line else {
            continue;
        };
        let mut metadata = parse_annotation(file, line)?;
        if matches!(parent, AstKind::ExportDeclaration(_)) {
            return Err("An @inspect variable cannot be exported".into());
        }
        if !matches!(parent, AstKind::Program(_)) {
            return Err("@inspect only works on a top-level const".into());
        }
        if declaration.kind != VariableDeclarationKind::Const || declaration.declarations.len() != 1
        {
            return Err("@inspect annotates a single const declaration".into());
        }
        let variable = &declaration.declarations[0];
        let BindingPattern::BindingIdentifier(id) = &variable.id else {
            return Err("@inspect needs a plain name, not destructuring".into());
        };
        metadata.name = id.name.to_string();
        let initial_expression = variable
            .init
            .as_ref()
            .ok_or("@inspect needs a literal initializer")?;
        let initial =
            literal(initial_expression).ok_or("@inspect needs a plain literal initializer")?;
        metadata.validate(&initial)?;
        let span = initial_expression.span();
        let initializer = span.start as usize..span.end as usize;
        for reference in
            semantic.symbol_references(id.symbol_id.get().ok_or("Missing inspect binding")?)
        {
            let parent = nodes.parent_kind(reference.node_id());
            if matches!(parent, AstKind::ExportSpecifier(_)) && reference.is_value() {
                return Err("An @inspect variable cannot be exported".into());
            }
            if !reference.is_value()
                || reference.is_type()
                || reference.flags().is_value_as_type()
                || nodes
                    .ancestor_kinds(reference.node_id())
                    .any(|kind| matches!(kind, AstKind::TSTypeQuery(_)))
            {
                continue;
            }
            if reference.is_write() {
                return Err("An @inspect const cannot be assigned to".into());
            }
            if rewrite {
                let span = semantic.reference_span(reference);
                let replacement = if matches!(parent, AstKind::ObjectProperty(property) if property.shorthand)
                {
                    format!("{}: {}()", id.name, id.name)
                } else {
                    format!("{}()", id.name)
                };
                patches.push((span.start as usize..span.end as usize, replacement));
            }
        }
        if rewrite {
            patches.push((
                initializer.clone(),
                format!(
                    "{helper}({}, {})",
                    serde_json::to_string(&metadata).map_err(|e| e.to_string())?,
                    &source[initializer.clone()]
                ),
            ));
        }
        bindings.push(Binding {
            declaration: metadata,
            initial,
            initializer,
        });
    }
    if !rewrite || bindings.is_empty() {
        return Ok((source.into(), bindings));
    }
    patches.sort_by_key(|(range, _)| range.start);
    let mut output = String::new();
    let mut cursor = 0;
    for (range, replacement) in patches {
        if range.start < cursor {
            return Err("Overlapping @inspect rewrites".into());
        }
        output.push_str(&source[cursor..range.start]);
        output.push_str(&replacement);
        cursor = range.end;
    }
    output.push_str(&source[cursor..]);
    let at = if let Some(directive) = parsed.program.directives.last() {
        directive.span.end as usize
    } else if output.starts_with("#!") {
        output.find('\n').map_or(output.len(), |at| at + 1)
    } else {
        0
    };
    output.insert_str(
        at,
        &format!("\nimport {{ __inspect as {helper} }} from '@ad3-editing/jsx';\n"),
    );
    Ok((output, bindings))
}

pub fn bindings(file: &Path, source: &str) -> Result<Vec<Binding>> {
    transform(file, source, false).map(|(_, bindings)| bindings)
}
pub(crate) fn compile(file: &Path, source: &str) -> Result<String> {
    transform(file, source, true).map(|(code, _)| code)
}

struct Entry {
    declaration: Declaration,
    initial: Value,
    get: JsObject,
    set: JsObject,
    committed: Value,
}
// JsObject/Gc clones held outside Boa's traced heap are roots (ref_count exceeds
// non_root_count). This Context-owned Rust registry keeps the accessors alive.
// Every LiveProject owns a fresh Context and installs a fresh registry once;
// reloads never reuse the preceding mount's entries.
type Registry = Rc<RefCell<Vec<Entry>>>;
pub(crate) fn install(context: &mut Context) {
    context.insert_data::<Registry>(Rc::new(RefCell::new(Vec::new())));
}
fn registry(context: &Context) -> JsResult<Registry> {
    context
        .get_data::<Registry>()
        .cloned()
        .ok_or_else(|| reactive::error("Missing inspector registry"))
}

pub(crate) fn host(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if reactive::host(context)?.borrow().mounted {
        return Err(reactive::error(
            "@inspect declarations belong at module top level",
        ));
    }
    let metadata = args
        .first()
        .ok_or_else(|| reactive::error("Missing @inspect declaration"))?
        .to_json(context)?;
    let declaration: Declaration = serde_json::from_value(
        metadata.ok_or_else(|| reactive::error("Missing @inspect metadata"))?,
    )
    .map_err(|e| reactive::error(e.to_string()))?;
    let value = args.get(1).cloned().unwrap_or_default();
    let initial = value
        .to_json(context)?
        .ok_or_else(|| reactive::error("Missing @inspect value"))?;
    declaration.validate(&initial).map_err(reactive::error)?;
    let entries = registry(context)?;
    if entries.borrow().len() >= 10_000 {
        return Err(reactive::error("Too many @inspect declarations"));
    }
    if entries.borrow().iter().any(|entry| {
        entry.declaration.file == declaration.file && entry.declaration.name == declaration.name
    }) {
        return Err(reactive::error("Duplicate @inspect identity"));
    }
    let pair = reactive::create_signal(&JsValue::undefined(), &[value], context)?
        .as_object()
        .ok_or_else(|| reactive::error("Invalid inspect signal"))?;
    let get = reactive::callable(Some(&pair.get(0, context)?))?;
    let set = reactive::callable(Some(&pair.get(1, context)?))?;
    entries.borrow_mut().push(Entry {
        declaration,
        initial: initial.clone(),
        get: get.clone(),
        set,
        committed: initial,
    });
    Ok(get.into())
}

fn prettify(name: &str) -> String {
    let mut output = String::new();
    let mut previous = None;
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !output.ends_with(' ') {
                output.push(' ');
            }
        } else {
            if ch.is_ascii_uppercase()
                && previous.is_some_and(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            {
                output.push(' ');
            }
            output.push(ch);
        }
        previous = Some(ch);
    }
    let output = output.trim();
    let mut chars = output.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

pub(crate) fn controls(context: &mut Context) -> JsResult<Vec<Control>> {
    let entries = registry(context)?;
    let snapshot: Vec<_> = entries
        .borrow()
        .iter()
        .map(|e| {
            (
                e.declaration.clone(),
                e.initial.clone(),
                e.get.clone(),
                e.committed.clone(),
            )
        })
        .collect();
    snapshot
        .into_iter()
        .map(|(declaration, initial, get, committed)| {
            let value = reactive::untracked(context, |context| {
                get.call(&JsValue::undefined(), &[], context)
            })?
            .to_json(context)?
            .ok_or_else(|| reactive::error("Invalid @inspect value"))?;
            let label = declaration
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| prettify(&declaration.name));
            let group = declaration.path[..declaration.path.len().saturating_sub(1)].to_vec();
            Ok(Control {
                declaration,
                label,
                group,
                initial,
                value,
                committed,
            })
        })
        .collect()
}

pub(crate) fn set(
    file: &Path,
    name: &str,
    value: Value,
    commit: bool,
    context: &mut Context,
) -> JsResult<()> {
    let entries = registry(context)?;
    let (index, set) = {
        let entries = entries.borrow();
        let (index, entry) = entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.declaration.file == file && e.declaration.name == name)
            .ok_or_else(|| reactive::error("@inspect control not found"))?;
        entry
            .declaration
            .validate(&value)
            .map_err(reactive::error)?;
        (index, entry.set.clone())
    };
    let next = JsValue::from_json(&value, context)?;
    set.call(&JsValue::undefined(), &[next], context)?;
    if commit {
        entries.borrow_mut()[index].committed = value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiler_preserves_hashbang_and_directive_and_uses_a_unique_import() {
        let source = "#!/usr/bin/env node\n'use strict';\nconst __ad3Inspect0 = 0;\n/** @inspect number */ const padding = 24;\nconst object = { padding };\ntype T = typeof padding;\nconst local = (padding: number) => padding;";
        let output = compile(Path::new("index.ts"), source).unwrap();
        assert!(output.starts_with(
            "#!/usr/bin/env node\n'use strict';\nimport { __inspect as __ad3Inspect1 }"
        ));
        assert!(output.contains("'use strict';"));
        assert!(output.contains("{ padding: padding() }"));
        assert!(output.contains("type T = typeof padding;"));
        assert!(output.contains("(padding: number) => padding;"));
        assert!(number("0x10000000000000000").unwrap() > u64::MAX as f64 / 2.0);
        assert_eq!(number("\u{feff} 0x10 ").unwrap(), 16.0);
        assert!(number("inf").is_err());
    }

    #[test]
    fn long_identifier_prefix_does_not_expand_the_helper_name() {
        let source = format!(
            "const __ad3Inspect0{} = 0;\n/** @inspect number */\nconst x = 1; export default x;",
            "_".repeat(65_536)
        );
        let code = compile(Path::new("index.ts"), &source).unwrap();
        assert!(code.contains("import { __inspect as __ad3Inspect0 }"));
        assert!(code.len() < source.len() + 500);
    }
}

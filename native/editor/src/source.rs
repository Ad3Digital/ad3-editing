// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Parse actual TSX syntax with Oxc. Edits replace byte spans, preserving all
//! unrelated source, comments, UTF-8 text and formatting.
use crate::{
    Result,
    document::{Location, Node, TAGS},
    timing::{Timeline, snap},
};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

type Environment = BTreeMap<String, Value>;
type Replacement = (Range<usize>, String);

fn js_text(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => dragonbox_ecma::Buffer::new()
            .format(value.as_f64().unwrap())
            .to_owned(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                if value.is_null() {
                    String::new()
                } else {
                    js_text(value)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}
fn range(span: Span) -> Range<usize> {
    span.start as usize..span.end as usize
}

fn constant(expression: &Expression<'_>, env: &Environment) -> Result<Value> {
    let expression = expression.get_inner_expression();
    let unsupported = || {
        format!(
            "Dynamic expression at byte {} needs the native module evaluator (not yet ported)",
            expression.span().start
        )
    };
    match expression {
        Expression::NumericLiteral(n) => Ok(json!(n.value)),
        Expression::StringLiteral(s) => Ok(json!(s.value.as_str())),
        Expression::BooleanLiteral(b) => Ok(json!(b.value)),
        Expression::NullLiteral(_) => Ok(Value::Null),
        Expression::Identifier(id) => env.get(id.name.as_str()).cloned().ok_or_else(unsupported),
        Expression::UnaryExpression(unary) => {
            let value = constant(&unary.argument, env)?;
            match unary.operator.as_str() {
                "-" => value.as_f64().map(|n| json!(-n)).ok_or_else(unsupported),
                "+" => value.as_f64().map(|n| json!(n)).ok_or_else(unsupported),
                "!" => value.as_bool().map(|v| json!(!v)).ok_or_else(unsupported),
                _ => Err(unsupported()),
            }
        }
        Expression::BinaryExpression(binary) => {
            let mut left = constant(&binary.left, env)?;
            let mut right = constant(&binary.right, env)?;
            if binary.operator.as_str() == "+" {
                if left.is_array() || left.is_object() {
                    left = Value::String(js_text(&left));
                }
                if right.is_array() || right.is_object() {
                    right = Value::String(js_text(&right));
                }
            }
            if binary.operator.as_str() == "+" && (left.is_string() || right.is_string()) {
                return Ok(json!(js_text(&left) + &js_text(&right)));
            }
            let a = left.as_f64().ok_or_else(unsupported)?;
            let b = right.as_f64().ok_or_else(unsupported)?;
            let n = match binary.operator.as_str() {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" => a / b,
                "%" => a % b,
                _ => return Err(unsupported()),
            };
            if n.is_finite() {
                Ok(json!(n))
            } else {
                Err("Non-finite constant expression".into())
            }
        }
        Expression::ArrayExpression(array) => {
            let mut values = Vec::new();
            for item in &array.elements {
                values.push(constant(
                    item.as_expression().ok_or_else(unsupported)?,
                    env,
                )?);
            }
            Ok(Value::Array(values))
        }
        Expression::ObjectExpression(object) => {
            let mut map = Map::new();
            for property in &object.properties {
                let ObjectPropertyKind::ObjectProperty(property) = property else {
                    return Err(unsupported());
                };
                if property.method || property.computed || property.kind != PropertyKind::Init {
                    return Err(unsupported());
                }
                let key = property.key.static_name().ok_or_else(unsupported)?;
                map.insert(key.into_owned(), constant(&property.value, env)?);
            }
            Ok(Value::Object(map))
        }
        Expression::StaticMemberExpression(member) => {
            let object = constant(&member.object, env)?;
            object
                .get(member.property.name.as_str())
                .cloned()
                .ok_or_else(unsupported)
        }
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            let cooked = template
                .quasis
                .first()
                .and_then(|q| q.value.cooked.as_ref())
                .ok_or_else(unsupported)?;
            Ok(json!(cooked.as_str()))
        }
        _ => Err(unsupported()),
    }
}

fn variables(declaration: &VariableDeclaration<'_>, env: &mut Environment) -> Result<()> {
    if declaration.kind != VariableDeclarationKind::Const {
        return Err("Mutable project variables need the native module evaluator".into());
    }
    for declaration in &declaration.declarations {
        let BindingPattern::BindingIdentifier(id) = &declaration.id else {
            return Err("Destructuring needs the native module evaluator".into());
        };
        let value = constant(
            declaration.init.as_ref().ok_or("Uninitialized constant")?,
            env,
        )?;
        env.insert(id.name.to_string(), value);
    }
    Ok(())
}

fn returned<'a, 'b>(
    body: &'b FunctionBody<'a>,
    env: &mut Environment,
) -> Result<&'b Expression<'a>> {
    let mut result = None;
    for statement in &body.statements {
        if result.is_some() {
            return Err("Statements after return are not supported by the static reader".into());
        }
        match statement {
            Statement::VariableDeclaration(declaration) => variables(declaration, env)?,
            Statement::ReturnStatement(ret) => result = ret.argument.as_ref(),
            Statement::EmptyStatement(_) => (),
            _ => {
                return Err(format!(
                    "Statement at byte {} needs the native module evaluator",
                    statement.span().start
                ));
            }
        }
    }
    result.ok_or_else(|| "The project component must return <stage>".into())
}

fn element_expression<'a, 'b>(
    expression: &'b Expression<'a>,
    env: &mut Environment,
) -> Result<&'b JSXElement<'a>> {
    match expression.get_inner_expression() {
        Expression::JSXElement(element) => Ok(element),
        Expression::ArrowFunctionExpression(function) if !function.r#async && function.params.items.is_empty() => {
            match &function.body {
                ArrowFunctionBody::FunctionBody(body) => element_expression(returned(body, env)?, env),
                body => element_expression(body.as_expression().ok_or("Invalid arrow body")?, env),
            }
        }
        _ => Err("The static project reader expects a component returning a <stage>; module evaluation is pending".into()),
    }
}

fn parse_element(element: &JSXElement<'_>, env: &Environment, serial: &mut usize) -> Result<Node> {
    let JSXElementName::Identifier(name) = &element.opening_element.name else {
        return Err("Custom components need the native module evaluator".into());
    };
    let tag = name.name.as_str();
    if !TAGS.contains(&tag) {
        return Err(format!(
            "<{tag}> requires a component/HTML host that is not ported yet"
        ));
    }
    let mut props = Map::new();
    let mut attributes = BTreeMap::new();
    for attribute in &element.opening_element.attributes {
        let JSXAttributeItem::Attribute(attribute) = attribute else {
            return Err("Spread JSX attributes need the native module evaluator".into());
        };
        let JSXAttributeName::Identifier(name) = &attribute.name else {
            return Err("Namespaced composition attributes are unsupported".into());
        };
        let value = match &attribute.value {
            None => Value::Bool(true),
            Some(JSXAttributeValue::StringLiteral(value)) => {
                json!(html_escape::decode_html_entities(value.value.as_str()).as_ref())
            }
            Some(JSXAttributeValue::ExpressionContainer(container)) => constant(
                container
                    .expression
                    .as_expression()
                    .ok_or("Empty JSX property")?,
                env,
            )?,
            _ => return Err("Element-valued properties require module evaluation".into()),
        };
        if props.insert(name.name.to_string(), value).is_some() {
            return Err(format!("Duplicate property {}", name.name));
        }
        attributes.insert(name.name.to_string(), range(attribute.span));
    }
    *serial += 1;
    let id = match props.get("id") {
        Some(Value::String(id)) if !id.trim().is_empty() => id.clone(),
        Some(_) => return Err("Node id must be a nonempty string".into()),
        None => format!("native-node-{serial}"),
    };
    let mut node = Node {
        id,
        tag: tag.into(),
        props,
        text: String::new(),
        children: Vec::new(),
        location: Location {
            file: None,
            spread: false,
            element: range(element.span),
            name_end: name.span.end as usize,
            attributes,
        },
    };
    parse_children(&element.children, env, serial, &mut node)?;
    Ok(node)
}

fn parse_children(
    children: &[JSXChild<'_>],
    env: &Environment,
    serial: &mut usize,
    parent: &mut Node,
) -> Result<()> {
    for child in children {
        match child {
            JSXChild::Element(element) => {
                parent.children.push(parse_element(element, env, serial)?)
            }
            JSXChild::Fragment(fragment) => {
                parse_children(&fragment.children, env, serial, parent)?
            }
            JSXChild::Text(text) => {
                // JSX collapses indentation while preserving single-line spaces.
                let decoded = html_escape::decode_html_entities(text.value.as_str());
                let lines = decoded.split('\n').collect::<Vec<_>>();
                for (index, line) in lines.iter().enumerate() {
                    let line = line.trim_end_matches('\r').replace('\t', " ");
                    let line = if index > 0 { line.trim_start() } else { &line };
                    let line = if index + 1 < lines.len() {
                        line.trim_end()
                    } else {
                        line
                    };
                    if !line.is_empty() {
                        parent.text.push_str(line);
                        if index + 1 < lines.len()
                            && lines[index + 1..]
                                .iter()
                                .any(|line| !line.trim().is_empty())
                        {
                            parent.text.push(' ');
                        }
                    }
                }
            }
            JSXChild::ExpressionContainer(container) => {
                if let Some(expression) = container.expression.as_expression() {
                    match expression.get_inner_expression() {
                        Expression::JSXElement(element) => {
                            parent.children.push(parse_element(element, env, serial)?)
                        }
                        _ => match constant(expression, env)? {
                            Value::Null | Value::Bool(_) => (),
                            Value::String(text) => parent.text.push_str(&text),
                            Value::Number(n) => parent.text.push_str(&js_text(&Value::Number(n))),
                            _ => return Err("Non-text JSX child needs module evaluation".into()),
                        },
                    }
                }
            }
            _ => return Err("Dynamic JSX children need module evaluation".into()),
        }
    }
    if !parent.text.trim().is_empty() && !matches!(parent.tag.as_str(), "text" | "textRange") {
        return Err(format!(
            "Text is only valid inside <text> / <textRange>, found in <{}>",
            parent.tag
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SourceDocument {
    pub path: PathBuf,
    pub project_root: PathBuf,
    pub source: String,
    pub root: Node,
    /// Frozen source modules. Literal @inspect edits are supported; generated
    /// JSX instance writeback still requires source mapping and unrolling.
    pub module_sources: BTreeMap<PathBuf, String>,
}

impl SourceDocument {
    pub fn parse(path: PathBuf, source: String) -> Result<Self> {
        if source.contains("@inspect") && !crate::inspect::bindings(&path, &source)?.is_empty() {
            return Err("@inspect variables require the live native evaluator".into());
        }
        let absolute = std::path::absolute(&path).map_err(|e| e.to_string())?;
        let parent = absolute.parent().ok_or("Source has no parent directory")?;
        let project_root = fs::canonicalize(parent).unwrap_or_else(|_| parent.into());
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, SourceType::tsx()).parse();
        if !parsed.diagnostics.is_empty() {
            return Err(format!(
                "{}: {}",
                path.display(),
                parsed
                    .diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let mut env = Environment::new();
        let mut root = None;
        for statement in &parsed.program.body {
            match statement {
                Statement::VariableDeclaration(declaration) if root.is_none() => {
                    variables(declaration, &mut env)?
                }
                Statement::ExportDefaultDeclaration(export) if root.is_none() => {
                    let element = match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(function)
                            if !function.r#async
                                && !function.generator
                                && function.params.items.is_empty() =>
                        {
                            element_expression(
                                returned(
                                    function
                                        .body
                                        .as_ref()
                                        .ok_or("Project function has no body")?,
                                    &mut env,
                                )?,
                                &mut env,
                            )?
                        }
                        declaration => element_expression(
                            declaration
                                .as_expression()
                                .ok_or("Unsupported default export")?,
                            &mut env,
                        )?,
                    };
                    root = Some(parse_element(element, &env, &mut 0)?);
                }
                Statement::EmptyStatement(_)
                | Statement::TSInterfaceDeclaration(_)
                | Statement::TSTypeAliasDeclaration(_) => (),
                Statement::ImportDeclaration(import) if import.import_kind.is_type() => (),
                _ => {
                    return Err(format!(
                        "Module statement at byte {} needs the native module evaluator; source left untouched",
                        statement.span().start
                    ));
                }
            }
        }
        let root = root.ok_or("Project has no default export")?;
        if root.tag != "stage" {
            return Err("Project must return a <stage>".into());
        }
        let mut ids = BTreeSet::new();
        for node in root.nodes() {
            if !ids.insert(&node.id) {
                return Err(format!("Duplicate id {:?}", node.id));
            }
        }
        drop(parsed);
        Ok(Self {
            path,
            project_root,
            source,
            root,
            module_sources: BTreeMap::new(),
        })
    }

    pub fn open(project: &Path) -> Result<Self> {
        let path = if project.is_file() {
            project.to_path_buf()
        } else {
            let mut candidates = Vec::new();
            let package = project.join("package.json");
            if package.exists() {
                let value: Value =
                    serde_json::from_slice(&fs::read(&package).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                if let Some(main) = value.get("main").and_then(Value::as_str) {
                    candidates.push(project.join(main));
                }
            }
            candidates.extend(
                ["index.tsx", "index.ts", "index.jsx", "index.js"].map(|p| project.join(p)),
            );
            candidates
                .into_iter()
                .find(|p| p.is_file())
                .ok_or_else(|| format!("No project entry in {}", project.display()))?
        };
        let path = fs::canonicalize(&path).map_err(|e| e.to_string())?;
        let project_root = if project.is_dir() {
            fs::canonicalize(project).map_err(|e| e.to_string())?
        } else {
            path.parent()
                .ok_or("Source has no parent directory")?
                .into()
        };
        if !path.starts_with(&project_root) {
            return Err("Project entry must stay inside the project directory".into());
        }
        let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        match Self::parse(path.clone(), source.clone()) {
            Ok(mut document) => {
                document.project_root = project_root;
                Ok(document)
            }
            Err(_) => {
                let evaluation = crate::evaluator::evaluate(&path, &source)?;
                Ok(Self {
                    path,
                    project_root,
                    source,
                    root: evaluation.root,
                    module_sources: evaluation.sources,
                })
            }
        }
    }

    fn node(&self, id: &str) -> Result<&Node> {
        self.ensure_editable()?;
        self.root
            .find(id)
            .ok_or_else(|| format!("Node not found: {id}"))
    }

    pub fn set(&self, id: &str, name: &str, value: Option<Value>) -> Result<Self> {
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            || name.starts_with(|ch: char| ch.is_ascii_digit())
        {
            return Err("Invalid property name".into());
        }
        let mut patches = self.stamps();
        let node = self.node(id)?;
        if name == "id" {
            patches.retain(|(range, _)| range.start != node.location.name_end);
        }
        patches.push(property_patch(node, name, value.as_ref()));
        self.rewrite(patches)
    }

    pub fn split(&self, timeline: &Timeline, id: &str, at: f64) -> Result<Self> {
        let at = snap(at);
        let node = self.node(id)?;
        if !node.is_timed() || node.tag == "scene" {
            return Err("Select a timeline clip to split".into());
        }
        let span = timeline.get(id)?;
        if !at.is_finite() || at <= span.start || at >= span.end {
            return Err("Split must be strictly inside the clip".into());
        }
        let source_at = span.source_at(at);
        let parent_time = match &span.parent_id {
            Some(parent) => timeline.get(parent)?.source_at(at),
            None => at,
        };
        let mut left = Vec::new();
        let mut right = Vec::new();
        let used = self
            .root
            .nodes()
            .into_iter()
            .map(|n| n.id.clone())
            .collect::<BTreeSet<_>>();
        let mut remap = BTreeMap::new();
        for descendant in node.nodes() {
            if !descendant.props.contains_key("id") {
                left.push(property_patch(
                    descendant,
                    "id",
                    Some(&json!(descendant.id)),
                ));
            }
            let mut suffix = 2;
            let mut fresh = format!("{}-{suffix}", descendant.id);
            while used.contains(&fresh) || remap.values().any(|id| id == &fresh) {
                suffix += 1;
                fresh = format!("{}-{suffix}", descendant.id);
            }
            right.push(property_patch(descendant, "id", Some(&json!(fresh))));
            remap.insert(descendant.id.clone(), fresh);
        }
        for descendant in node.nodes() {
            if let Some(target) = descendant
                .string("syncTo")
                .and_then(|target| remap.get(target))
            {
                right.push(property_patch(descendant, "syncTo", Some(&json!(target))));
            }
        }
        left.push(property_patch(node, "end", None));
        left.push(property_patch(node, "sourceOut", Some(&json!(source_at))));
        right.push(property_patch(node, "start", Some(&json!(parent_time))));
        right.push(property_patch(node, "end", None));
        right.push(property_patch(node, "sourceIn", Some(&json!(source_at))));
        right.push(property_patch(
            node,
            "sourceOut",
            Some(&json!(span.source_out)),
        ));
        let slice = &self.source[node.location.element.clone()];
        let make = |patches: Vec<Replacement>| {
            apply(
                slice,
                patches
                    .into_iter()
                    .map(|(r, v)| {
                        (
                            r.start - node.location.element.start
                                ..r.end - node.location.element.start,
                            v,
                        )
                    })
                    .collect(),
            )
        };
        let left = make(left)?;
        let right = make(right)?;
        let line_start = self.source[..node.location.element.start]
            .rfind('\n')
            .map_or(0, |n| n + 1);
        let indent = &self.source[line_start..node.location.element.start];
        let indent = if indent.chars().all(char::is_whitespace) {
            indent
        } else {
            ""
        };
        let mut patches = self
            .stamps()
            .into_iter()
            .filter(|(r, _)| !node.location.element.contains(&r.start))
            .collect::<Vec<_>>();
        patches.push((
            node.location.element.clone(),
            format!("{left}\n{indent}{right}"),
        ));
        self.rewrite(patches)
    }

    pub fn trim(&self, timeline: &Timeline, id: &str, start: f64, end: f64) -> Result<Self> {
        let (start, end) = (snap(start), snap(end));
        let span = timeline.get(id)?;
        if !start.is_finite()
            || !end.is_finite()
            || start < span.start
            || end > span.end
            || start >= end
        {
            return Err("Trim window must lie inside the clip".into());
        }
        let node = self.node(id)?;
        if node.tag == "scene" {
            return Err("Use workarea to trim a scene".into());
        }
        let parent_time = match &span.parent_id {
            Some(parent) => timeline.get(parent)?.source_at(start),
            None => start,
        };
        let mut patches = self.stamps();
        patches.extend([
            property_patch(node, "start", Some(&json!(parent_time))),
            property_patch(node, "end", None),
            property_patch(node, "sourceIn", Some(&json!(span.source_at(start)))),
            property_patch(node, "sourceOut", Some(&json!(span.source_at(end)))),
        ]);
        self.rewrite(patches)
    }

    pub fn remove(&self, id: &str) -> Result<Self> {
        let node = self.node(id)?;
        if node.tag == "stage" {
            return Err("Cannot remove the stage".into());
        }
        let mut patches = self
            .stamps()
            .into_iter()
            .filter(|(r, _)| !node.location.element.contains(&r.start))
            .collect::<Vec<_>>();
        patches.push((node.location.element.clone(), String::new()));
        self.rewrite(patches)
    }

    pub fn move_clip(&self, timeline: &Timeline, id: &str, new_scene_start: f64) -> Result<Self> {
        let node = self.node(id)?;
        let span = timeline.get(id)?;
        if !node.is_timed() || node.tag == "scene" || !new_scene_start.is_finite() {
            return Err("Invalid clip move".into());
        }
        let start = snap(new_scene_start).max(0.0);
        let parent_rate = span
            .parent_id
            .as_ref()
            .map(|id| timeline.get(id))
            .transpose()?
            .map_or(1.0, |p| p.playback_rate);
        let delta = (start - span.start) * parent_rate;
        let old_start = node
            .props
            .get("start")
            .map(crate::timing::time_value)
            .transpose()?
            .unwrap_or(0.0);
        let mut patches = self.stamps();
        patches.push(property_patch(
            node,
            "start",
            Some(&json!(old_start + delta)),
        ));
        if let Some(end) = node.props.get("end") {
            patches.push(property_patch(
                node,
                "end",
                Some(&json!(crate::timing::time_value(end)? + delta)),
            ));
        }
        if let Some(parent) = span
            .parent_id
            .as_ref()
            .and_then(|id| self.root.find(id))
            .filter(|parent| parent.tag == "sequence")
        {
            let end = start + span.end - span.start;
            for sibling in parent
                .children
                .iter()
                .filter(|sibling| sibling.id != node.id && sibling.is_timed())
            {
                let sibling = timeline.get(&sibling.id)?;
                if start < sibling.end && end > sibling.start {
                    return Err("Move would overlap another clip in the sequence".into());
                }
            }
        }
        self.rewrite(patches)
    }

    pub fn activate_scene(&self, id: &str) -> Result<Self> {
        self.ensure_editable()?;
        let node = self
            .root
            .children
            .iter()
            .find(|node| node.id == id && node.tag == "scene")
            .ok_or("Scene not found")?;
        let mut patches = self.stamps();
        for scene in self.root.children.iter().filter(|node| node.tag == "scene") {
            patches.push(property_patch(
                scene,
                "active",
                if scene.id == node.id {
                    Some(&Value::Bool(true))
                } else {
                    None
                },
            ));
        }
        self.rewrite(patches)
    }

    fn stamps(&self) -> Vec<Replacement> {
        self.root
            .nodes()
            .into_iter()
            .filter(|node| !node.props.contains_key("id"))
            .map(|node| property_patch(node, "id", Some(&json!(node.id))))
            .collect()
    }
    fn rewrite(&self, patches: Vec<Replacement>) -> Result<Self> {
        self.ensure_editable()?;
        let mut next = Self::parse(self.path.clone(), apply(&self.source, patches)?)?;
        next.project_root = self.project_root.clone();
        Ok(next)
    }

    pub fn ensure_editable(&self) -> Result<()> {
        if !self.module_sources.is_empty() {
            return Err("This module graph is open for rendering. Native multi-file source mapping and generated-instance edits are still being ported; edit the TSX source and reload.".into());
        }
        Ok(())
    }

    /// Native writers serialize through an OS lock; source comparison also
    /// detects saves by the existing editor or an external text editor.
    pub fn save_over(&self, expected: &Self) -> Result<()> {
        if self.path != expected.path || self.project_root != expected.project_root {
            return Err("Source save target changed".into());
        }
        let snapshot = |document: &Self| -> Result<BTreeMap<PathBuf, String>> {
            if document.module_sources.is_empty() {
                return Ok(BTreeMap::from([(
                    document.path.clone(),
                    document.source.clone(),
                )]));
            }
            if document.module_sources.get(&document.path) != Some(&document.source) {
                return Err("Inconsistent entry source snapshot".into());
            }
            Ok(document.module_sources.clone())
        };
        let before = snapshot(expected)?;
        let after = snapshot(self)?;
        if before.keys().ne(after.keys()) {
            return Err("Source save cannot change the module graph".into());
        }
        let changes = after
            .iter()
            .filter(|(path, value)| before.get(*path) != Some(*value))
            .collect::<Vec<_>>();
        if changes.len() > 1 {
            return Err("A native source transaction currently writes one module at a time".into());
        }
        let root = fs::canonicalize(&self.project_root).map_err(|e| e.to_string())?;
        let state = root.join(".diffusion");
        fs::create_dir_all(&state).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("native-source.lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        for (path, source) in &before {
            if fs::read_to_string(path).map_err(|e| e.to_string())? != *source {
                return Err(format!(
                    "Project changed on disk: {}. Reload before applying this edit; no source was overwritten.",
                    path.display()
                ));
            }
        }
        let Some((target, source)) = changes.first().copied() else {
            return Ok(());
        };
        let canonical = crate::atomic_file::source_target(&root, target)?;
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temp = state.join(format!("native-save-{}-{suffix}.tmp", std::process::id()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(source.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            crate::atomic_file::replace(&temp, &canonical, before[target].as_bytes())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn property_patch(node: &Node, name: &str, value: Option<&Value>) -> Replacement {
    let text = value
        .map(|value| format!("{name}={{{value}}}"))
        .unwrap_or_default();
    match node.location.attributes.get(name) {
        Some(span) => (span.clone(), text),
        None => (
            node.location.name_end..node.location.name_end,
            if text.is_empty() {
                text
            } else {
                format!(" {text}")
            },
        ),
    }
}

fn apply(source: &str, mut patches: Vec<Replacement>) -> Result<String> {
    patches.sort_by_key(|(range, _)| (range.start, range.end));
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for (range, value) in patches {
        if range.start < cursor
            || range.end < range.start
            || range.end > source.len()
            || !source.is_char_boundary(range.start)
            || !source.is_char_boundary(range.end)
        {
            return Err("Overlapping or invalid source edits".into());
        }
        output.push_str(&source[cursor..range.start]);
        output.push_str(&value);
        cursor = range.end;
    }
    output.push_str(&source[cursor..]);
    Ok(output)
}

pub struct History {
    pub document: SourceDocument,
    undo: Vec<SourceDocument>,
    redo: Vec<SourceDocument>,
}
impl History {
    pub fn new(document: SourceDocument) -> Self {
        Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    pub fn commit(&mut self, next: SourceDocument) -> Result<()> {
        next.save_over(&self.document)?;
        self.undo.push(std::mem::replace(&mut self.document, next));
        self.redo.clear();
        Ok(())
    }
    pub fn undo(&mut self) -> Result<()> {
        self.restore(false, |_| Ok(()))
    }
    pub fn redo(&mut self) -> Result<()> {
        self.restore(true, |_| Ok(()))
    }
    pub fn restore<T>(
        &mut self,
        redo: bool,
        validate: impl FnOnce(&SourceDocument) -> Result<T>,
    ) -> Result<T> {
        let candidate = if redo {
            self.redo.last()
        } else {
            self.undo.last()
        }
        .ok_or("No history entry")?;
        let resolved = validate(candidate)?;
        candidate.save_over(&self.document)?;
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        to.push(std::mem::replace(&mut self.document, from.pop().unwrap()));
        Ok(resolved)
    }
}

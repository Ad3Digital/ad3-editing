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
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
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

#[derive(Clone, Debug)]
pub struct SourceClipboard {
    document: SourceDocument,
    roots: Vec<String>,
    parent: Option<String>,
}

impl SourceDocument {
    /// Compatible with the original compiler's project-relative file:locator.
    /// Unstamped generated IDs have no durable color key yet.
    pub fn source_key(&self, id: &str) -> Option<String> {
        let node = self.root.find(id)?;
        let authored = node.props.get("id")?.as_str()?;
        let file = node.location.file.as_deref().unwrap_or(&self.path);
        let relative = if file.is_absolute() {
            file.strip_prefix(&self.project_root).ok()?
        } else {
            file
        };
        Some(format!(
            "{}:{authored}",
            relative.to_string_lossy().replace('\\', "/")
        ))
    }

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

    pub fn set_text(&self, id: &str, text: &str) -> Result<Self> {
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        if !matches!(node.tag.as_str(), "text" | "textRange") {
            return Err("Select a text element".into());
        }
        let range = node.location.element.clone();
        let original = &draft.source[range.clone()];
        let children = node
            .children
            .iter()
            .map(|child| draft.source[child.location.element.clone()].to_owned())
            .collect::<Vec<_>>()
            .join("");
        let content = format!(
            "{{{}}}{children}",
            serde_json::to_string(text).map_err(|error| error.to_string())?
        );
        let replacement = if original.ends_with("/>") {
            format!(
                "{}>{content}</{}>",
                &original[..original.len() - 2],
                node.tag
            )
        } else {
            let attributes_end = node
                .location
                .attributes
                .values()
                .map(|range| range.end)
                .max()
                .unwrap_or(node.location.name_end);
            let opening_end = draft.source[attributes_end..range.end]
                .find('>')
                .ok_or("Missing text opening tag")?
                + attributes_end
                + 1;
            let close = original.rfind("</").ok_or("Missing text closing tag")?;
            format!(
                "{}{content}{}",
                &draft.source[range.start..opening_end],
                &original[close..]
            )
        };
        draft.rewrite(vec![(range, replacement)])
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

    pub fn stamp_ids(&self) -> Result<Self> {
        self.rewrite(self.stamps())
    }

    /// Synchronize an existing property's track at the target's source time.
    /// An ordinary property edit without a track does not create animation.
    pub fn nudge(
        &self,
        timeline: &Timeline,
        ids: &[String],
        at: f64,
        delta: [f64; 2],
    ) -> Result<Self> {
        self.ensure_editable()?;
        if !at.is_finite() || delta.iter().any(|value| !value.is_finite()) {
            return Err("Invalid nudge clock or distance".into());
        }
        let sampled = crate::motion::Motion::default().sample(&self.root, timeline, at, 30.0)?;
        let mut draft = self.clone();
        let mut seen = BTreeSet::new();
        for id in ids {
            let node = sampled
                .find(id)
                .ok_or_else(|| format!("Unknown node: {id}"))?;
            if !seen.insert(id)
                || !matches!(
                    node.tag.as_str(),
                    "scene"
                        | "group"
                        | "rect"
                        | "image"
                        | "video"
                        | "text"
                        | "captions"
                        | "html"
                        | "surface"
                )
            {
                continue;
            }
            for (axis, amount) in ["x", "y"].into_iter().zip(delta) {
                if amount == 0.0 {
                    continue;
                }
                let value = crate::timing::round_frame(node.number(axis, 0.0)? + amount);
                draft = draft.set_at_playhead(timeline, id, axis, json!(value), at)?;
            }
        }
        Ok(draft)
    }

    pub fn bake_container_transform(
        &self,
        timeline: &Timeline,
        id: &str,
        at: f64,
        child: crate::geometry::TransformValues,
        container: crate::geometry::Mat2D,
    ) -> Result<(Self, crate::geometry::TransformValues)> {
        self.ensure_editable()?;
        self.node(id)?;
        if !at.is_finite() {
            return Err("Invalid transform edit clock".into());
        }
        let baked = child.bake_container(container);
        if [
            container.a - 1.0,
            container.b,
            container.c,
            container.d - 1.0,
            container.e,
            container.f,
        ]
        .into_iter()
        .all(|value| value.abs() < 1e-6)
        {
            return Ok((self.clone(), baked));
        }
        if baked
            .position
            .into_iter()
            .chain(baked.scale)
            .chain(baked.skew)
            .chain([baked.rotation])
            .any(|value| !value.is_finite())
        {
            return Err("Container bake produced a non-finite transform".into());
        }
        let round = |value: f64, places: f64| crate::timing::round_frame(value * places) / places;
        let mut draft = self.clone();
        for (name, before, after, places) in [
            ("x", child.position[0], baked.position[0], 1.0),
            ("y", child.position[1], baked.position[1], 1.0),
            ("rotation", child.rotation, baked.rotation, 100.0),
        ] {
            if after != round(before, places) {
                draft = draft.set_at_playhead(timeline, id, name, json!(after), at)?;
            }
        }
        if baked.scale[0] != round(child.scale[0], 10000.0)
            || baked.scale[1] != round(child.scale[1], 10000.0)
        {
            if (baked.scale[0] - baked.scale[1]).abs() < 1e-6 {
                draft = draft.set(id, "scaleX", None)?.set(id, "scaleY", None)?;
                draft = draft.set_at_playhead(timeline, id, "scale", json!(baked.scale[0]), at)?;
                if baked.scale[0] == 1.0 {
                    draft = draft.set(id, "scale", None)?;
                }
            } else {
                draft = draft.set(id, "scale", None)?;
                draft = draft.set_at_playhead(timeline, id, "scaleX", json!(baked.scale[0]), at)?;
                draft = draft.set_at_playhead(timeline, id, "scaleY", json!(baked.scale[1]), at)?;
            }
        }
        Ok((draft, baked))
    }

    pub fn set_at_playhead(
        &self,
        timeline: &Timeline,
        id: &str,
        property: &str,
        value: Value,
        at: f64,
    ) -> Result<Self> {
        let node = self.node(id)?;
        let clock = self
            .root
            .nodes()
            .into_iter()
            .filter(|ancestor| ancestor.is_timed() && ancestor.find(id).is_some())
            .min_by_key(|ancestor| ancestor.location.element.len());
        let source_time = clock
            .map(|clock| timeline.get(&clock.id).map(|span| span.source_at(at)))
            .transpose()?;
        let tracked = node.children.iter().any(|child| {
            child.tag == "keyframeTrack" && child.string("property") == Some(property)
        });
        let next = self.set(id, property, Some(value.clone()))?;
        if tracked {
            next.sync_keyframe(
                id,
                property,
                value,
                source_time.ok_or("Keyframe target has no timed ancestor")?,
            )
        } else {
            Ok(next)
        }
    }

    pub fn toggle_keyframe_at_playhead(
        &self,
        timeline: &Timeline,
        id: &str,
        property: &str,
        value: Value,
        at: f64,
    ) -> Result<Self> {
        self.ensure_editable()?;
        if !at.is_finite() || !crate::motion::PROPERTIES.contains(&property) {
            return Err("Invalid keyframe clock or property".into());
        }
        let node = self.node(id)?;
        if node.tag == "sequence" {
            return Err("Sequence keyframe targeting is not implemented".into());
        }
        let clock = self
            .root
            .nodes()
            .into_iter()
            .filter(|ancestor| ancestor.is_timed() && ancestor.find(id).is_some())
            .min_by_key(|ancestor| ancestor.location.element.len())
            .ok_or("Keyframe target has no timed ancestor")?;
        let time = snap(timeline.get(&clock.id)?.source_at(at));
        let track = node.children.iter().find(|child| {
            child.tag == "keyframeTrack" && child.string("property") == Some(property)
        });
        if let Some(track) = track {
            if let Some(point) = track.children.iter().find(|point| {
                point.tag == "keyframe"
                    && point
                        .props
                        .get("time")
                        .and_then(|value| crate::timing::time_value(value).ok())
                        .is_some_and(|at| snap(at) == time)
            }) {
                return self.remove(if track.children.len() == 1 {
                    &track.id
                } else {
                    &point.id
                });
            }
            return self.sync_keyframe(id, property, value, time);
        }
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        let fresh = |prefix: &str| {
            let mut serial = 1;
            loop {
                let id = format!("{prefix}-{serial}");
                if draft.root.find(&id).is_none() {
                    break id;
                }
                serial += 1;
            }
        };
        let track = fresh("keyframe-track");
        let point = fresh("keyframe");
        let element = format!(
            "<keyframeTrack id={track:?} property={property:?}><keyframe id={point:?} time={{{time}}} value={{{value}}}/></keyframeTrack>"
        );
        let range = node.location.element.clone();
        let text = &draft.source[range.clone()];
        let replacement = if text.ends_with("/>") {
            format!("{}>\n{element}\n</{}>", &text[..text.len() - 2], node.tag)
        } else {
            let close = text
                .rfind("</")
                .ok_or("Missing keyframe target closing tag")?;
            format!("{}\n{element}\n{}", &text[..close], &text[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    pub fn keyframe_targets(&self, id: &str) -> Result<Vec<(String, String)>> {
        let root = self.node(id)?;
        let mut pending = vec![root];
        let mut tracks = Vec::new();
        while let Some(target) = pending.pop() {
            for track in target
                .children
                .iter()
                .filter(|child| child.tag == "keyframeTrack")
            {
                if let Some(property) = track.string("property") {
                    tracks.push((target.id.clone(), property.into()));
                }
            }
            pending.extend(
                target
                    .children
                    .iter()
                    .rev()
                    .filter(|child| !child.is_timed() && child.tag != "keyframeTrack"),
            );
        }
        Ok(tracks)
    }

    pub fn keyframe_points(
        &self,
        timeline: &Timeline,
        id: &str,
        property: &str,
    ) -> Result<Vec<(String, f64, Value)>> {
        let node = self.node(id)?;
        let Some(track) = node.children.iter().find(|child| {
            child.tag == "keyframeTrack" && child.string("property") == Some(property)
        }) else {
            return Ok(Vec::new());
        };
        let clock = self
            .root
            .nodes()
            .into_iter()
            .rev()
            .find(|ancestor| ancestor.is_timed() && ancestor.find(id).is_some())
            .ok_or("Keyframe target has no timed ancestor")?;
        let span = timeline.get(&clock.id)?;
        let mut points = track
            .children
            .iter()
            .filter(|child| child.tag == "keyframe")
            .map(|point| {
                let source_time = point
                    .props
                    .get("time")
                    .map(crate::timing::time_value)
                    .transpose()?
                    .unwrap_or(0.0);
                Ok((
                    point.id.clone(),
                    span.origin + snap(source_time) / span.playback_rate,
                    point.props.get("value").cloned().unwrap_or(Value::Null),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        points.sort_by(|a, b| a.1.total_cmp(&b.1));
        Ok(points)
    }

    pub fn remove_keyframe(&self, id: &str) -> Result<Self> {
        self.ensure_editable()?;
        if self.node(id)?.tag != "keyframe" {
            return Err("Select a keyframe to remove".into());
        }
        let track = self
            .root
            .nodes()
            .into_iter()
            .find(|node| {
                node.tag == "keyframeTrack" && node.children.iter().any(|point| point.id == id)
            })
            .ok_or("Keyframe has no parent track")?;
        self.remove(if track.children.len() == 1 {
            &track.id
        } else {
            id
        })
    }

    pub fn move_keyframe(&self, timeline: &Timeline, id: &str, scene_time: f64) -> Result<Self> {
        self.ensure_editable()?;
        if !scene_time.is_finite() || self.node(id)?.tag != "keyframe" {
            return Err("Select a keyframe and a finite scene time".into());
        }
        let clock = self
            .root
            .nodes()
            .into_iter()
            .rev()
            .find(|ancestor| ancestor.is_timed() && ancestor.find(id).is_some())
            .ok_or("Keyframe target has no timed ancestor")?;
        let source_time = timeline.get(&clock.id)?.source_at(scene_time);
        if !source_time.is_finite() {
            return Err("Keyframe source time overflows".into());
        }
        self.set(id, "time", Some(json!(snap(source_time.max(0.0)))))
    }

    pub fn move_keyframes(&self, timeline: &Timeline, ids: &[String], delta: f64) -> Result<Self> {
        self.ensure_editable()?;
        if !delta.is_finite() {
            return Err("Invalid keyframe displacement".into());
        }
        let mut draft = self.clone();
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id) {
                continue;
            }
            let point = self.node(id)?;
            if point.tag != "keyframe" {
                return Err("Select keyframes to move".into());
            }
            let clock = self
                .root
                .nodes()
                .into_iter()
                .rev()
                .find(|ancestor| ancestor.is_timed() && ancestor.find(id).is_some())
                .ok_or("Keyframe target has no timed ancestor")?;
            let span = timeline.get(&clock.id)?;
            let source_time = point
                .props
                .get("time")
                .map(crate::timing::time_value)
                .transpose()?
                .unwrap_or(0.0);
            let at = span.origin + snap(source_time) / span.playback_rate + delta;
            draft = draft.move_keyframe(timeline, id, at)?;
        }
        Ok(draft)
    }

    pub fn set_keyframe_easing(&self, id: &str, easing: &str) -> Result<Self> {
        self.ensure_editable()?;
        if self.node(id)?.tag != "keyframe" {
            return Err("Select a keyframe to change easing".into());
        }
        crate::easing::Easing::parse(easing)?;
        self.set(
            id,
            "easing",
            if easing == "linear" {
                None
            } else {
                Some(json!(easing))
            },
        )
    }

    pub fn remove_keyframe_track(&self, id: &str, property: &str) -> Result<Self> {
        self.ensure_editable()?;
        let node = self.node(id)?;
        match node.children.iter().find(|child| {
            child.tag == "keyframeTrack" && child.string("property") == Some(property)
        }) {
            Some(track) => self.remove(&track.id),
            None => Ok(self.clone()),
        }
    }

    pub fn sync_keyframe(
        &self,
        id: &str,
        property: &str,
        value: Value,
        source_time: f64,
    ) -> Result<Self> {
        self.ensure_editable()?;
        if !source_time.is_finite() {
            return Err("Invalid keyframe source time".into());
        }
        let node = self.node(id)?;
        let Some(track) = node.children.iter().find(|child| {
            child.tag == "keyframeTrack" && child.string("property") == Some(property)
        }) else {
            return Ok(self.clone());
        };
        let time = snap(source_time);
        if let Some(point) = track.children.iter().find(|point| {
            point.tag == "keyframe"
                && point
                    .props
                    .get("time")
                    .and_then(|time| crate::timing::time_value(time).ok())
                    .is_some_and(|at| snap(at) == time)
        }) {
            return self.set(&point.id, "value", Some(value));
        }
        let track_id = track.id.clone();
        let draft = self.stamp_ids()?;
        let track = draft.node(&track_id)?;
        let mut serial = 1;
        let point_id = loop {
            let candidate = format!("keyframe-{serial}");
            if draft.root.find(&candidate).is_none() {
                break candidate;
            }
            serial += 1;
        };
        let element = format!("<keyframe id={point_id:?} time={{{time}}} value={{{value}}}/>");
        let range = track.location.element.clone();
        let text = &draft.source[range.clone()];
        let replacement = if text.ends_with("/>") {
            format!("{}>\n{element}\n</keyframeTrack>", &text[..text.len() - 2])
        } else {
            let close = text
                .rfind("</")
                .ok_or("Missing keyframe track closing tag")?;
            format!("{}\n{element}\n{}", &text[..close], &text[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    pub fn move_animation(&self, id: &str, later: bool) -> Result<Self> {
        self.move_style_child(id, later, "animation")
    }

    pub fn move_effect(&self, id: &str, later: bool) -> Result<Self> {
        self.move_style_child(id, later, "effect")
    }

    pub fn move_paint(&self, id: &str, later: bool) -> Result<Self> {
        self.move_style_child(id, later, "paint")
    }

    pub fn move_outline_style(&self, id: &str, later: bool) -> Result<Self> {
        let tag = self.node(id)?.tag.as_str();
        if !matches!(tag, "stroke" | "shadow") {
            return Err("Target is not a stroke or shadow".into());
        }
        self.move_style_child(id, later, tag)
    }

    fn move_style_child(&self, id: &str, later: bool, tag: &str) -> Result<Self> {
        self.ensure_editable()?;
        let draft = self.stamp_ids()?;
        let parent = draft
            .root
            .nodes()
            .into_iter()
            .find(|parent| parent.children.iter().any(|child| child.id == id))
            .ok_or("Style parent not found")?;
        let animations = parent
            .children
            .iter()
            .filter(|child| {
                if tag == "paint" {
                    crate::paints::is_fill(&child.tag)
                } else {
                    child.tag == tag
                }
            })
            .collect::<Vec<_>>();
        let index = animations
            .iter()
            .position(|node| node.id == id)
            .ok_or("Target has a different style type")?;
        let neighbor = if later {
            index
                .checked_add(1)
                .filter(|index| *index < animations.len())
        } else {
            index.checked_sub(1)
        };
        let Some(neighbor) = neighbor else {
            return Ok(self.clone());
        };
        let first = animations[index].location.element.clone();
        let second = animations[neighbor].location.element.clone();
        let first_text = draft.source[first.clone()].to_string();
        let second_text = draft.source[second.clone()].to_string();
        draft.rewrite(vec![(first, second_text), (second, first_text)])
    }

    pub fn restack(&self, ids: &[String], front: bool) -> Result<Self> {
        self.ensure_editable()?;
        for id in ids {
            self.node(id)?;
        }
        let ordered = |parent: &Node| {
            let (moving, staying): (Vec<_>, Vec<_>) = parent
                .children
                .iter()
                .partition(|child| ids.contains(&child.id));
            if front {
                staying
                    .into_iter()
                    .chain(moving)
                    .map(|node| node.id.clone())
                    .collect::<Vec<_>>()
            } else {
                moving
                    .into_iter()
                    .chain(staying)
                    .map(|node| node.id.clone())
                    .collect::<Vec<_>>()
            }
        };
        let parents = self
            .root
            .nodes()
            .into_iter()
            .filter(|parent| {
                ordered(parent)
                    != parent
                        .children
                        .iter()
                        .map(|node| node.id.clone())
                        .collect::<Vec<_>>()
            })
            .map(|node| node.id.clone())
            .rev()
            .collect::<Vec<_>>();
        if parents.is_empty() {
            return Ok(self.clone());
        }
        let mut draft = self.stamp_ids()?;
        for id in parents {
            let parent = draft.node(&id)?;
            let order = ordered(parent);
            let patches = parent
                .children
                .iter()
                .zip(order)
                .filter_map(|(slot, id)| {
                    if slot.id == id {
                        return None;
                    }
                    let moved = parent.children.iter().find(|child| child.id == id).unwrap();
                    Some((
                        slot.location.element.clone(),
                        draft.source[moved.location.element.clone()].to_owned(),
                    ))
                })
                .collect();
            draft = draft.rewrite(patches)?;
        }
        Ok(draft)
    }

    pub fn dissolve_container(
        &self,
        timeline: &Timeline,
        id: &str,
        at: f64,
        geometry: &BTreeMap<String, crate::compositor::EditGeometry>,
    ) -> Result<(
        Self,
        Vec<String>,
        BTreeMap<String, crate::geometry::TransformValues>,
    )> {
        self.ensure_editable()?;
        let container = self.node(id)?;
        if !matches!(container.tag.as_str(), "group" | "sequence" | "scene") {
            return Err("Select a group, sequence or nested scene to dissolve".into());
        }
        let span = timeline.get(id)?;
        if container.tag == "scene" && span.parent_id.is_none() {
            return Err("A root scene cannot be dissolved into the stage".into());
        }
        let parent = span
            .parent_id
            .as_ref()
            .map(|id| timeline.get(id))
            .transpose()?;
        let matrix = if container.tag == "sequence" {
            crate::geometry::Mat2D::IDENTITY
        } else {
            geometry
                .get(id)
                .ok_or("Missing container frame geometry")?
                .transform
                .local_matrix()
        };
        let children = container
            .children
            .iter()
            .filter(|child| child.is_timed() && !child.flag("mask"))
            .map(|child| child.id.clone())
            .collect::<Vec<_>>();
        let mut draft = self.stamp_ids()?;
        let mut overrides = BTreeMap::new();
        for child_id in &children {
            let child = self.node(child_id)?;
            let mut leaves = vec![child];
            while let Some(leaf) = leaves.pop() {
                if leaf.tag == "sequence" {
                    leaves.extend(
                        leaf.children
                            .iter()
                            .filter(|child| child.is_timed() && !child.flag("mask")),
                    );
                } else if leaf.tag != "audio" {
                    let values = geometry
                        .get(&leaf.id)
                        .ok_or_else(|| format!("Missing frame geometry for {}", leaf.id))?
                        .transform;
                    let (next, baked) =
                        draft.bake_container_transform(timeline, &leaf.id, at, values, matrix)?;
                    draft = next;
                    overrides.insert(leaf.id.clone(), baked);
                }
            }
            let child_span = timeline.get(child_id)?;
            let start = parent.map_or(child_span.start, |parent| {
                parent.source_at(child_span.start)
            });
            let rate = child_span.playback_rate / parent.map_or(1.0, |parent| parent.playback_rate);
            draft = draft.set(child_id, "start", Some(json!(snap(start))))?;
            draft = draft.set(child_id, "end", None)?;
            draft = draft.set(child_id, "sourceIn", Some(json!(child_span.source_in)))?;
            draft = draft.set(child_id, "sourceOut", Some(json!(child_span.source_out)))?;
            draft = draft.set(child_id, "playbackRate", Some(json!(rate)))?;
        }
        let container = draft.node(id)?;
        let content = children
            .iter()
            .map(|id| {
                draft
                    .node(id)
                    .map(|child| draft.source[child.location.element.clone()].to_owned())
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n");
        Ok((
            draft.rewrite(vec![(container.location.element.clone(), content)])?,
            children,
            overrides,
        ))
    }

    pub fn wrap_scene(
        &self,
        timeline: &Timeline,
        ids: &[String],
        at: f64,
        geometry: &BTreeMap<String, crate::compositor::EditGeometry>,
    ) -> Result<(Self, Option<String>)> {
        if !at.is_finite() {
            return Err("Scene wrapping requires a finite playhead time".into());
        }
        let (mut draft, group) = self.group(ids)?;
        let Some(group) = group else {
            return Ok((draft, None));
        };
        let mut pending = draft.node(&group)?.children.iter().collect::<Vec<_>>();
        let mut measured = Vec::new();
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        while let Some(node) = pending.pop() {
            if !node.is_timed() || node.flag("mask") {
                continue;
            }
            if node.tag == "sequence" {
                pending.extend(node.children.iter());
                continue;
            }
            if node.tag == "audio" {
                continue;
            }
            let item = geometry
                .get(&node.id)
                .ok_or_else(|| format!("Missing frame geometry for {}", node.id))?;
            let rect = item.frame.bounds(item.transform.local_matrix());
            bounds[0] = bounds[0].min(rect.min_x);
            bounds[1] = bounds[1].min(rect.min_y);
            bounds[2] = bounds[2].max(rect.max_x);
            bounds[3] = bounds[3].max(rect.max_y);
            measured.push(node.id.clone());
        }
        if measured.is_empty() {
            return Ok((self.clone(), None));
        }
        if bounds.iter().any(|value| !value.is_finite()) {
            return Err("Scene bounds must be finite".into());
        }
        let corner = [bounds[0].floor(), bounds[1].floor()];
        let size = [
            (bounds[2].ceil() - corner[0]).max(1.0),
            (bounds[3].ceil() - corner[1]).max(1.0),
        ];
        let mut serial = 1;
        let id = loop {
            let candidate = format!("scene-{serial}");
            if draft.root.find(&candidate).is_none() {
                break candidate;
            }
            serial += 1;
        };
        let name = self
            .root
            .nodes()
            .iter()
            .filter_map(|node| {
                node.string("name")?
                    .strip_prefix("Scene ")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Scene name counter exhausted")?;
        let wrapper = draft.node(&group)?;
        let range = wrapper.location.element.clone();
        let text = &draft.source[range.clone()];
        let text = format!(
            "<scene{}scene>",
            &text["<group".len()..text.len() - "group>".len()]
        );
        draft = draft.rewrite(vec![(range, text)])?;
        draft = draft.set(&group, "id", Some(json!(id)))?;
        for (property, value) in [
            ("name", json!(format!("Scene {name}"))),
            ("x", json!(corner[0])),
            ("y", json!(corner[1])),
            ("width", json!(size[0])),
            ("height", json!(size[1])),
        ] {
            draft = draft.set(&id, property, Some(value))?;
        }
        if corner != [0.0, 0.0] {
            draft = draft.nudge(timeline, &measured, at, [-corner[0], -corner[1]])?;
        }
        Ok((draft, Some(id)))
    }

    pub fn wrap_sequence(
        &self,
        timeline: &Timeline,
        ids: &[String],
    ) -> Result<(Self, Option<String>)> {
        let (mut draft, group) = self.group(ids)?;
        let Some(group) = group else {
            return Ok((draft, None));
        };
        let mut serial = 1;
        let id = loop {
            let id = format!("sequence-{serial}");
            if draft.root.find(&id).is_none() {
                break id;
            }
            serial += 1;
        };
        let name = self
            .root
            .nodes()
            .iter()
            .filter_map(|node| {
                node.string("name")?
                    .strip_prefix("Sequence ")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Sequence name counter exhausted")?;
        let wrapper = draft.node(&group)?;
        let range = wrapper.location.element.clone();
        let text = &draft.source[range.clone()];
        let text = format!(
            "<sequence{}sequence>",
            &text["<group".len()..text.len() - "group>".len()]
        );
        draft = draft.rewrite(vec![(range, text)])?;
        draft = draft.set(&group, "name", Some(json!(format!("Sequence {name}"))))?;
        draft = draft.set(&group, "id", Some(json!(id)))?;
        let durations = self
            .root
            .nodes()
            .into_iter()
            .filter(|node| matches!(node.tag.as_str(), "video" | "audio"))
            .filter_map(|node| {
                timeline
                    .spans
                    .get(&node.id)
                    .map(|span| (node.id.clone(), span.source_out))
            })
            .collect::<BTreeMap<_, _>>();
        fn settle(
            mut draft: SourceDocument,
            durations: &BTreeMap<String, f64>,
            target: &str,
            occupied: [f64; 2],
        ) -> Result<SourceDocument> {
            let Some(node) = draft.root.find(target) else {
                return Ok(draft);
            };
            let timeline = Timeline::resolve(&draft.root, durations)?;
            let span = timeline.get(target)?;
            if span.end <= occupied[0] || span.start >= occupied[1] {
                return Ok(draft);
            }
            if span.start >= occupied[0] && span.end <= occupied[1] {
                return draft.remove(target);
            }
            if matches!(node.tag.as_str(), "group" | "sequence") {
                let children = node
                    .children
                    .iter()
                    .filter(|child| child.is_timed())
                    .map(|node| node.id.clone())
                    .collect::<Vec<_>>();
                for child in children {
                    draft = settle(draft, durations, &child, occupied)?;
                }
                if draft
                    .node(target)?
                    .children
                    .iter()
                    .all(|child| !child.is_timed())
                {
                    draft = draft.remove(target)?;
                }
                return Ok(draft);
            }
            if span.start >= occupied[0] {
                return draft.trim(&timeline, target, occupied[1], span.end);
            }
            if span.end <= occupied[1] {
                return draft.trim(&timeline, target, span.start, occupied[0]);
            }
            Err("Sequence overlap unexpectedly requires a middle split".into())
        }
        let resolved = Timeline::resolve(&draft.root, &durations)?;
        let mut children = draft
            .node(&id)?
            .children
            .iter()
            .filter(|child| child.is_timed())
            .map(|child| child.id.clone())
            .collect::<Vec<_>>();
        children.sort_by(|a, b| resolved.spans[a].start.total_cmp(&resolved.spans[b].start));
        for (index, authoritative) in children.iter().enumerate() {
            if draft.root.find(authoritative).is_none() {
                continue;
            }
            let timeline = Timeline::resolve(&draft.root, &durations)?;
            let span = timeline.get(authoritative)?;
            let occupied = [span.start, span.end];
            if occupied[1] <= occupied[0] {
                continue;
            }
            for target in &children[index + 1..] {
                draft = settle(draft, &durations, target, occupied)?;
            }
        }
        Ok((draft, Some(id)))
    }

    pub fn group(&self, ids: &[String]) -> Result<(Self, Option<String>)> {
        self.ensure_editable()?;
        for id in ids {
            self.node(id)?;
        }
        let ids = ids
            .iter()
            .filter(|id| !self.root.find(id).unwrap().flag("mask"))
            .cloned()
            .collect::<Vec<_>>();
        let nodes = self.root.nodes();
        let roots = ids
            .iter()
            .filter(|id| {
                !nodes.iter().any(|ancestor| {
                    ancestor.id != **id && ids.contains(&ancestor.id) && ancestor.find(id).is_some()
                })
            })
            .collect::<Vec<_>>();
        let Some(first) = roots.first() else {
            return Ok((self.clone(), None));
        };
        let Some(parent) = nodes
            .iter()
            .find(|node| node.children.iter().any(|child| child.id == ***first))
        else {
            return Ok((self.clone(), None));
        };
        let parent_id = parent.id.clone();
        let draft = self.stamp_ids()?;
        let parent = draft.node(&parent_id)?;
        let members = parent
            .children
            .iter()
            .filter(|node| roots.iter().any(|id| ***id == node.id))
            .collect::<Vec<_>>();
        let mut serial = 1;
        let name_serial = nodes
            .iter()
            .filter_map(|node| {
                let suffix = node.string("name")?.strip_prefix("Group ")?;
                if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                suffix.parse::<u64>().ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Group name counter exhausted")?;
        let name = format!("Group {name_serial}");
        let group = loop {
            let id = format!("group-{serial}");
            if draft.root.find(&id).is_none() {
                break id;
            }
            serial += 1;
        };
        let content = members
            .iter()
            .map(|node| draft.source[node.location.element.clone()].to_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let mut patches = members
            .iter()
            .map(|node| (node.location.element.clone(), String::new()))
            .collect::<Vec<_>>();
        patches[0].1 = format!("<group id={group:?} name={name:?}>\n{content}\n</group>");
        Ok((draft.rewrite(patches)?, Some(group)))
    }

    pub fn copy_selection(&self, ids: &[String]) -> Result<Option<SourceClipboard>> {
        self.ensure_editable()?;
        for id in ids {
            self.node(id)?;
        }
        let document = self.stamp_ids()?;
        let nodes = document.root.nodes();
        let mut roots = Vec::new();
        for id in ids {
            if id == &document.root.id || roots.contains(id) {
                continue;
            }
            if nodes.iter().any(|ancestor| {
                ancestor.id != *id && ids.contains(&ancestor.id) && ancestor.find(id).is_some()
            }) {
                continue;
            }
            roots.push(id.clone());
        }
        let Some(first) = roots.first() else {
            return Ok(None);
        };
        let parent = nodes
            .iter()
            .find(|node| node.children.iter().any(|child| &child.id == first))
            .map(|node| node.id.clone());
        Ok(Some(SourceClipboard {
            document,
            roots,
            parent,
        }))
    }

    pub fn paste_selection(
        &self,
        clipboard: &SourceClipboard,
        parent: &str,
        anchor: Option<&str>,
    ) -> Result<(Self, Vec<String>)> {
        self.ensure_editable()?;
        let draft = self.stamp_ids()?;
        let mut parent = draft.node(parent)?;
        if !parent.is_container() {
            return Err("Paste destination must be a container".into());
        }
        let mut anchor = anchor.map(str::to_owned);
        if parent.tag == "sequence"
            && self.path == clipboard.document.path
            && clipboard.parent.as_deref() == Some(&parent.id)
        {
            let outer = draft
                .root
                .nodes()
                .into_iter()
                .find(|node| node.children.iter().any(|child| child.id == parent.id))
                .ok_or("Copied sequence has no parent")?;
            let index = outer
                .children
                .iter()
                .position(|child| child.id == parent.id)
                .unwrap();
            anchor = outer.children.get(index + 1).map(|node| node.id.clone());
            parent = outer;
        }
        let mut used = draft
            .root
            .nodes()
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        let mut remap = BTreeMap::new();
        for id in &clipboard.roots {
            for node in clipboard.document.node(id)?.nodes() {
                let mut serial = 2;
                let fresh = loop {
                    let candidate = format!("{}-{serial}", node.id);
                    if used.insert(candidate.clone()) {
                        break candidate;
                    }
                    serial += 1;
                };
                remap.insert(node.id.clone(), fresh);
            }
        }
        let mut content = String::new();
        let mut copies = Vec::new();
        for id in &clipboard.roots {
            let root = clipboard.document.node(id)?;
            let start = root.location.element.start;
            let mut patches = Vec::new();
            for node in root.nodes() {
                patches.push(property_patch(node, "id", Some(&json!(remap[&node.id]))));
                for property in ["active", "selected"] {
                    if node.props.contains_key(property) {
                        patches.push(property_patch(node, property, None));
                    }
                }
                if let Some(target) = node.string("syncTo").and_then(|target| remap.get(target)) {
                    patches.push(property_patch(node, "syncTo", Some(&json!(target))));
                }
            }
            let text = apply(
                &clipboard.document.source[root.location.element.clone()],
                patches
                    .into_iter()
                    .map(|(range, value)| (range.start - start..range.end - start, value))
                    .collect(),
            )?;
            content.push_str(&format!("\n{text}"));
            copies.push(remap[id].clone());
        }
        if let Some(anchor) = anchor {
            let anchor = parent
                .children
                .iter()
                .find(|node| node.id == anchor)
                .ok_or("Paste anchor must be a child of its destination")?;
            let at = anchor.location.element.start;
            return Ok((draft.rewrite(vec![(at..at, content)])?, copies));
        }
        let range = parent.location.element.clone();
        let text = &draft.source[range.clone()];
        let replacement = if text.ends_with("/>") {
            format!("{}>{content}\n</{}>", &text[..text.len() - 2], parent.tag)
        } else {
            let close = text
                .rfind("</")
                .ok_or("Missing paste destination closing tag")?;
            format!("{}{content}\n{}", &text[..close], &text[close..])
        };
        Ok((draft.rewrite(vec![(range, replacement)])?, copies))
    }

    pub fn duplicate(&self, ids: &[String]) -> Result<(Self, Vec<String>)> {
        self.ensure_editable()?;
        for id in ids {
            self.node(id)?;
        }
        let draft = self.stamp_ids()?;
        let nodes = draft.root.nodes();
        let roots = nodes
            .iter()
            .copied()
            .filter(|node| {
                ids.contains(&node.id)
                    && node.id != draft.root.id
                    && !nodes.iter().any(|parent| {
                        parent.id != node.id
                            && ids.contains(&parent.id)
                            && parent.location.element.start <= node.location.element.start
                            && parent.location.element.end >= node.location.element.end
                    })
            })
            .collect::<Vec<_>>();
        let mut used = nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        let mut inserts = BTreeMap::<usize, String>::new();
        let mut copies = Vec::new();
        for node in roots {
            let mut below = node;
            while let Some(parent) = nodes
                .iter()
                .find(|parent| parent.children.iter().any(|child| child.id == below.id))
            {
                if parent.tag != "sequence" {
                    break;
                }
                below = parent;
            }
            let mut remap = BTreeMap::new();
            for child in node.nodes() {
                let mut suffix = 2;
                let fresh = loop {
                    let candidate = format!("{}-{suffix}", child.id);
                    if used.insert(candidate.clone()) {
                        break candidate;
                    }
                    suffix += 1;
                };
                remap.insert(child.id.clone(), fresh);
            }
            let mut patches = Vec::new();
            for child in node.nodes() {
                patches.push(property_patch(child, "id", Some(&json!(remap[&child.id]))));
                if child.props.contains_key("active") {
                    patches.push(property_patch(child, "active", None));
                }
                if child.props.contains_key("selected") {
                    patches.push(property_patch(child, "selected", None));
                }
                if let Some(target) = child.string("syncTo").and_then(|id| remap.get(id)) {
                    patches.push(property_patch(child, "syncTo", Some(&json!(target))));
                }
            }
            let start = node.location.element.start;
            let copy = apply(
                &draft.source[node.location.element.clone()],
                patches
                    .into_iter()
                    .map(|(range, value)| (range.start - start..range.end - start, value))
                    .collect(),
            )?;
            inserts
                .entry(below.location.element.end)
                .or_default()
                .push_str(&format!("\n{copy}"));
            copies.push(remap[&node.id].clone());
        }
        if copies.is_empty() {
            return Ok((self.clone(), copies));
        }
        Ok((
            draft.rewrite(
                inserts
                    .into_iter()
                    .map(|(at, text)| (at..at, text))
                    .collect(),
            )?,
            copies,
        ))
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

    fn playhead_units<'a>(
        &'a self,
        timeline: &Timeline,
        scene: &str,
        selected: &[String],
        at: f64,
    ) -> Result<Vec<&'a Node>> {
        self.ensure_editable()?;
        let scene_node = self.node(scene)?;
        if scene_node.tag != "scene" || !at.is_finite() {
            return Err("Playhead edit requires a scene and finite time".into());
        }
        let at = snap(at);
        if !at.is_finite() {
            return Err("Playhead exceeds the frame clock range".into());
        }
        fn collect<'a>(
            node: &'a Node,
            timeline: &Timeline,
            at: f64,
            units: &mut Vec<&'a Node>,
        ) -> Result<()> {
            if !node.is_timed() || !timeline.get(&node.id)?.visible(at) {
                return Ok(());
            }
            if node.tag == "sequence" {
                for child in &node.children {
                    collect(child, timeline, at, units)?;
                }
            } else {
                let span = timeline.get(&node.id)?;
                if at > span.start && at < span.end && !units.iter().any(|unit| unit.id == node.id)
                {
                    units.push(node);
                }
            }
            Ok(())
        }
        let targets = selected
            .iter()
            .filter(|id| id.as_str() != scene)
            .map(|id| self.node(id))
            .collect::<Result<Vec<_>>>()?;
        for target in &targets {
            if timeline.get(&target.id)?.scene_id != scene {
                return Err("Ripple trim selection must belong to the active scene".into());
            }
        }
        let mut units = Vec::new();
        if targets.is_empty() {
            for child in &scene_node.children {
                collect(child, timeline, at, &mut units)?;
            }
        } else {
            for target in targets {
                collect(target, timeline, at, &mut units)?;
            }
        }
        // A selected group is one editing unit; its descendants travel with it.
        let ids = units
            .iter()
            .map(|node| node.id.as_str())
            .collect::<BTreeSet<_>>();
        Ok(units
            .iter()
            .copied()
            .filter(|node| {
                let mut parent = timeline
                    .spans
                    .get(&node.id)
                    .and_then(|span| span.parent_id.as_ref());
                while let Some(id) = parent {
                    if ids.contains(id.as_str()) {
                        return false;
                    }
                    parent = timeline
                        .spans
                        .get(id)
                        .and_then(|span| span.parent_id.as_ref());
                }
                true
            })
            .collect())
    }

    pub fn split_at_playhead(
        &self,
        timeline: &Timeline,
        scene: &str,
        selected: &[String],
        at: f64,
    ) -> Result<(Self, Vec<String>)> {
        let units = self
            .playhead_units(timeline, scene, selected, at)?
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        let next = units.iter().try_fold(self.clone(), |document, id| {
            document.split(timeline, id, at)
        })?;
        Ok((next, units))
    }

    pub fn ripple_trim(
        &self,
        timeline: &Timeline,
        scene: &str,
        selected: &[String],
        at: f64,
        left: bool,
    ) -> Result<(Self, Vec<String>, f64)> {
        let units = self.playhead_units(timeline, scene, selected, at)?;
        let at = snap(at);
        if units.is_empty() {
            return Ok((self.clone(), Vec::new(), at));
        }
        let mut patches = self.stamps();
        let mut shifted = units
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        let mut playhead = at;
        if left {
            playhead = f64::NEG_INFINITY;
        }
        for node in &units {
            let span = timeline.get(&node.id)?;
            let parent = span
                .parent_id
                .as_ref()
                .map(|id| timeline.get(id))
                .transpose()?;
            let parent_start = parent.map_or(span.start, |parent| parent.source_at(span.start));
            patches.extend([
                property_patch(node, "start", Some(&json!(parent_start))),
                property_patch(node, "end", None),
                property_patch(
                    node,
                    "sourceIn",
                    Some(&json!(span.source_at(if left { at } else { span.start }))),
                ),
                property_patch(
                    node,
                    "sourceOut",
                    Some(&json!(span.source_at(if left { span.end } else { at }))),
                ),
            ]);
            let removed = if left { at - span.start } else { span.end - at };
            if left {
                playhead = playhead.max(span.start);
            }
            if let Some(sequence) = span
                .parent_id
                .as_ref()
                .and_then(|id| self.root.find(id))
                .filter(|node| node.tag == "sequence")
            {
                for follower in sequence.children.iter().filter(|node| node.is_timed()) {
                    let follower_span = timeline.get(&follower.id)?;
                    if follower_span.start < span.end || !shifted.insert(follower.id.clone()) {
                        continue;
                    }
                    let delta = -removed * parent.map_or(1.0, |parent| parent.playback_rate);
                    let start = follower
                        .props
                        .get("start")
                        .map(crate::timing::time_value)
                        .transpose()?
                        .unwrap_or(0.0);
                    patches.push(property_patch(
                        follower,
                        "start",
                        Some(&json!(start + delta)),
                    ));
                    if let Some(end) = follower.props.get("end") {
                        patches.push(property_patch(
                            follower,
                            "end",
                            Some(&json!(crate::timing::time_value(end)? + delta)),
                        ));
                    }
                }
            }
        }
        Ok((
            self.rewrite(patches)?,
            units.iter().map(|node| node.id.clone()).collect(),
            playhead,
        ))
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

    fn freeze_sync_placement(&self, timeline: &Timeline, id: &str) -> Result<Self> {
        let node = self.node(id)?;
        if !node.props.contains_key("syncTo") {
            return Ok(self.clone());
        }
        let span = timeline.get(id)?;
        let start = span
            .parent_id
            .as_ref()
            .map(|id| timeline.get(id))
            .transpose()?
            .map_or(span.start, |parent| parent.source_at(span.start));
        let mut patches = self.stamps();
        patches.extend([
            property_patch(node, "syncTo", None),
            property_patch(node, "start", Some(&json!(start))),
            property_patch(node, "end", None),
            property_patch(node, "sourceIn", Some(&json!(span.source_at(span.start)))),
            property_patch(node, "sourceOut", Some(&json!(span.source_at(span.end)))),
        ]);
        self.rewrite(patches)
    }

    pub fn move_clip(&self, timeline: &Timeline, id: &str, new_scene_start: f64) -> Result<Self> {
        let node = self.node(id)?;
        let span = timeline.get(id)?;
        if !node.is_timed() || node.tag == "scene" || !new_scene_start.is_finite() {
            return Err("Invalid clip move".into());
        }
        let start = snap(new_scene_start).max(0.0);
        if start != span.start
            && node
                .nodes()
                .iter()
                .any(|node| node.props.contains_key("syncTo"))
        {
            return self.move_clips(timeline, &[id.into()], start - span.start);
        }
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

    pub fn move_clips(&self, timeline: &Timeline, ids: &[String], delta: f64) -> Result<Self> {
        self.move_clips_inner(timeline, ids, delta, true)
    }

    fn move_clips_inner(
        &self,
        timeline: &Timeline,
        ids: &[String],
        delta: f64,
        check_collisions: bool,
    ) -> Result<Self> {
        self.ensure_editable()?;
        let delta = snap(delta);
        if !delta.is_finite() {
            return Err("Invalid clip move".into());
        }
        let mut moving = BTreeSet::new();
        for id in ids {
            let node = self.node(id)?;
            if !node.is_timed() || matches!(node.tag.as_str(), "scene" | "stage") {
                return Err("Select timeline clips to move".into());
            }
            moving.insert(id.clone());
        }
        // Moving a container already carries its children. Keep their offsets.
        let roots = moving
            .iter()
            .filter(|id| {
                let mut parent = timeline
                    .spans
                    .get(*id)
                    .and_then(|span| span.parent_id.as_ref());
                while let Some(id) = parent {
                    if moving.contains(id) {
                        return false;
                    }
                    parent = timeline
                        .spans
                        .get(id)
                        .and_then(|span| span.parent_id.as_ref());
                }
                true
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let minimum = roots
            .iter()
            .map(|id| timeline.get(id).map(|span| span.start))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .reduce(f64::min)
            .unwrap_or(0.0);
        let delta = delta.max(-minimum);
        if delta != 0.0 {
            let linked = roots
                .iter()
                .flat_map(|id| self.root.find(id).unwrap().nodes())
                .filter(|node| node.props.contains_key("syncTo"))
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();
            if !linked.is_empty() {
                let mut draft = self.clone();
                for id in linked {
                    draft = draft.freeze_sync_placement(timeline, &id)?;
                }
                return draft.move_clips_inner(timeline, ids, delta, check_collisions);
            }
        }
        let mut patches = self.stamps();
        for id in &roots {
            let node = self.node(id)?;
            let span = timeline.get(id)?;
            let parent = span
                .parent_id
                .as_ref()
                .map(|id| timeline.get(id))
                .transpose()?;
            let local_delta = delta * parent.map_or(1.0, |parent| parent.playback_rate);
            let start = node
                .props
                .get("start")
                .map(crate::timing::time_value)
                .transpose()?
                .unwrap_or(0.0);
            patches.push(property_patch(
                node,
                "start",
                Some(&json!(start + local_delta)),
            ));
            if let Some(end) = node.props.get("end") {
                patches.push(property_patch(
                    node,
                    "end",
                    Some(&json!(crate::timing::time_value(end)? + local_delta)),
                ));
            }
            if check_collisions
                && let Some(sequence) = span
                    .parent_id
                    .as_ref()
                    .and_then(|id| self.root.find(id))
                    .filter(|node| node.tag == "sequence")
            {
                for sibling in sequence
                    .children
                    .iter()
                    .filter(|node| node.is_timed() && node.id != *id)
                {
                    let sibling_span = timeline.get(&sibling.id)?;
                    let sibling_delta = if roots.contains(&sibling.id) {
                        delta
                    } else {
                        0.0
                    };
                    if span.start + delta < sibling_span.end + sibling_delta
                        && span.end + delta > sibling_span.start + sibling_delta
                    {
                        return Err("Move would overlap another clip in the sequence".into());
                    }
                }
            }
        }
        if roots.is_empty() || delta == 0.0 {
            return Ok(self.clone());
        }
        self.rewrite(patches)
    }

    pub fn sequence_gaps(&self, timeline: &Timeline, sequence: &str) -> Result<Vec<[f64; 2]>> {
        let node = self.node(sequence)?;
        if node.tag != "sequence" {
            return Err("Gap selection requires a sequence".into());
        }
        let mut spans = node
            .children
            .iter()
            .filter(|node| node.is_timed())
            .map(|node| timeline.get(&node.id))
            .collect::<Result<Vec<_>>>()?;
        spans.retain(|span| span.end > span.start);
        spans.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut gaps = Vec::new();
        let mut end = spans.first().map_or(0.0, |span| span.end);
        for span in spans.iter().skip(1) {
            if span.start > end {
                gaps.push([end, span.start]);
            }
            end = end.max(span.end);
        }
        Ok(gaps)
    }

    pub fn close_gap(
        &self,
        timeline: &Timeline,
        scene: &str,
        sequence: &str,
        gap: [f64; 2],
    ) -> Result<Self> {
        let scene_node = self.node(scene)?;
        if scene_node.tag != "scene" || timeline.get(sequence)?.scene_id != scene {
            return Err("Gap must belong to the active scene".into());
        }
        if !self.sequence_gaps(timeline, sequence)?.contains(&gap) {
            return Err("Gap coordinates are stale or do not describe an empty interval".into());
        }
        fn collect(
            node: &Node,
            timeline: &Timeline,
            end: f64,
            followers: &mut Vec<String>,
        ) -> Result<()> {
            for child in node.children.iter().filter(|node| node.is_timed()) {
                if timeline.get(&child.id)?.start >= end {
                    followers.push(child.id.clone());
                } else {
                    collect(child, timeline, end, followers)?;
                }
            }
            Ok(())
        }
        let mut followers = Vec::new();
        collect(scene_node, timeline, gap[1], &mut followers)?;
        // The existing gap action ripples every later scene row, including
        // rows with crossing clips; sequence collision checks apply to drags.
        self.move_clips_inner(timeline, &followers, gap[0] - gap[1], false)
    }

    pub fn insert_media(
        &self,
        scene: &str,
        kind: &str,
        source: &str,
        at: f64,
    ) -> Result<(Self, String)> {
        self.ensure_editable()?;
        if !matches!(kind, "image" | "video" | "audio")
            || source.trim().is_empty()
            || !at.is_finite()
            || at < 0.0
        {
            return Err("Media insertion requires an image, video or audio path and a finite nonnegative time".into());
        }
        let mut serial = 1;
        while self.root.find(&format!("{kind}-{serial}")).is_some() {
            serial += 1;
        }
        let id = format!("{kind}-{serial}");
        let base = Path::new(source)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(kind);
        let prefix = format!("{base} ");
        let mut nodes = Vec::new();
        self.root.walk(&mut nodes);
        let number = nodes
            .iter()
            .filter_map(|node| {
                node.string("name")?
                    .strip_prefix(&prefix)?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Media name numbering exhausted")?;
        let name = serde_json::to_string(&format!("{base} {number}"))
            .map_err(|error| error.to_string())?;
        let draft = self.stamp_ids()?;
        let parent = draft.node(scene)?;
        if parent.tag != "scene" {
            return Err("Media insertion requires a scene".into());
        }
        let visual = if kind == "audio" {
            String::new()
        } else {
            format!(
                " width={{{}}} height={{{}}} objectFit=\"contain\"",
                parent.number("width", 1920.0)?,
                parent.number("height", 1080.0)?
            )
        };
        let content = format!(
            "\n<{kind} id=\"{id}\" name={{{name}}} start={{{at}}} src={{{}}}{visual} />\n",
            serde_json::to_string(source).map_err(|error| error.to_string())?
        );
        let range = parent.location.element.clone();
        let text = &draft.source[range.clone()];
        let replacement = if text.ends_with("/>") {
            format!("{}>{content}</{}>", &text[..text.len() - 2], parent.tag)
        } else {
            let close = text.rfind("</").ok_or("Missing scene closing tag")?;
            format!("{}{content}{}", &text[..close], &text[close..])
        };
        Ok((draft.rewrite(vec![(range, replacement)])?, id))
    }

    pub fn remove_intrinsic_media(&self, timeline: &Timeline, id: &str) -> Result<Self> {
        self.ensure_editable()?;
        let node = self.node(id)?;
        if !matches!(node.tag.as_str(), "image" | "video") {
            return Err("Only intrinsic image or video media can become a rect".into());
        }
        let span = timeline.get(id)?;
        let parent = span
            .parent_id
            .as_deref()
            .map(|id| timeline.get(id))
            .transpose()?;
        let local = |time| parent.map_or(time, |parent| parent.source_at(time));
        let mut document = self.stamp_ids()?;
        if !node.props.contains_key("start") {
            document = document.set(id, "start", Some(json!(local(span.start))))?;
        }
        if !node.props.contains_key("end") {
            document = document.set(id, "end", Some(json!(local(span.end))))?;
        }
        for property in [
            "src",
            "error",
            "objectFit",
            "frameRate",
            "sourceIn",
            "sourceOut",
            "upscale",
            "removeBackground",
            "addAudio",
            "volume",
            "muted",
            "syncTo",
        ] {
            document = document.set(id, property, None)?;
        }
        let node = document.node(id)?;
        let mut patches = vec![(
            node.location.name_end - node.tag.len()..node.location.name_end,
            "rect".into(),
        )];
        let element = &document.source[node.location.element.clone()];
        if !element.ends_with("/>") {
            let close = element.rfind("</").ok_or("Missing media closing tag")?
                + node.location.element.start
                + 2;
            patches.push((close..close + node.tag.len(), "rect".into()));
        }
        document.rewrite(patches)
    }

    pub fn insert_media_centered(
        &self,
        scene: &str,
        kind: &str,
        source: &str,
        at: f64,
        dimensions: (u32, u32),
    ) -> Result<(Self, String)> {
        let (width, height) = dimensions;
        if width == 0 || height == 0 {
            return Err("Media dimensions must be positive".into());
        }
        let parent = self.node(scene)?;
        let x = ((parent.number("width", 1920.0)? - f64::from(width)) / 2.0).round();
        let y = ((parent.number("height", 1080.0)? - f64::from(height)) / 2.0).round();
        let (mut document, id) = self.insert_media(scene, kind, source, at)?;
        for (property, value) in [
            ("width", f64::from(width)),
            ("height", f64::from(height)),
            ("x", x),
            ("y", y),
        ] {
            document = document.set(&id, property, Some(serde_json::json!(value)))?;
        }
        Ok((document, id))
    }

    pub fn insert_shadow(&self, id: &str) -> Result<Self> {
        self.insert_visual_style(
            id,
            "shadow",
            "color=\"#000000\" opacity={0.5} blur={12} offsetX={4} offsetY={4}",
        )
    }

    pub fn insert_stroke(&self, id: &str) -> Result<Self> {
        self.insert_visual_style(id, "stroke", "color=\"#ffffff\" width={2} opacity={1}")
    }

    pub fn insert_solid_paint(&self, id: &str) -> Result<Self> {
        self.insert_visual_style(id, "solidPaint", "color=\"#5b8cff\" opacity={1}")
    }

    pub fn insert_mask(&self, id: &str, size: [f64; 2]) -> Result<(Self, String)> {
        if size
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0 || *value > 1_000_000.0)
        {
            return Err("Invalid mask dimensions".into());
        }
        let serial = self
            .root
            .nodes()
            .into_iter()
            .filter_map(|node| {
                node.string("name")?
                    .strip_prefix("Mask ")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Mask name counter exhausted")?;
        let document = self.insert_visual_style(
            id,
            "rect",
            &format!(
                "mask name=\"Mask {serial}\" x={{20}} y={{20}} width={{{}}} height={{{}}}",
                size[0].round(),
                size[1].round()
            ),
        )?;
        let mask = document
            .node(id)?
            .children
            .last()
            .ok_or("Inserted mask missing")?
            .id
            .clone();
        Ok((document, mask))
    }

    pub fn insert_media_paint(&self, id: &str, video: bool, source: &str) -> Result<Self> {
        if source.trim().is_empty() {
            return Err("Media source cannot be empty".into());
        }
        let attributes = format!(
            "src={{{}}} opacity={{1}} objectFit=\"cover\"",
            serde_json::to_string(source).map_err(|error| error.to_string())?
        );
        self.insert_visual_style(
            id,
            if video { "videoPaint" } else { "imagePaint" },
            &attributes,
        )
    }

    pub fn insert_gradient_paint(&self, id: &str, radial: bool) -> Result<Self> {
        self.insert_visual_style(
            id,
            if radial {
                "radialGradientPaint"
            } else {
                "linearGradientPaint"
            },
            "opacity={1}",
        )
    }

    pub fn insert_gradient_stop(&self, id: &str, offset: f64, color: &str) -> Result<Self> {
        self.ensure_editable()?;
        if !offset.is_finite()
            || !(0.0..=1.0).contains(&offset)
            || crate::color::parse(color).is_none()
        {
            return Err("Invalid gradient stop position or color".into());
        }
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        if !matches!(
            node.tag.as_str(),
            "linearGradientPaint" | "radialGradientPaint"
        ) {
            return Err("Color stops require a gradient".into());
        }
        let mut serial = 1;
        while draft.root.find(&format!("colorStop-{serial}")).is_some() {
            serial += 1;
        }
        let content = format!(
            "\n<colorStop id=\"colorStop-{serial}\" offset={{{offset}}} color={{{}}} opacity={{1}}/>\n",
            serde_json::to_string(color).map_err(|error| error.to_string())?
        );
        let range = node.location.element.clone();
        let original = &draft.source[range.clone()];
        let replacement = if original.ends_with("/>") {
            format!(
                "{}>{content}</{}>",
                &original[..original.len() - 2],
                node.tag
            )
        } else {
            let close = original.rfind("</").ok_or("Missing gradient closing tag")?;
            format!("{}{content}{}", &original[..close], &original[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    fn insert_visual_style(&self, id: &str, tag: &str, attributes: &str) -> Result<Self> {
        self.ensure_editable()?;
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        if !matches!(
            node.tag.as_str(),
            "scene" | "group" | "rect" | "image" | "video" | "text" | "textRange"
        ) {
            return Err("Style requires a visual holder or text range".into());
        }
        let mut serial = 1;
        while [
            format!("{tag}-{serial}"),
            format!("{tag}-{serial}-start"),
            format!("{tag}-{serial}-end"),
        ]
        .iter()
        .any(|id| draft.root.find(id).is_some())
        {
            serial += 1;
        }
        let content = if matches!(tag, "linearGradientPaint" | "radialGradientPaint") {
            format!(
                "\n<{tag} id=\"{tag}-{serial}\" {attributes}><colorStop id=\"{tag}-{serial}-start\" offset={{0}} color=\"#ffffff\"/><colorStop id=\"{tag}-{serial}-end\" offset={{1}} color=\"#000000\"/></{tag}>\n"
            )
        } else {
            format!("\n<{tag} id=\"{tag}-{serial}\" {attributes} />\n")
        };
        let range = node.location.element.clone();
        let original = &draft.source[range.clone()];
        let replacement = if original.ends_with("/>") {
            format!(
                "{}>{content}</{}>",
                &original[..original.len() - 2],
                node.tag
            )
        } else {
            let close = original
                .rfind("</")
                .ok_or("Missing shadow holder closing tag")?;
            format!("{}{content}{}", &original[..close], &original[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    pub fn insert_effect(&self, id: &str, kind: &str, value: f64) -> Result<Self> {
        self.ensure_editable()?;
        if !matches!(
            kind,
            "blur"
                | "brightness"
                | "contrast"
                | "saturate"
                | "grayscale"
                | "invert"
                | "sepia"
                | "hueRotate"
        ) || !value.is_finite()
        {
            return Err("Invalid effect type or value".into());
        }
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        if !node.is_timed() || matches!(node.tag.as_str(), "stage" | "sequence" | "audio") {
            return Err("Effects require a visual holder".into());
        }
        let mut serial = 1;
        while draft.root.find(&format!("effect-{serial}")).is_some() {
            serial += 1;
        }
        let content =
            format!("\n<effect id=\"effect-{serial}\" type=\"{kind}\" value={{{value}}} />\n");
        let range = node.location.element.clone();
        let original = &draft.source[range.clone()];
        let replacement = if original.ends_with("/>") {
            format!(
                "{}>{content}</{}>",
                &original[..original.len() - 2],
                node.tag
            )
        } else {
            let close = original
                .rfind("</")
                .ok_or("Missing effect holder closing tag")?;
            format!("{}{content}{}", &original[..close], &original[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    pub fn change_animation_preset(&self, id: &str, kind: &str) -> Result<Self> {
        self.ensure_editable()?;
        let node = self.node(id)?;
        if node.tag != "animation" {
            return Err("Target is not an animation".into());
        }
        let parent = self
            .root
            .nodes()
            .into_iter()
            .find(|parent| parent.children.iter().any(|child| child.id == id))
            .ok_or("Animation parent not found")?;
        Self::validate_animation_holder(parent, kind)?;
        self.set(id, "type", Some(json!(kind)))
    }

    fn validate_animation_holder(node: &Node, kind: &str) -> Result<()> {
        if !matches!(
            kind,
            "fade"
                | "gain"
                | "grow"
                | "shrink"
                | "blur"
                | "slideLeft"
                | "slideRight"
                | "slideUp"
                | "slideDown"
                | "spin"
                | "twist"
                | "appearWord"
                | "appearChar"
                | "scramble"
        ) {
            return Err("Invalid animation preset or timing".into());
        }
        if !node.is_timed() || matches!(node.tag.as_str(), "stage" | "sequence") {
            return Err("Animation requires a geometry or group holder".into());
        }
        if matches!(kind, "appearWord" | "appearChar" | "scramble")
            && !matches!(node.tag.as_str(), "text" | "captions")
        {
            return Err("Text animation requires text or captions".into());
        }
        Ok(())
    }

    pub fn insert_animation(
        &self,
        id: &str,
        kind: &str,
        out: bool,
        duration: f64,
        delay: f64,
    ) -> Result<Self> {
        self.ensure_editable()?;
        if !duration.is_finite()
            || duration <= 0.0
            || duration > 86400.0
            || !delay.is_finite()
            || delay < 0.0
            || delay > 86400.0
        {
            return Err("Invalid animation preset or timing".into());
        }
        Self::validate_animation_holder(self.node(id)?, kind)?;
        let draft = self.stamp_ids()?;
        let node = draft.node(id)?;
        let mut serial = 1;
        while draft.root.find(&format!("animation-{serial}")).is_some() {
            serial += 1;
        }
        let phase = if out { "out" } else { "in" };
        let content = format!(
            "\n<animation id=\"animation-{serial}\" type=\"{kind}\" phase=\"{phase}\" duration={{{duration}}} delay={{{delay}}} />\n"
        );
        let range = node.location.element.clone();
        let original = &draft.source[range.clone()];
        let replacement = if original.ends_with("/>") {
            format!(
                "{}>{content}</{}>",
                &original[..original.len() - 2],
                node.tag
            )
        } else {
            let close = original
                .rfind("</")
                .ok_or("Missing animation holder closing tag")?;
            format!("{}{content}{}", &original[..close], &original[close..])
        };
        draft.rewrite(vec![(range, replacement)])
    }

    pub fn insert_visual(&self, scene: &str, kind: &str, at: f64) -> Result<(Self, String)> {
        self.ensure_editable()?;
        if !matches!(kind, "rect" | "circle" | "text") || !at.is_finite() || at < 0.0 {
            return Err(
                "Visual insertion requires a supported shape and a finite nonnegative time".into(),
            );
        }
        let mut serial = 1;
        while self.root.find(&format!("{kind}-{serial}")).is_some() {
            serial += 1;
        }
        let id = format!("{kind}-{serial}");
        let draft = self.stamp_ids()?;
        let parent = draft.node(scene)?;
        if parent.tag != "scene" {
            return Err("Visual insertion requires a scene".into());
        }
        let content = match kind {
            "rect" => format!(
                "\n<rect id=\"{id}\" start={{{at}}} width={{320}} height={{180}} fill=\"#5b8cff\" />\n"
            ),
            "circle" => format!(
                "\n<rect id=\"{id}\" start={{{at}}} width={{180}} height={{180}} cornerRadius={{90}} fill=\"#5b8cff\" />\n"
            ),
            _ => format!(
                "\n<text id=\"{id}\" start={{{at}}} fontSize={{64}} color=\"white\">Texto</text>\n"
            ),
        };
        let range = parent.location.element.clone();
        let text = &draft.source[range.clone()];
        let replacement = if text.ends_with("/>") {
            format!("{}>{content}</{}>", &text[..text.len() - 2], parent.tag)
        } else {
            let close = text.rfind("</").ok_or("Missing scene closing tag")?;
            format!("{}{content}{}", &text[..close], &text[close..])
        };
        Ok((draft.rewrite(vec![(range, replacement)])?, id))
    }

    pub fn create_scene(&self, width: u32, height: u32, duration: f64) -> Result<(Self, String)> {
        self.ensure_editable()?;
        if self.root.tag != "stage"
            || !(1..=8192).contains(&width)
            || !(1..=8192).contains(&height)
            || !duration.is_finite()
            || !(1.0 / 30.0..=86400.0).contains(&duration)
        {
            return Err("A new scene requires a stage, dimensions between 1 and 8192, and a finite duration between one frame and 24 hours".into());
        }
        let mut serial = 1;
        while self.root.find(&format!("scene-{serial}")).is_some()
            || self
                .root
                .find(&format!("scene-{serial}-background"))
                .is_some()
        {
            serial += 1;
        }
        let id = format!("scene-{serial}");
        let draft = self.stamp_ids()?;
        let root = &draft.root;
        let range = root.location.element.clone();
        let text = &draft.source[range.clone()];
        let content = format!(
            "\n<scene id=\"{id}\" name=\"Scene {serial}\" width={{{width}}} height={{{height}}} workarea={{[0,{duration}]}}><solidPaint id=\"{id}-background\" color=\"#000000\" /></scene>\n"
        );
        let replacement = if text.ends_with("/>") {
            format!("{}>{content}</{}>", &text[..text.len() - 2], root.tag)
        } else {
            let close = text.rfind("</").ok_or("Missing stage closing tag")?;
            format!("{}{content}{}", &text[..close], &text[close..])
        };
        let next = draft
            .rewrite(vec![(range, replacement)])?
            .activate_scene(&id)?;
        Ok((next, id))
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
        static NEXT_CANDIDATE: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT_CANDIDATE.fetch_add(1, Ordering::Relaxed);
        let temp = state.join(format!(
            "native-save-{}-{suffix}-{serial}.tmp",
            std::process::id()
        ));
        crate::atomic_file::write_candidate(&temp, &canonical, before[target].as_bytes(), source)
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

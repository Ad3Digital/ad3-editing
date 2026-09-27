// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! JSX lowering for the Rust host. Attribute getters and child factories keep
//! Solid's lazy props/children semantics; original byte spans travel with nodes.
use crate::Result;
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use serde_json::{Map, json};
use std::path::Path;

struct Lower<'s> {
    source: &'s str,
    path: &'s Path,
    patches: Vec<(Span, String)>,
    error: Option<String>,
    depth: usize,
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn text(value: &str) -> String {
    let decoded = html_escape::decode_html_entities(value);
    let lines = decoded.split('\n').collect::<Vec<_>>();
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        let line = line.trim_end_matches('\r').replace('\t', " ");
        let line = if index > 0 { line.trim_start() } else { &line };
        let line = if index + 1 < lines.len() {
            line.trim_end()
        } else {
            line
        };
        if !line.is_empty() {
            output.push_str(line);
            if index + 1 < lines.len()
                && lines[index + 1..]
                    .iter()
                    .any(|line| !line.trim().is_empty())
            {
                output.push(' ');
            }
        }
    }
    output
}

impl Lower<'_> {
    fn slice(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }
    fn finish(mut self, span: Span) -> Result<String> {
        if let Some(error) = self.error {
            return Err(error);
        }
        self.patches.sort_by_key(|(span, _)| span.start);
        let mut cursor = span.start as usize;
        let mut result = String::new();
        for (patch, replacement) in self.patches {
            if (patch.start as usize) < cursor || patch.end > span.end {
                return Err("Overlapping JSX lowering spans".into());
            }
            result.push_str(&self.source[cursor..patch.start as usize]);
            result.push_str(&replacement);
            cursor = patch.end as usize;
        }
        result.push_str(&self.source[cursor..span.end as usize]);
        Ok(result)
    }
    fn expression(&self, expression: &Expression<'_>) -> Result<String> {
        let mut lower = Lower {
            source: self.source,
            path: self.path,
            patches: Vec::new(),
            error: None,
            depth: self.depth + 1,
        };
        lower.visit_expression(expression);
        lower.finish(expression.span())
    }
    fn children(&self, children: &[JSXChild<'_>], dynamic: bool) -> Result<String> {
        let mut values = Vec::new();
        let next = Lower {
            source: self.source,
            path: self.path,
            patches: Vec::new(),
            error: None,
            depth: self.depth + 1,
        };
        for child in children {
            let value = match child {
                JSXChild::Element(element) => next.element(element)?,
                JSXChild::Fragment(fragment) => next.fragment(fragment)?,
                JSXChild::Text(value) => {
                    let value = text(value.value.as_str());
                    if value.is_empty() {
                        continue;
                    }
                    quoted(&value)
                }
                JSXChild::ExpressionContainer(container) => {
                    match container.expression.as_expression() {
                        Some(expression) => {
                            let value = self.expression(expression)?;
                            if dynamic {
                                format!("__ad3Dynamic(() => ({value}))")
                            } else {
                                format!("({value})")
                            }
                        }
                        None => continue,
                    }
                }
                JSXChild::Spread(spread) => {
                    let value = self.expression(&spread.expression)?;
                    if dynamic {
                        format!("__ad3Dynamic(() => ({value}))")
                    } else {
                        format!("({value})")
                    }
                }
            };
            values.push(value);
        }
        Ok(match values.len() {
            0 => "undefined".into(),
            1 => format!("() => ({})", values[0]),
            _ => format!("() => ([{}])", values.join(",")),
        })
    }
    fn fragment(&self, fragment: &JSXFragment<'_>) -> Result<String> {
        if self.depth > 256 {
            return Err("JSX exceeds the nesting limit".into());
        }
        Ok(format!(
            "__ad3Jsx(null, {{}}, {}, null)",
            self.children(&fragment.children, true)?
        ))
    }
    fn element(&self, element: &JSXElement<'_>) -> Result<String> {
        if self.depth > 256 {
            return Err("JSX exceeds the nesting limit".into());
        }
        let name = &element.opening_element.name;
        let tag = match name {
            JSXElementName::Identifier(id) => quoted(id.name.as_str()),
            JSXElementName::IdentifierReference(_)
            | JSXElementName::MemberExpression(_)
            | JSXElementName::ThisExpression(_) => self.slice(name.span()).into(),
            JSXElementName::NamespacedName(_) => {
                return Err("Namespaced JSX tags require the HTML host".into());
            }
        };
        let mut attrs = Map::new();
        let mut segments = Vec::new();
        let mut properties = Vec::new();
        let mut has_spread = false;
        for attribute in &element.opening_element.attributes {
            match attribute {
                JSXAttributeItem::Attribute(attribute) => {
                    let name = self.slice(attribute.name.span());
                    let value = match &attribute.value {
                        None => "true".into(),
                        Some(JSXAttributeValue::StringLiteral(value)) => {
                            quoted(&html_escape::decode_html_entities(value.value.as_str()))
                        }
                        Some(JSXAttributeValue::ExpressionContainer(container)) => self
                            .expression(
                                container
                                    .expression
                                    .as_expression()
                                    .ok_or("Empty JSX attribute expression")?,
                            )?,
                        Some(JSXAttributeValue::Element(element)) => self.element(element)?,
                        Some(JSXAttributeValue::Fragment(fragment)) => self.fragment(fragment)?,
                    };
                    properties.push(format!("get [{}](){{return ({value});}}", quoted(name)));
                    attrs.insert(
                        name.into(),
                        json!({"start": attribute.span.start, "end": attribute.span.end}),
                    );
                }
                JSXAttributeItem::SpreadAttribute(spread) => {
                    has_spread = true;
                    if !properties.is_empty() {
                        segments.push(format!("{{{}}}", properties.join(",")));
                        properties.clear();
                    }
                    segments.push(format!("() => ({})", self.expression(&spread.argument)?));
                }
            }
        }
        if !properties.is_empty() {
            segments.push(format!("{{{}}}", properties.join(",")));
        }
        let props = if has_spread {
            format!("__ad3MergeProps({})", segments.join(","))
        } else {
            segments.pop().unwrap_or_else(|| "{}".into())
        };
        let origin = json!({"file": self.path, "element": {"start": element.span.start, "end": element.span.end},
            "name_end": name.span().end, "attributes": attrs, "spread": has_spread});
        Ok(format!(
            "__ad3Jsx({tag}, {props}, {}, {})",
            self.children(
                &element.children,
                matches!(name, JSXElementName::Identifier(_))
            )?,
            quoted(&origin.to_string())
        ))
    }
}

impl<'a> Visit<'a> for Lower<'_> {
    fn visit_jsx_element(&mut self, element: &JSXElement<'a>) {
        match self.element(element) {
            Ok(code) => self.patches.push((element.span, code)),
            Err(error) => {
                self.error.get_or_insert(error);
            }
        }
    }
    fn visit_jsx_fragment(&mut self, fragment: &JSXFragment<'a>) {
        match self.fragment(fragment) {
            Ok(code) => self.patches.push((fragment.span, code)),
            Err(error) => {
                self.error.get_or_insert(error);
            }
        }
    }
}

pub fn lower(path: &Path, source: &str) -> Result<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).map_err(|error| error.to_string())?;
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!("{}: {:?}", path.display(), parsed.diagnostics));
    }
    let mut lower = Lower {
        source,
        path,
        patches: Vec::new(),
        error: None,
        depth: 0,
    };
    lower.visit_program(&parsed.program);
    lower.finish(Span::new(
        0,
        source.len().try_into().map_err(|_| "Source is too large")?,
    ))
}

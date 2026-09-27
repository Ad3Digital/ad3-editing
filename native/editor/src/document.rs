// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use crate::Result;
use serde::Serialize;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, ops::Range};

pub const TAGS: &[&str] = &[
    "stage",
    "scene",
    "group",
    "rect",
    "video",
    "image",
    "audio",
    "text",
    "textRange",
    "sequence",
    "captions",
    "adjustmentLayer",
    "solidPaint",
    "linearGradientPaint",
    "radialGradientPaint",
    "imagePaint",
    "videoPaint",
    "colorStop",
    "stroke",
    "shadow",
    "effect",
    "animation",
    "keyframeTrack",
    "keyframe",
    "htmlPaint",
    "html",
    "shaderPaint",
    "surfacePaint",
    "surface",
];

#[derive(Clone, Debug, Default)]
pub struct Location {
    pub element: Range<usize>,
    pub name_end: usize,
    pub attributes: BTreeMap<String, Range<usize>>,
}

/// Authored properties are retained verbatim in the original source. This tree
/// is a derived document, never a second project file to get out of sync.
#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub tag: String,
    pub props: Map<String, Value>,
    pub text: String,
    pub children: Vec<Node>,
    #[serde(skip)]
    pub location: Location,
}

impl Node {
    pub fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }

    pub fn walk<'a>(&'a self, output: &mut Vec<&'a Node>) {
        output.push(self);
        for child in &self.children {
            child.walk(output);
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        self.props
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn number(&self, name: &str, default: f64) -> Result<f64> {
        match self.props.get(name) {
            None => Ok(default),
            Some(v) => v
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{}: {name} must be a finite number", self.id)),
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        self.props.get(name).and_then(Value::as_str)
    }

    pub fn is_container(&self) -> bool {
        matches!(self.tag.as_str(), "stage" | "scene" | "group" | "sequence")
    }

    pub fn is_timed(&self) -> bool {
        self.is_container()
            || matches!(
                self.tag.as_str(),
                "rect"
                    | "video"
                    | "image"
                    | "audio"
                    | "text"
                    | "captions"
                    | "adjustmentLayer"
                    | "html"
                    | "surface"
            )
    }

    pub fn nodes(&self) -> Vec<&Self> {
        let mut nodes = Vec::new();
        self.walk(&mut nodes);
        nodes
    }
}

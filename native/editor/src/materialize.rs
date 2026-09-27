// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Resolve live host nodes at a frame boundary without remounting components.
use crate::{
    document::{Location, Node, TAGS},
    reactive::{self, error, function},
};
use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, js_string,
    object::{JsObject, ObjectInitializer},
    property::{Attribute, PropertyDescriptor},
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub fn dynamic(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(reactive::memo(
        reactive::callable(args.first())?,
        JsValue::undefined(),
        None,
        false,
        context,
    )?
    .into())
}

fn once(factory: JsObject, context: &mut Context) -> JsResult<JsValue> {
    reactive::untracked(context, |context| {
        factory.call(&JsValue::undefined(), &[], context)
    })
}

pub fn jsx(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let tag = args.first().ok_or_else(|| error("Missing JSX tag"))?;
    let props = args
        .get(1)
        .and_then(JsValue::as_object)
        .unwrap_or_else(|| ObjectInitializer::new(context).build());
    let factory = args.get(2).and_then(JsValue::as_callable);
    if let Some(component) = tag.as_callable() {
        let props = if let Some(factory) = factory {
            let children = ObjectInitializer::new(context).build();
            children.define_property_or_throw(
                js_string!("children"),
                PropertyDescriptor::builder()
                    .get(factory)
                    .enumerable(true)
                    .configurable(true)
                    .build(),
                context,
            )?;
            crate::props::merge(
                &JsValue::undefined(),
                &[props.into(), children.into()],
                context,
            )?
        } else {
            props.into()
        };
        return reactive::untracked(context, |context| {
            component.call(&JsValue::undefined(), &[props], context)
        });
    }
    let children = if let Some(factory) = factory {
        once(factory, context)?
    } else {
        let getter = function(
            context,
            NativeFunction::from_copy_closure_with_captures(
                |_, _, props, context| props.get(js_string!("children"), context),
                props.clone(),
            ),
            0,
        );
        dynamic(&JsValue::undefined(), &[getter.into()], context)?
    };
    let children = crate::control::children_value(children, context)?;
    if tag.is_null() {
        return Ok(children);
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
    let serial = {
        let state = reactive::host(context)?;
        let mut state = state.borrow_mut();
        state.node_serial += 1;
        state.node_serial
    };
    let properties = crate::props::bind(props, context)?;
    Ok(ObjectInitializer::new(context)
        .property(js_string!("tag"), tag.clone(), Attribute::empty())
        .property(js_string!("props"), properties, Attribute::empty())
        .property(js_string!("children"), children, Attribute::empty())
        .property(
            js_string!("origin"),
            args.get(3).cloned().unwrap_or_default(),
            Attribute::empty(),
        )
        .property(
            js_string!("serial"),
            js_string!(format!("native-eval-{serial}")),
            Attribute::empty(),
        )
        .build()
        .into())
}

struct Reader {
    ancestors: Vec<JsObject>,
    count: usize,
    ids: BTreeSet<String>,
}
impl Reader {
    fn append(
        &mut self,
        value: &JsValue,
        parent: &mut Node,
        context: &mut Context,
    ) -> JsResult<()> {
        self.count += 1;
        if self.count > 100_000 || self.ancestors.len() > 128 {
            return Err(error("Composition exceeds its node/nesting limit"));
        }
        if value.is_null_or_undefined() || value.is_boolean() {
            return Ok(());
        }
        if let Some(text) = value.as_string() {
            parent.text.push_str(&text.to_std_string_escaped());
            return Ok(());
        }
        if let Some(number) = value.as_number() {
            if !number.is_finite() {
                return Err(error("Non-finite composition child"));
            }
            parent
                .text
                .push_str(dragonbox_ecma::Buffer::new().format(number));
            return Ok(());
        }
        let object = value
            .as_object()
            .ok_or_else(|| error("Invalid composition child"))?;
        if self.ancestors.contains(&object) {
            return Err(error("Composition children contain a cycle"));
        }
        self.ancestors.push(object.clone());
        if let Some(callback) = value.as_callable() {
            if callback
                .get(js_string!("length"), context)?
                .to_length(context)?
                > 0
            {
                return Err(error("A callback child requires a control-flow component"));
            }
            let resolved = callback.call(&JsValue::undefined(), &[], context)?;
            self.append(&resolved, parent, context)?;
        } else if object.is_array() {
            let length = object
                .get(js_string!("length"), context)?
                .to_length(context)?;
            if length > 100_000 {
                return Err(error("Composition array is too large"));
            }
            for i in 0..length {
                self.append(&object.get(i, context)?, parent, context)?;
            }
        } else {
            parent.children.push(self.node(&object, context)?);
        }
        self.ancestors.pop();
        Ok(())
    }
    fn node(&mut self, object: &JsObject, context: &mut Context) -> JsResult<Node> {
        let tag = object
            .get(js_string!("tag"), context)?
            .as_string()
            .ok_or_else(|| error("Component returned an invalid composition node"))?
            .to_std_string_escaped();
        if !TAGS.contains(&tag.as_str()) {
            return Err(error("Component returned an invalid composition tag"));
        }
        let input = object.get(js_string!("props"), context)?;
        let entries = reactive::callable(Some(&input))?
            .call(&JsValue::undefined(), &[], context)?
            .as_object()
            .ok_or_else(|| error("Invalid host properties"))?;
        let length = entries
            .get(js_string!("length"), context)?
            .to_length(context)?;
        let mut props = Map::new();
        for index in 0..length {
            let entry = entries
                .get(index, context)?
                .as_object()
                .ok_or_else(|| error("Invalid property binding"))?;
            let key = entry
                .get(js_string!("key"), context)?
                .to_property_key(context)?;
            let getter = entry.get(js_string!("get"), context)?;
            let value =
                reactive::callable(Some(&getter))?.call(&JsValue::undefined(), &[], context)?;
            if value.is_undefined() {
                continue;
            }
            let text = value
                .as_string()
                .ok_or_else(|| error("Invalid host property snapshot"))?
                .to_std_string_escaped();
            props.insert(
                key.to_string(),
                serde_json::from_str(&text).map_err(|e| error(e.to_string()))?,
            );
        }
        let id = match props.get("id") {
            Some(Value::String(id)) if !id.trim().is_empty() => id.clone(),
            Some(_) => return Err(error("Node id must be a nonempty string")),
            None => object
                .get(js_string!("serial"), context)?
                .as_string()
                .ok_or_else(|| error("Invalid host node identity"))?
                .to_std_string_escaped(),
        };
        if !self.ids.insert(id.clone()) {
            return Err(error(format!(
                "Duplicate id {id:?}; generated instances must currently have distinct ids"
            )));
        }
        let origin = object.get(js_string!("origin"), context)?;
        let location = if origin.is_null_or_undefined() {
            Location::default()
        } else {
            serde_json::from_str(
                &origin
                    .as_string()
                    .ok_or_else(|| error("Invalid source origin"))?
                    .to_std_string_escaped(),
            )
            .map_err(|error_| error(error_.to_string()))?
        };
        let mut node = Node {
            id,
            tag,
            props,
            text: String::new(),
            children: Vec::new(),
            location,
        };
        self.append(
            &object.get(js_string!("children"), context)?,
            &mut node,
            context,
        )?;
        if !node.text.trim().is_empty() && !matches!(node.tag.as_str(), "text" | "textRange") {
            return Err(error(format!(
                "Text is only valid inside <text> / <textRange>, found in <{}>",
                node.tag
            )));
        }
        Ok(node)
    }
}

pub fn root(value: &JsValue, context: &mut Context) -> JsResult<Node> {
    let mut reader = Reader {
        ancestors: Vec::new(),
        count: 0,
        ids: BTreeSet::new(),
    };
    let mut holder = Node {
        id: String::new(),
        tag: String::new(),
        props: Map::new(),
        text: String::new(),
        children: Vec::new(),
        location: Location::default(),
    };
    reactive::untracked(context, |context| {
        reader.append(value, &mut holder, context)
    })?;
    if holder.children.len() != 1
        || !holder.text.trim().is_empty()
        || holder.children[0].tag != "stage"
    {
        return Err(error("Project must return one <stage>"));
    }
    Ok(holder.children.remove(0))
}

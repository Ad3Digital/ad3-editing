// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native reactive mergeProps/splitProps views. Rust Proxy traps preserve lazy
//! spread reads, override order and getters instead of copying current values.
use crate::reactive::{error, function};
use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, js_string,
    object::{
        JsObject, ObjectInitializer,
        builtins::{JsArray, JsProxy},
    },
    property::{Attribute, PropertyKey},
};

fn sources(target: &JsObject, context: &mut Context) -> JsResult<Vec<JsObject>> {
    let stored = target
        .get(js_string!("__ad3Sources"), context)?
        .as_object()
        .ok_or_else(|| error("Invalid props view"))?;
    let length = stored
        .get(js_string!("length"), context)?
        .to_length(context)?;
    let mut result = Vec::new();
    for index in 0..length {
        let value = stored.get(index, context)?;
        let value = if let Some(callback) = value.as_callable() {
            callback.call(&JsValue::undefined(), &[], context)?
        } else {
            value
        };
        if !value.is_null_or_undefined() {
            result.push(value.to_object(context)?);
        }
    }
    Ok(result)
}

fn allowed(target: &JsObject, key: &PropertyKey, context: &mut Context) -> JsResult<bool> {
    let keys = target.get(js_string!("__ad3Keys"), context)?;
    let Some(keys) = keys.as_object() else {
        return Ok(true);
    };
    let length = keys
        .get(js_string!("length"), context)?
        .to_length(context)?;
    let mut contains = false;
    for index in 0..length {
        if keys.get(index, context)?.to_property_key(context)? == *key {
            contains = true;
            break;
        }
    }
    let exclude = target
        .get(js_string!("__ad3Exclude"), context)?
        .to_boolean();
    Ok(contains != exclude)
}

fn get(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let target = args[0]
        .as_object()
        .ok_or_else(|| error("Missing props target"))?;
    let key = args[1].to_property_key(context)?;
    if !allowed(&target, &key, context)? {
        return Ok(JsValue::undefined());
    }
    for source in sources(&target, context)?.iter().rev() {
        let value = source.get(key.clone(), context)?;
        if !value.is_undefined() {
            return Ok(value);
        }
    }
    Ok(JsValue::undefined())
}

fn own_keys(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let target = args[0]
        .as_object()
        .ok_or_else(|| error("Missing props target"))?;
    let mut keys = Vec::new();
    for source in sources(&target, context)? {
        for key in source.own_property_keys(context)? {
            if !keys.contains(&key) && allowed(&target, &key, context)? {
                keys.push(key);
            }
        }
    }
    Ok(JsArray::from_iter(keys.into_iter().map(JsValue::from), context).into())
}

fn has(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let target = args[0]
        .as_object()
        .ok_or_else(|| error("Missing props target"))?;
    let key = args[1].to_property_key(context)?;
    if allowed(&target, &key, context)? {
        for source in sources(&target, context)? {
            if source.has_property(key.clone(), context)? {
                return Ok(true.into());
            }
        }
    }
    Ok(false.into())
}

fn descriptor(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if !has(&JsValue::undefined(), args, context)?.to_boolean() {
        return Ok(JsValue::undefined());
    }
    let getter = function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, captures, context| get(&JsValue::undefined(), captures, context),
            args[..2].to_vec(),
        ),
        0,
    );
    Ok(ObjectInitializer::new(context)
        .property(js_string!("enumerable"), true, Attribute::all())
        .property(js_string!("configurable"), true, Attribute::all())
        .property(js_string!("get"), getter, Attribute::all())
        .build()
        .into())
}

fn view(
    values: &[JsValue],
    keys: Option<Vec<JsValue>>,
    exclude: bool,
    context: &mut Context,
) -> JsResult<JsValue> {
    let values = JsArray::from_iter(values.iter().cloned(), context);
    let keys: JsValue = keys
        .map(|keys| JsArray::from_iter(keys, context).into())
        .unwrap_or_default();
    let target = ObjectInitializer::new(context)
        .property(js_string!("__ad3Sources"), values, Attribute::CONFIGURABLE)
        .property(js_string!("__ad3Keys"), keys, Attribute::CONFIGURABLE)
        .property(js_string!("__ad3Exclude"), exclude, Attribute::CONFIGURABLE)
        .build();
    Ok(JsProxy::builder(target)
        .get(get)
        .own_keys(own_keys)
        .has(has)
        .get_own_property_descriptor(descriptor)
        .build(context)?
        .into())
}

pub fn merge(_: &JsValue, values: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    view(values, None, false, context)
}

pub fn split(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let props = args
        .first()
        .cloned()
        .ok_or_else(|| error("splitProps requires props"))?;
    let mut result = Vec::new();
    let mut excluded = Vec::new();
    for selection in &args[1..] {
        let selection = selection
            .as_object()
            .ok_or_else(|| error("splitProps keys must be arrays"))?;
        let length = selection
            .get(js_string!("length"), context)?
            .to_length(context)?;
        let mut keys = Vec::new();
        for index in 0..length {
            keys.push(selection.get(index, context)?);
        }
        excluded.extend(keys.iter().cloned());
        result.push(view(
            std::slice::from_ref(&props),
            Some(keys),
            false,
            context,
        )?);
    }
    result.push(view(&[props], Some(excluded), true, context)?);
    Ok(JsArray::from_iter(result, context).into())
}

/// One memo per attribute: changing one prop must not poll nonreactive siblings.
/// A separate key-list memo owns spread insertions/removals.
pub(crate) fn bind(props: JsObject, context: &mut Context) -> JsResult<JsObject> {
    use crate::reactive::{self, Key};
    let owner = reactive::new_owner(reactive::current_owner(context)?, context)?;
    let data = ObjectInitializer::new(context)
        .property(js_string!("props"), props, Attribute::all())
        .property(js_string!("owner"), owner.value(), Attribute::all())
        .build();
    let callback = function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, data, context| {
                let props = data
                    .get(js_string!("props"), context)?
                    .as_object()
                    .ok_or_else(|| error("Invalid bound props"))?;
                let keys = props.own_property_keys(context)?;
                if keys.len() > 10_000 {
                    return Err(error("Node exceeds 10,000 properties"));
                }
                reactive::untracked(context, |context| {
                    let old = data.get(js_string!("entries"), context)?.as_object();
                    let length = if let Some(old) = &old {
                        old.get(js_string!("length"), context)?.to_length(context)?
                    } else {
                        0
                    };
                    let mut prior = Vec::new();
                    for index in 0..length {
                        prior.push(
                            old.as_ref()
                                .unwrap()
                                .get(index, context)?
                                .as_object()
                                .ok_or_else(|| error("Invalid prop binding"))?,
                        );
                    }
                    let mut used = vec![false; prior.len()];
                    let mut entries = Vec::new();
                    for key in keys {
                        if key.to_string() == "children" {
                            continue;
                        }
                        let mut found = None;
                        for (index, entry) in prior.iter().enumerate() {
                            if entry
                                .get(js_string!("key"), context)?
                                .to_property_key(context)?
                                == key
                            {
                                found = Some(index);
                                break;
                            }
                        }
                        if let Some(index) = found {
                            used[index] = true;
                            entries.push(prior[index].clone().into());
                            continue;
                        }
                        let scope = reactive::new_owner(
                            Key::from_value(&data.get(js_string!("owner"), context)?)?,
                            context,
                        )?;
                        let captures = vec![props.clone().into(), JsValue::from(key)];
                        let read = function(
                            context,
                            NativeFunction::from_copy_closure_with_captures(
                                |_, _, captures, context| {
                                    let props = captures[0]
                                        .as_object()
                                        .ok_or_else(|| error("Invalid prop source"))?;
                                    let value = props
                                        .get(captures[1].to_property_key(context)?, context)?;
                                    if value.is_undefined() {
                                        return Ok(JsValue::undefined());
                                    }
                                    let data = crate::evaluator::snapshot_json(
                                        &value,
                                        context,
                                        &mut Vec::new(),
                                        &mut 0,
                                    )?;
                                    Ok(js_string!(data.to_string()).into())
                                },
                                captures.clone(),
                            ),
                            0,
                        );
                        let getter = reactive::with_owner(Some(scope), None, context, |context| {
                            reactive::memo(read, JsValue::undefined(), None, false, context)
                        })?;
                        entries.push(
                            ObjectInitializer::new(context)
                                .property(js_string!("key"), captures[1].clone(), Attribute::all())
                                .property(js_string!("owner"), scope.value(), Attribute::all())
                                .property(js_string!("get"), getter, Attribute::all())
                                .build()
                                .into(),
                        );
                    }
                    for (index, entry) in prior.iter().enumerate() {
                        if !used[index] {
                            reactive::dispose_owner(
                                Key::from_value(&entry.get(js_string!("owner"), context)?)?,
                                context,
                            )?;
                        }
                    }
                    let entries = JsArray::from_iter(entries, context);
                    data.set(js_string!("entries"), entries.clone(), true, context)?;
                    Ok(entries.into())
                })
            },
            data,
        ),
        0,
    );
    reactive::with_owner(Some(owner), None, context, |context| {
        reactive::memo(callback, JsValue::undefined(), None, false, context)
    })
}

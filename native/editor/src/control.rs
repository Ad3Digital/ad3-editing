// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native Solid control flow. Cached branches own separate lifetimes so a list
//! recomputation does not remount surviving component instances.
use crate::reactive::{self, Key, error};
use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, js_string,
    object::{JsObject, ObjectInitializer, builtins::JsArray},
    property::Attribute,
};

fn get(object: &JsObject, name: &str, context: &mut Context) -> JsResult<JsValue> {
    object.get(js_string!(name), context)
}
fn set(
    object: &JsObject,
    name: &str,
    value: impl Into<JsValue>,
    context: &mut Context,
) -> JsResult<()> {
    object
        .set(js_string!(name), value, true, context)
        .map(|_| ())
}
fn object(value: JsValue) -> JsResult<JsObject> {
    value
        .as_object()
        .ok_or_else(|| error("Expected control props"))
}
fn values(value: &JsValue, context: &mut Context) -> JsResult<Vec<JsValue>> {
    if value.is_null_or_undefined() || value.as_boolean() == Some(false) {
        return Ok(Vec::new());
    }
    let array = object(value.clone())?;
    if !array.is_array() {
        return Err(error("For/Index each must be an array"));
    }
    let length = get(&array, "length", context)?.to_length(context)?;
    if length > 100_000 {
        return Err(error("Reactive list exceeds 100,000 items"));
    }
    (0..length).map(|index| array.get(index, context)).collect()
}

fn controller(args: &[JsValue], context: &mut Context) -> JsResult<JsObject> {
    let props = object(args.first().cloned().unwrap_or_default())?;
    let owner = reactive::new_owner(reactive::current_owner(context)?, context)?;
    Ok(ObjectInitializer::new(context)
        .property(js_string!("props"), props, Attribute::all())
        .property(js_string!("owner"), owner.value(), Attribute::all())
        .build())
}

fn memo(
    data: JsObject,
    callback: boa_engine::native_function::NativeFunctionPointer,
    context: &mut Context,
) -> JsResult<JsValue> {
    let owner = Key::from_value(&get(&data, "owner", context)?)?;
    let callback = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            move |_, _, captures, context| {
                callback(
                    &JsValue::undefined(),
                    std::slice::from_ref(captures),
                    context,
                )
            },
            JsValue::from(data),
        ),
        0,
    );
    reactive::with_owner(Some(owner), None, context, |context| {
        Ok(reactive::memo(callback, JsValue::undefined(), None, false, context)?.into())
    })
}

fn dispose_branch(data: &JsObject, context: &mut Context) -> JsResult<()> {
    let owner = get(data, "branchOwner", context)?;
    if !owner.is_undefined() {
        reactive::dispose_owner(Key::from_value(&owner)?, context)?;
    }
    set(data, "branchOwner", JsValue::undefined(), context)?;
    set(data, "branch", JsValue::undefined(), context)
}

fn branch(
    data: &JsObject,
    property: &str,
    render_argument: Option<JsValue>,
    context: &mut Context,
) -> JsResult<JsValue> {
    let owner = reactive::new_owner(Key::from_value(&get(data, "owner", context)?)?, context)?;
    set(data, "branchOwner", owner.value(), context)?;
    let props = object(get(data, "props", context)?)?;
    let value = reactive::with_owner(Some(owner), None, context, |context| {
        let value = get(&props, property, context)?;
        if let Some(argument) = render_argument
            && let Some(callback) = value.as_callable()
        {
            callback.call(&JsValue::undefined(), &[argument], context)
        } else {
            Ok(value)
        }
    })?;
    set(data, "branch", value.clone(), context)?;
    Ok(value)
}

fn list(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = object(args[0].clone())?;
    let props = object(get(&data, "props", context)?)?;
    let items = values(&get(&props, "each", context)?, context)?;
    // Everything except the each getter is untracked, as in Solid mapArray.
    reactive::untracked(context, |context| {
        reactive::batch(context, |context| {
            let indexed = get(&data, "indexed", context)?.to_boolean();
            let prior = values(&get(&data, "entries", context)?, context)?;
            let mut used = vec![false; prior.len()];
            let mut entries = Vec::new();
            let mut output = Vec::new();
            let render = if items.is_empty() {
                None
            } else {
                Some(reactive::callable(Some(&get(
                    &props, "children", context,
                )?))?)
            };
            for (index, item) in items.into_iter().enumerate() {
                let mut found = None;
                for (old, entry) in prior.iter().enumerate() {
                    if used[old] {
                        continue;
                    }
                    let entry = object(entry.clone())?;
                    if (indexed && index == old)
                        || (!indexed && get(&entry, "item", context)?.strict_equals(&item))
                    {
                        found = Some((old, entry));
                        break;
                    }
                }
                let entry = if let Some((old, entry)) = found {
                    used[old] = true;
                    let setter = reactive::callable(Some(&get(&entry, "set", context)?))?;
                    let value = if indexed {
                        item.clone()
                    } else {
                        JsValue::from(index)
                    };
                    // A list item can itself be callable; a setter's functional form
                    // must not execute it while updating an Index item signal.
                    let constant = reactive::function(
                        context,
                        NativeFunction::from_copy_closure_with_captures(
                            |_, _, value, _| Ok(value.clone()),
                            value,
                        ),
                        0,
                    );
                    setter.call(&JsValue::undefined(), &[constant.into()], context)?;
                    set(&entry, "item", item.clone(), context)?;
                    entry
                } else {
                    let owner = reactive::new_owner(
                        Key::from_value(&get(&data, "owner", context)?)?,
                        context,
                    )?;
                    reactive::with_owner(Some(owner), None, context, |context| {
                        let signal = object(reactive::create_signal(
                            &JsValue::undefined(),
                            &[if indexed { item.clone() } else { index.into() }],
                            context,
                        )?)?;
                        let getter = signal.get(0, context)?;
                        let setter = signal.get(1, context)?;
                        let arguments = if indexed {
                            [getter, index.into()]
                        } else {
                            [item.clone(), getter]
                        };
                        let value = render.as_ref().unwrap().call(
                            &JsValue::undefined(),
                            &arguments,
                            context,
                        )?;
                        Ok(ObjectInitializer::new(context)
                            .property(js_string!("owner"), owner.value(), Attribute::all())
                            .property(js_string!("item"), item, Attribute::all())
                            .property(js_string!("set"), setter, Attribute::all())
                            .property(js_string!("value"), value, Attribute::all())
                            .build())
                    })?
                };
                output.push(get(&entry, "value", context)?);
                entries.push(entry.into());
            }
            for (index, entry) in prior.into_iter().enumerate() {
                if !used[index] {
                    reactive::dispose_owner(
                        Key::from_value(&get(&object(entry)?, "owner", context)?)?,
                        context,
                    )?;
                }
            }
            let empty = entries.is_empty();
            let entries = JsArray::from_iter(entries, context);
            set(&data, "entries", entries, context)?;
            if empty {
                if get(&data, "branchOwner", context)?.is_undefined() {
                    branch(&data, "fallback", None, context)
                } else {
                    get(&data, "branch", context)
                }
            } else {
                dispose_branch(&data, context)?;
                Ok(JsArray::from_iter(output, context).into())
            }
        })
    })
}

pub fn for_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    memo(controller(args, context)?, list, context)
}
pub fn index_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = controller(args, context)?;
    set(&data, "indexed", true, context)?;
    memo(data, list, context)
}

pub fn show_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let props = object(args.first().cloned().unwrap_or_default())?;
    let keyed = reactive::untracked(context, |context| get(&props, "keyed", context))?.to_boolean();
    let read = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, props, context| get(props, "when", context),
            props.clone(),
        ),
        0,
    );
    let condition_value = reactive::memo(read, JsValue::undefined(), None, false, context)?;
    let condition = if keyed {
        condition_value.clone()
    } else {
        let equals = reactive::function(
            context,
            NativeFunction::from_fn_ptr(|_, args, _| {
                Ok((args[0].to_boolean() == args[1].to_boolean()).into())
            }),
            2,
        );
        reactive::memo(
            condition_value.clone(),
            JsValue::undefined(),
            Some(equals.into()),
            false,
            context,
        )?
    };
    let callback = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, data: &Vec<JsValue>, context| {
                let props = object(data[0].clone())?;
                let condition = reactive::callable(Some(&data[1]))?;
                let value = condition.call(&JsValue::undefined(), &[], context)?;
                if !value.to_boolean() {
                    return get(&props, "fallback", context);
                }
                let child = get(&props, "children", context)?;
                if let Some(render) = child.as_callable()
                    && get(&render, "length", context)?.to_length(context)? > 0
                {
                    let argument = if data[3].to_boolean() {
                        value
                    } else {
                        reactive::function(
                            context,
                            NativeFunction::from_copy_closure_with_captures(
                                |_, _, data: &Vec<JsValue>, context| {
                                    let condition = reactive::callable(Some(&data[0]))?;
                                    if !reactive::untracked(context, |context| {
                                        condition.call(&JsValue::undefined(), &[], context)
                                    })?
                                    .to_boolean()
                                    {
                                        return Err(error(
                                            "Show accessor read outside its active branch",
                                        ));
                                    }
                                    reactive::callable(Some(&data[1]))?.call(
                                        &JsValue::undefined(),
                                        &[],
                                        context,
                                    )
                                },
                                vec![data[1].clone(), data[2].clone()],
                            ),
                            0,
                        )
                        .into()
                    };
                    reactive::untracked(context, |context| {
                        render.call(&JsValue::undefined(), &[argument], context)
                    })
                } else {
                    Ok(child)
                }
            },
            vec![
                props.into(),
                condition.into(),
                condition_value.into(),
                keyed.into(),
            ],
        ),
        0,
    );
    Ok(reactive::memo(callback, JsValue::undefined(), None, false, context)?.into())
}

fn resolve_children(
    value: JsValue,
    context: &mut Context,
    depth: usize,
    count: &mut usize,
) -> JsResult<JsValue> {
    *count += 1;
    if depth > 128 || *count > 100_000 {
        return Err(error("Reactive children exceed nesting/size limits"));
    }
    if let Some(callback) = value.as_callable()
        && get(&callback, "length", context)?.to_length(context)? == 0
    {
        return resolve_children(
            callback.call(&JsValue::undefined(), &[], context)?,
            context,
            depth + 1,
            count,
        );
    }
    if let Some(array) = value.as_object().filter(JsObject::is_array) {
        let length = get(&array, "length", context)?.to_length(context)?;
        if length > 100_000 {
            return Err(error("Reactive children array is too large"));
        }
        let mut output = Vec::new();
        for index in 0..length {
            let value = resolve_children(array.get(index, context)?, context, depth + 1, count)?;
            if let Some(array) = value.as_object().filter(JsObject::is_array) {
                let length = get(&array, "length", context)?.to_length(context)?;
                for index in 0..length {
                    output.push(array.get(index, context)?);
                }
            } else {
                output.push(value);
            }
        }
        return Ok(JsArray::from_iter(output, context).into());
    }
    Ok(value)
}

pub fn children_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let source = reactive::memo(
        reactive::callable(args.first())?,
        JsValue::undefined(),
        None,
        false,
        context,
    )?;
    let resolve = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, source, context| {
                resolve_children(
                    source.call(&JsValue::undefined(), &[], context)?,
                    context,
                    0,
                    &mut 0,
                )
            },
            source,
        ),
        0,
    );
    let memo = reactive::memo(resolve, JsValue::undefined(), None, false, context)?;
    let to_array = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, memo, context| {
                let value = memo.call(&JsValue::undefined(), &[], context)?;
                if value.as_object().is_some_and(|object| object.is_array()) {
                    return Ok(value);
                }
                Ok(JsArray::from_iter(
                    if value.is_null_or_undefined() {
                        Vec::new()
                    } else {
                        vec![value]
                    },
                    context,
                )
                .into())
            },
            memo.clone(),
        ),
        0,
    );
    set(&memo, "toArray", to_array, context)?;
    Ok(memo.into())
}

pub(crate) fn children_value(value: JsValue, context: &mut Context) -> JsResult<JsValue> {
    let factory = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(|_, _, value, _| Ok(value.clone()), value),
        0,
    );
    children_api(&JsValue::undefined(), &[factory.into()], context)
}

pub fn match_api(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(object(args.first().cloned().unwrap_or_default())?.into())
}

pub fn switch_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let props = object(args.first().cloned().unwrap_or_default())?;
    let read = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, props, context| get(props, "children", context),
            props.clone(),
        ),
        0,
    );
    let children = children_api(&JsValue::undefined(), &[read.into()], context)?;
    let build = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, children, context| {
                let value = reactive::callable(Some(children))?.call(
                    &JsValue::undefined(),
                    &[],
                    context,
                )?;
                let matches = if value.as_object().is_some_and(|value| value.is_array()) {
                    values(&value, context)?
                } else {
                    vec![value]
                };
                let mut select = reactive::function(
                    context,
                    NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                    0,
                );
                for (index, value) in matches.into_iter().enumerate() {
                    let props = object(value)?;
                    let keyed = get(&props, "keyed", context)?.to_boolean();
                    let read = reactive::function(
                        context,
                        NativeFunction::from_copy_closure_with_captures(
                            |_, _, data: &Vec<JsValue>, context| {
                                let previous = reactive::callable(Some(&data[0]))?.call(
                                    &JsValue::undefined(),
                                    &[],
                                    context,
                                )?;
                                if previous.to_boolean() {
                                    return Ok(JsValue::undefined());
                                }
                                get(&object(data[1].clone())?, "when", context)
                            },
                            vec![select.clone().into(), props.clone().into()],
                        ),
                        0,
                    );
                    let condition_value =
                        reactive::memo(read, JsValue::undefined(), None, false, context)?;
                    let condition = if keyed {
                        condition_value.clone()
                    } else {
                        let equals = reactive::function(
                            context,
                            NativeFunction::from_fn_ptr(|_, args, _| {
                                Ok((args[0].to_boolean() == args[1].to_boolean()).into())
                            }),
                            2,
                        );
                        reactive::memo(
                            condition_value.clone(),
                            JsValue::undefined(),
                            Some(equals.into()),
                            false,
                            context,
                        )?
                    };
                    select = reactive::function(
                        context,
                        NativeFunction::from_copy_closure_with_captures(
                            |_, _, data: &Vec<JsValue>, context| {
                                let previous = reactive::callable(Some(&data[0]))?.call(
                                    &JsValue::undefined(),
                                    &[],
                                    context,
                                )?;
                                if previous.to_boolean() {
                                    return Ok(previous);
                                }
                                if !reactive::callable(Some(&data[1]))?
                                    .call(&JsValue::undefined(), &[], context)?
                                    .to_boolean()
                                {
                                    return Ok(JsValue::undefined());
                                }
                                Ok(JsArray::from_iter(data[2..].iter().cloned(), context).into())
                            },
                            vec![
                                select.into(),
                                condition.into(),
                                JsValue::from(index as u32),
                                condition_value.into(),
                                props.into(),
                            ],
                        ),
                        0,
                    );
                }
                Ok(select.into())
            },
            children,
        ),
        0,
    );
    let switch = reactive::memo(build, JsValue::undefined(), None, false, context)?;
    let render = reactive::function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, data: &Vec<JsValue>, context| {
                let switch = reactive::callable(Some(&data[0]))?;
                let select = switch.call(&JsValue::undefined(), &[], context)?;
                let selected =
                    reactive::callable(Some(&select))?.call(&JsValue::undefined(), &[], context)?;
                if !selected.to_boolean() {
                    return get(&object(data[1].clone())?, "fallback", context);
                }
                let selected = object(selected)?;
                let index = selected.get(0u32, context)?;
                let condition = selected.get(1u32, context)?;
                let props = object(selected.get(2u32, context)?)?;
                let child = get(&props, "children", context)?;
                if let Some(render) = child.as_callable()
                    && get(&render, "length", context)?.to_length(context)? > 0
                {
                    let argument =
                        if get(&props, "keyed", context)?.to_boolean() {
                            reactive::callable(Some(&condition))?.call(
                                &JsValue::undefined(),
                                &[],
                                context,
                            )?
                        } else {
                            reactive::function(
                                context,
                                NativeFunction::from_copy_closure_with_captures(
                                    |_, _, data: &Vec<JsValue>, context| {
                                        let selected =
                                            reactive::untracked(context, |context| {
                                                let select = reactive::callable(Some(&data[0]))?
                                                    .call(&JsValue::undefined(), &[], context)?;
                                                reactive::callable(Some(&select))?.call(
                                                    &JsValue::undefined(),
                                                    &[],
                                                    context,
                                                )
                                            })?;
                                        if !selected
                                            .as_object()
                                            .map(|value| value.get(0u32, context))
                                            .transpose()?
                                            .is_some_and(|index| index.strict_equals(&data[1]))
                                        {
                                            return Err(error(
                                                "Match accessor read outside its active branch",
                                            ));
                                        }
                                        reactive::callable(Some(&data[2]))?.call(
                                            &JsValue::undefined(),
                                            &[],
                                            context,
                                        )
                                    },
                                    vec![switch.into(), index, condition],
                                ),
                                0,
                            )
                            .into()
                        };
                    reactive::untracked(context, |context| {
                        render.call(&JsValue::undefined(), &[argument], context)
                    })
                } else {
                    Ok(child)
                }
            },
            vec![switch.into(), props.into()],
        ),
        0,
    );
    Ok(reactive::memo(render, JsValue::undefined(), None, false, context)?.into())
}

pub const FUNCTIONS: &[(&str, boa_engine::native_function::NativeFunctionPointer)] = &[
    ("For", for_api),
    ("Index", index_api),
    ("Show", show_api),
    ("Switch", switch_api),
    ("Match", match_api),
    ("children", children_api),
];

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Rust implementation of the project reactive host. JavaScript values are
//! rooted by Context-owned state; native callbacks capture only generation keys.
use boa_engine::{
    Context, JsNativeError, JsResult, JsValue, NativeFunction, js_string,
    object::{
        FunctionObjectBuilder, JsObject, ObjectInitializer,
        builtins::{JsArray, JsPromise},
    },
    property::Attribute,
};
use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    rc::Rc,
};

pub fn error(message: impl Into<String>) -> boa_engine::JsError {
    JsNativeError::typ().with_message(message.into()).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(u64);
impl Key {
    pub(crate) fn value(self) -> JsValue {
        js_string!(self.0.to_string()).into()
    }
    pub(crate) fn from_value(value: &JsValue) -> JsResult<Self> {
        value
            .as_string()
            .and_then(|value| value.to_std_string_escaped().parse().ok())
            .map(Self)
            .ok_or_else(|| error("Invalid owner token"))
    }
}
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}
struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<usize>,
}
impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }
}
impl<T> Arena<T> {
    fn insert(&mut self, value: T) -> Key {
        let index = self.free.pop().unwrap_or_else(|| {
            self.slots.push(Slot {
                generation: 0,
                value: None,
            });
            self.slots.len() - 1
        });
        let slot = &mut self.slots[index];
        slot.generation = slot.generation.wrapping_add(1);
        slot.value = Some(value);
        Key((u64::from(slot.generation) << 32) | index as u64)
    }
    fn get(&self, key: Key) -> Option<&T> {
        let slot = self.slots.get(key.0 as u32 as usize)?;
        (slot.generation == (key.0 >> 32) as u32)
            .then_some(slot.value.as_ref())
            .flatten()
    }
    fn get_mut(&mut self, key: Key) -> Option<&mut T> {
        let slot = self.slots.get_mut(key.0 as u32 as usize)?;
        if slot.generation == (key.0 >> 32) as u32 {
            slot.value.as_mut()
        } else {
            None
        }
    }
    fn remove(&mut self, key: Key) -> Option<T> {
        let index = key.0 as u32 as usize;
        let slot = self.slots.get_mut(index)?;
        if slot.generation != (key.0 >> 32) as u32 {
            return None;
        }
        let value = slot.value.take()?;
        self.free.push(index);
        Some(value)
    }
}

#[derive(Default)]
struct Owner {
    parent: Option<Key>,
    children: Vec<Key>,
    signals: Vec<Key>,
    computations: Vec<Key>,
    cleanups: Vec<JsObject>,
}
struct Signal {
    value: JsValue,
    equals: Option<JsValue>,
    observers: BTreeSet<Key>,
    computation: Option<Key>,
}
struct Computation {
    callback: JsObject,
    value: JsValue,
    output: Option<Key>,
    owner: Key,
    dependencies: BTreeSet<Key>,
    dirty: bool,
    running: bool,
}

pub struct State {
    owners: Arena<Owner>,
    signals: Arena<Signal>,
    computations: Arena<Computation>,
    pub root: Key,
    owner: Option<Key>,
    listener: Option<Key>,
    batch: usize,
    executing: usize,
    flushing: bool,
    mounting: bool,
    memos: VecDeque<Key>,
    effects: VecDeque<Key>,
    pub mounted: bool,
    pub offline: bool,
    pub holds: Vec<JsPromise>,
    pub revision: u64,
    ticker: Option<[Key; 4]>,
    tick_values: [JsValue; 4],
    pub node_serial: u64,
    pull_depth: usize,
}
pub type Host = Rc<RefCell<State>>;
impl State {
    pub fn uses_ticker(&self) -> bool {
        self.ticker.is_some()
    }
}

pub fn host(context: &Context) -> JsResult<Host> {
    context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| error("No mounted native reactive host"))
}

pub fn install(context: &mut Context, offline: bool) -> Host {
    let mut owners = Arena::default();
    let root = owners.insert(Owner::default());
    let state = Rc::new(RefCell::new(State {
        owners,
        root,
        signals: Arena::default(),
        computations: Arena::default(),
        owner: Some(root),
        listener: None,
        batch: 0,
        executing: 0,
        flushing: false,
        mounting: true,
        memos: VecDeque::new(),
        effects: VecDeque::new(),
        mounted: false,
        offline,
        holds: Vec::new(),
        revision: 0,
        ticker: None,
        tick_values: [0.0.into(), 0.0.into(), 0.0.into(), false.into()],
        node_serial: 0,
        pull_depth: 0,
    }));
    context.insert_data(state.clone());
    state
}

pub fn function(context: &Context, function: NativeFunction, length: usize) -> JsObject {
    FunctionObjectBuilder::new(context.realm(), function)
        .length(length)
        .build()
        .into()
}

pub fn current_owner(context: &Context) -> JsResult<Key> {
    let state = host(context)?;
    let state = state.borrow();
    Ok(state.owner.unwrap_or(state.root))
}

pub fn new_owner(parent: Key, context: &Context) -> JsResult<Key> {
    let state = host(context)?;
    let mut state = state.borrow_mut();
    if state.owners.get(parent).is_none() {
        return Err(error("Reactive owner has been disposed"));
    }
    let child = state.owners.insert(Owner {
        parent: Some(parent),
        ..Owner::default()
    });
    state.owners.get_mut(parent).unwrap().children.push(child);
    Ok(child)
}

pub fn with_owner<T>(
    owner: Option<Key>,
    listener: Option<Key>,
    context: &mut Context,
    run: impl FnOnce(&mut Context) -> JsResult<T>,
) -> JsResult<T> {
    let state = host(context)?;
    let previous = {
        let mut state = state.borrow_mut();
        let previous = (state.owner, state.listener);
        state.owner = owner;
        state.listener = listener;
        previous
    };
    let result = run(context);
    let mut state = state.borrow_mut();
    (state.owner, state.listener) = previous;
    result
}

pub fn untracked<T>(
    context: &mut Context,
    run: impl FnOnce(&mut Context) -> JsResult<T>,
) -> JsResult<T> {
    let owner = host(context)?.borrow().owner;
    with_owner(owner, None, context, run)
}

fn remove_computation(key: Key, state: &mut State) {
    if let Some(computation) = state.computations.remove(key) {
        for source in computation.dependencies {
            if let Some(source) = state.signals.get_mut(source) {
                source.observers.remove(&key);
            }
        }
    }
}

fn dispose_contents(owner: Key, context: &mut Context) -> JsResult<()> {
    let state = host(context)?;
    let contents = {
        let mut state = state.borrow_mut();
        let Some(owner) = state.owners.get_mut(owner) else {
            return Ok(());
        };
        let parent = owner.parent;
        std::mem::replace(
            owner,
            Owner {
                parent,
                ..Owner::default()
            },
        )
    };
    let mut first_error = None;
    {
        let mut state = state.borrow_mut();
        for computation in contents.computations {
            remove_computation(computation, &mut state);
        }
    }
    for child in contents.children.into_iter().rev() {
        if let Err(error) = dispose_owner(child, context) {
            first_error.get_or_insert(error);
        }
    }
    for cleanup in contents.cleanups.into_iter().rev() {
        if let Err(error) = untracked(context, |context| {
            cleanup.call(&JsValue::undefined(), &[], context)
        }) {
            first_error.get_or_insert(error);
        }
    }
    for source in contents.signals {
        state.borrow_mut().signals.remove(source);
    }
    first_error.map_or(Ok(()), Err)
}

pub fn dispose_owner(owner: Key, context: &mut Context) -> JsResult<()> {
    crate::stack::recursive(|| batch(context, |context| dispose_owner_inner(owner, context)))
}

fn dispose_owner_inner(owner: Key, context: &mut Context) -> JsResult<()> {
    let state = host(context)?;
    let parent = state
        .borrow()
        .owners
        .get(owner)
        .and_then(|owner| owner.parent);
    let result = dispose_contents(owner, context);
    let mut state = state.borrow_mut();
    state.owners.remove(owner);
    if let Some(parent) = parent.and_then(|parent| state.owners.get_mut(parent)) {
        parent.children.retain(|key| *key != owner);
    }
    result
}

fn signal(value: JsValue, equals: Option<JsValue>, owner: Key, state: &mut State) -> JsResult<Key> {
    if state.owners.get(owner).is_none() {
        return Err(error("Reactive owner has been disposed"));
    }
    let key = state.signals.insert(Signal {
        value,
        equals,
        observers: BTreeSet::new(),
        computation: None,
    });
    state.owners.get_mut(owner).unwrap().signals.push(key);
    Ok(key)
}

pub fn read(key: Key, context: &mut Context) -> JsResult<JsValue> {
    let state = host(context)?;
    let computation = state
        .borrow()
        .signals
        .get(key)
        .ok_or_else(|| error("Signal belongs to a disposed scope"))?
        .computation;
    if let Some(computation) = computation {
        update(computation, context)?;
    }
    let mut state = state.borrow_mut();
    if let Some(listener) = state.listener {
        state
            .signals
            .get_mut(key)
            .ok_or_else(|| error("Disposed signal"))?
            .observers
            .insert(listener);
        if let Some(listener) = state.computations.get_mut(listener) {
            listener.dependencies.insert(key);
        }
    }
    Ok(state
        .signals
        .get(key)
        .ok_or_else(|| error("Disposed signal"))?
        .value
        .clone())
}

pub fn write(key: Key, value: JsValue, context: &mut Context) -> JsResult<JsValue> {
    let state = host(context)?;
    let (previous, equals) = {
        let state = state.borrow();
        let signal = state
            .signals
            .get(key)
            .ok_or_else(|| error("Signal belongs to a disposed scope"))?;
        (signal.value.clone(), signal.equals.clone())
    };
    let equal = match equals {
        None => previous.strict_equals(&value),
        Some(equals) if equals.as_boolean() == Some(false) => false,
        Some(equals) => untracked(context, |context| {
            equals
                .as_callable()
                .ok_or_else(|| error("Signal equals must be false or a function"))?
                .call(&JsValue::undefined(), &[previous, value.clone()], context)
                .map(|value| value.to_boolean())
        })?,
    };
    if !equal {
        let mut state = state.borrow_mut();
        let signal = state
            .signals
            .get_mut(key)
            .ok_or_else(|| error("Disposed signal"))?;
        signal.value = value.clone();
        let observers = signal.observers.clone();
        state.revision = state.revision.wrapping_add(1);
        for key in observers {
            if let Some(computation) = state.computations.get_mut(key)
                && !computation.dirty
            {
                computation.dirty = true;
                if computation.output.is_some() {
                    state.memos.push_back(key);
                } else {
                    state.effects.push_back(key);
                }
            }
        }
    }
    flush(context)?;
    Ok(value)
}

fn update(key: Key, context: &mut Context) -> JsResult<()> {
    crate::stack::recursive(|| update_bounded(key, context))
}
fn update_bounded(key: Key, context: &mut Context) -> JsResult<()> {
    let state = host(context)?;
    if state.borrow().pull_depth >= 128 {
        return Err(error("Reactive dependency chain exceeds 128 levels"));
    }
    state.borrow_mut().pull_depth += 1;
    let result = update_inner(key, context);
    state.borrow_mut().pull_depth -= 1;
    result
}

fn update_inner(key: Key, context: &mut Context) -> JsResult<()> {
    let state = host(context)?;
    // A downstream memo can be read inside a batch before its upstream memo
    // reaches the queue. Pull producers first; equality still suppresses effects.
    let dependencies = {
        let state = state.borrow();
        let Some(computation) = state.computations.get(key) else {
            return Ok(());
        };
        if computation.running {
            return Err(error("Circular reactive computation"));
        }
        computation
            .dependencies
            .iter()
            .filter_map(|source| {
                state
                    .signals
                    .get(*source)
                    .and_then(|source| source.computation)
            })
            .collect::<Vec<_>>()
    };
    for dependency in dependencies {
        update(dependency, context)?;
    }
    let (callback, value, owner, output, dependencies) = {
        let mut state = state.borrow_mut();
        let Some(computation) = state.computations.get_mut(key) else {
            return Ok(());
        };
        if computation.running {
            return Err(error("Circular reactive computation"));
        }
        if !computation.dirty {
            return Ok(());
        }
        computation.running = true;
        computation.dirty = false;
        let values = (
            computation.callback.clone(),
            computation.value.clone(),
            computation.owner,
            computation.output,
            std::mem::take(&mut computation.dependencies),
        );
        state.executing += 1;
        values
    };
    for source in dependencies {
        if let Some(source) = state.borrow_mut().signals.get_mut(source) {
            source.observers.remove(&key);
        }
    }
    let result = dispose_contents(owner, context).and_then(|()| {
        with_owner(Some(owner), Some(key), context, |context| {
            callback.call(&JsValue::undefined(), &[value], context)
        })
    });
    {
        let mut state = state.borrow_mut();
        state.executing -= 1;
        if let Some(computation) = state.computations.get_mut(key) {
            computation.running = false;
        }
    }
    let value = result?;
    if let Some(computation) = state.borrow_mut().computations.get_mut(key) {
        computation.value = value.clone();
    }
    if let Some(output) = output {
        write(output, value, context)?;
    }
    Ok(())
}

pub fn flush(context: &mut Context) -> JsResult<()> {
    let state = host(context)?;
    {
        let mut state = state.borrow_mut();
        if state.flushing || state.batch > 0 || state.executing > 0 || state.mounting {
            return Ok(());
        }
        state.flushing = true;
    }
    let result = (|| {
        for _ in 0..10_000 {
            let next = {
                let mut state = state.borrow_mut();
                state
                    .memos
                    .pop_front()
                    .or_else(|| state.effects.pop_front())
            };
            let Some(next) = next else {
                return Ok(());
            };
            update(next, context)?;
        }
        Err(error(
            "Reactive update exceeded the 10,000 computation limit",
        ))
    })();
    state.borrow_mut().flushing = false;
    result
}

pub fn finish_mount(context: &mut Context) -> JsResult<()> {
    host(context)?.borrow_mut().mounting = false;
    flush(context)
}

fn options_equals(options: Option<&JsValue>, context: &mut Context) -> JsResult<Option<JsValue>> {
    let Some(options) = options.and_then(JsValue::as_object) else {
        return Ok(None);
    };
    let value = options.get(js_string!("equals"), context)?;
    if value.is_undefined() {
        return Ok(None);
    }
    if value.as_boolean() == Some(false) || value.is_callable() {
        Ok(Some(value))
    } else {
        Err(error("equals must be false or a comparator function"))
    }
}

fn getter(key: Key, context: &Context) -> JsObject {
    function(
        context,
        NativeFunction::from_copy_closure(move |_, _, context| read(key, context)),
        0,
    )
}

pub fn memo(
    callback: JsObject,
    initial: JsValue,
    equals: Option<JsValue>,
    lazy: bool,
    context: &mut Context,
) -> JsResult<JsObject> {
    let (key, output) = computation(callback, initial, equals, true, context)?;
    if !lazy {
        update(key, context)?;
    }
    Ok(getter(output.unwrap(), context))
}

fn computation(
    callback: JsObject,
    initial: JsValue,
    equals: Option<JsValue>,
    memo: bool,
    context: &mut Context,
) -> JsResult<(Key, Option<Key>)> {
    let parent = current_owner(context)?;
    let owner = new_owner(parent, context)?;
    let state = host(context)?;
    let mut state = state.borrow_mut();
    let output = if memo {
        Some(signal(initial.clone(), equals, parent, &mut state)?)
    } else {
        None
    };
    let key = state.computations.insert(Computation {
        callback,
        value: initial,
        output,
        owner,
        dependencies: BTreeSet::new(),
        dirty: true,
        running: false,
    });
    state.owners.get_mut(parent).unwrap().computations.push(key);
    if let Some(output) = output {
        state.signals.get_mut(output).unwrap().computation = Some(key);
    }
    Ok((key, output))
}

pub fn create_signal(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = args.first().cloned().unwrap_or_default();
    let equals = options_equals(args.get(1), context)?;
    let owner = current_owner(context)?;
    let key = signal(value, equals, owner, &mut host(context)?.borrow_mut())?;
    let read = getter(key, context);
    let set = function(
        context,
        NativeFunction::from_copy_closure(move |_, args, context| {
            let next = args.first().cloned().unwrap_or_default();
            let next = if let Some(callback) = next.as_callable() {
                let previous = untracked(context, |context| read_signal_value(key, context))?;
                untracked(context, |context| {
                    callback.call(&JsValue::undefined(), &[previous], context)
                })?
            } else {
                next
            };
            write(key, next, context)
        }),
        1,
    );
    Ok(JsArray::from_iter([read.into(), set.into()], context).into())
}

fn read_signal_value(key: Key, context: &mut Context) -> JsResult<JsValue> {
    read(key, context)
}

pub fn create_memo(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    let equals = options_equals(args.get(2), context)?;
    Ok(memo(
        callback,
        args.get(1).cloned().unwrap_or_default(),
        equals,
        false,
        context,
    )?
    .into())
}

pub fn callable(value: Option<&JsValue>) -> JsResult<JsObject> {
    value
        .and_then(JsValue::as_callable)
        .ok_or_else(|| error("Expected a function"))
}

pub fn create_effect(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let (key, _) = computation(
        callable(args.first())?,
        args.get(1).cloned().unwrap_or_default(),
        None,
        false,
        context,
    )?;
    host(context)?.borrow_mut().effects.push_back(key);
    flush(context)?;
    Ok(JsValue::undefined())
}

pub fn create_render_effect(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (key, _) = computation(
        callable(args.first())?,
        args.get(1).cloned().unwrap_or_default(),
        None,
        false,
        context,
    )?;
    update(key, context)?;
    Ok(JsValue::undefined())
}

pub fn on_cleanup(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    let state = host(context)?;
    let mut state = state.borrow_mut();
    if let Some(owner) = state.owner.and_then(|owner| state.owners.get_mut(owner)) {
        owner.cleanups.push(callback.clone());
    }
    Ok(callback.into())
}

pub fn on_mount(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    let callback = function(
        context,
        NativeFunction::from_copy_closure_with_captures(
            |_, _, callback, context| {
                untracked(context, |context| {
                    callback.call(&JsValue::undefined(), &[], context)
                })
            },
            callback,
        ),
        0,
    );
    create_effect(&JsValue::undefined(), &[callback.into()], context)
}

pub fn batch<T>(
    context: &mut Context,
    run: impl FnOnce(&mut Context) -> JsResult<T>,
) -> JsResult<T> {
    let state = host(context)?;
    state.borrow_mut().batch += 1;
    let result = run(context);
    state.borrow_mut().batch -= 1;
    let flushed = flush(context);
    result.and_then(|value| flushed.map(|()| value))
}

pub fn batch_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    batch(context, |context| {
        callback.call(&JsValue::undefined(), &[], context)
    })
}

pub fn untrack_api(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    untracked(context, |context| {
        callback.call(&JsValue::undefined(), &[], context)
    })
}

pub fn create_root(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = callable(args.first())?;
    let parent = current_owner(context)?;
    let owner = new_owner(parent, context)?;
    let dispose = function(
        context,
        NativeFunction::from_copy_closure(move |_, _, context| {
            dispose_owner(owner, context)?;
            Ok(JsValue::undefined())
        }),
        0,
    );
    with_owner(Some(owner), None, context, |context| {
        callback.call(&JsValue::undefined(), &[dispose.into()], context)
    })
}

pub fn use_ticker(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let state = host(context)?;
    if !state.borrow().mounted {
        return Err(error(
            "useTicker only works inside a mounted project component",
        ));
    }
    let current_keys = state.borrow().ticker;
    let keys = if let Some(keys) = current_keys {
        keys
    } else {
        let mut state = state.borrow_mut();
        let owner = state.root;
        let values = state.tick_values.clone();
        let keys = [
            signal(values[0].clone(), None, owner, &mut state)?,
            signal(values[1].clone(), None, owner, &mut state)?,
            signal(values[2].clone(), None, owner, &mut state)?,
            signal(values[3].clone(), None, owner, &mut state)?,
        ];
        state.ticker = Some(keys);
        keys
    };
    let values = keys.map(|key| getter(key, context));
    Ok(ObjectInitializer::new(context)
        .property(js_string!("time"), values[0].clone(), Attribute::all())
        .property(js_string!("frame"), values[1].clone(), Attribute::all())
        .property(js_string!("delta"), values[2].clone(), Attribute::all())
        .property(js_string!("playing"), values[3].clone(), Attribute::all())
        .function(
            NativeFunction::from_fn_ptr(|_, args, context| {
                let state = host(context)?;
                if state.borrow().offline {
                    let promise =
                        JsPromise::resolve(args.first().cloned().unwrap_or_default(), context)?;
                    state.borrow_mut().holds.push(promise);
                }
                Ok(JsValue::undefined())
            }),
            js_string!("hold"),
            1,
        )
        .build()
        .into())
}

pub fn tick(time: f64, delta: f64, playing: bool, context: &mut Context) -> JsResult<()> {
    tick_at_rate(time, delta, playing, 30.0, context)
}
pub fn tick_at_rate(
    time: f64,
    delta: f64,
    playing: bool,
    fps: f64,
    context: &mut Context,
) -> JsResult<()> {
    if !fps.is_finite() || fps <= 0.0 || fps > 240.0 {
        return Err(error("FPS must be in (0, 240]"));
    }
    if !time.is_finite() || !delta.is_finite() || !(time * fps).is_finite() {
        return Err(error("Non-finite project tick"));
    }
    let values = [
        time.into(),
        (time * fps + 0.5).floor().into(),
        delta.into(),
        playing.into(),
    ];
    let state = host(context)?;
    state.borrow_mut().tick_values = values.clone();
    let keys = state.borrow().ticker;
    if let Some(keys) = keys {
        batch(context, |context| {
            for (key, value) in keys.into_iter().zip(values) {
                write(key, value, context)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub const SOLID_FUNCTIONS: &[(&str, boa_engine::native_function::NativeFunctionPointer)] = &[
    ("createSignal", create_signal),
    ("createMemo", create_memo),
    ("createEffect", create_effect),
    ("createRenderEffect", create_render_effect),
    ("createComputed", create_render_effect),
    ("onCleanup", on_cleanup),
    ("onMount", on_mount),
    ("batch", batch_api),
    ("untrack", untrack_api),
    ("createRoot", create_root),
];

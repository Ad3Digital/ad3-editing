use ad3_editor::{
    evaluator::{LiveProject, evaluate},
    reactive,
};
use boa_engine::{Context, NativeFunction, Source, js_string};
use serde_json::json;
use std::path::Path;

fn context() -> Context {
    let mut context = Context::default();
    reactive::install(&mut context, false);
    for &(name, callback) in reactive::SOLID_FUNCTIONS {
        context
            .register_global_builtin_callable(
                js_string!(name),
                1,
                NativeFunction::from_fn_ptr(callback),
            )
            .unwrap();
    }
    context
}
fn eval(context: &mut Context, code: &str) -> String {
    context
        .eval(Source::from_bytes(code))
        .unwrap()
        .to_string(context)
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn signals_memos_batches_and_conditional_dependencies() {
    let mut c = context();
    eval(
        &mut c,
        r#"
        const [a,setA]=createSignal(1), [b,setB]=createSignal(10), [useA,setUseA]=createSignal(true);
        const sum=createMemo(()=>a()+b()), twice=createMemo(()=>sum()*2);
        const parity=createMemo(()=>a()%2);
        const selected=createMemo(()=>useA()?a():b());
        let log=[], parityRuns=0;
        createEffect(()=>log.push([a(),sum(),twice(),selected()]));
        createEffect(()=>{parity();parityRuns++});
    "#,
    );
    assert_eq!(eval(&mut c, "log.length"), "0");
    reactive::finish_mount(&mut c).unwrap();
    assert_eq!(eval(&mut c, "JSON.stringify(log)"), "[[1,11,22,1]]");
    assert_eq!(
        eval(&mut c, "batch(()=>{setA(3);setB(20);return twice()})"),
        "46"
    );
    assert_eq!(
        eval(&mut c, "JSON.stringify(log)"),
        "[[1,11,22,1],[3,23,46,3]]"
    );
    assert_eq!(eval(&mut c, "parityRuns"), "1");
    eval(&mut c, "setUseA(false);setA(5)");
    assert_eq!(eval(&mut c, "selected()"), "20");
    assert_eq!(eval(&mut c, "setA(v=>v+1); a()"), "6");
    assert_eq!(eval(&mut c, "parityRuns"), "2");
}

#[test]
fn owners_cleanup_nested_effects_and_disposal() {
    let mut c = context();
    eval(
        &mut c,
        r#"
        let log=[], dispose, set, setChild;
        createRoot(d=>{
            dispose=d;
            const [a,s]=createSignal(0); set=s;
            const [child,sc]=createSignal(0); setChild=sc;
            onCleanup(()=>log.push('root'));
            createEffect(()=>{
                const value=a();
                onCleanup(()=>log.push('cleanup'+value));
                createEffect(()=>log.push(value+':'+child()));
            });
        });
    "#,
    );
    reactive::finish_mount(&mut c).unwrap();
    eval(&mut c, "set(1);setChild(2);dispose();dispose()");
    assert_eq!(
        eval(&mut c, "JSON.stringify(log)"),
        r#"["0:0","cleanup0","1:0","1:2","cleanup1","root"]"#
    );
    assert!(c.eval(Source::from_bytes("set(2)")).is_err());
}

#[test]
fn live_ticker_keeps_components_and_lazy_props_between_frames() {
    let source = r#"
        import {createSignal, createEffect, createMemo, onMount, mergeProps, splitProps} from 'solid-js';
        import {useTicker, TIME_FPS} from '@diffusionstudio/jsx';
        let mounts=0;
        function Label(props){
            mounts++;
            const [first,rest]=splitProps(props,['x']);
            return <text id="label" x={first.x} color={rest.color}>{props.children}</text>;
        }
        export default function Project(){
            const ticker=useTicker();
            const [count,setCount]=createSignal(0);
            onMount(()=>setCount(10));
            createEffect(()=>{ticker.frame();setCount(n=>n+1)});
            const x=createMemo(()=>ticker.time()*100);
            const props=mergeProps({color:'red'}, {get x(){return x()}});
            return <stage><scene id="s" duration={5}>
                <Label {...props} color={undefined}>{count()}:{ticker.frame()}:{mounts}:{TIME_FPS}</Label>
                <rect id="clock" x={ticker.delta()} opacity={ticker.playing()?1:0}/>
            </scene></stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("reactive.tsx"), source, false).unwrap();
    let first = live.sample(0.0, false).unwrap();
    assert_eq!(first.find("label").unwrap().text, "11:0:1:30");
    let second = live.sample(1.0, true).unwrap();
    let label = second.find("label").unwrap();
    assert_eq!(label.text, "12:30:1:30");
    assert_eq!(label.props["x"], json!(100.0));
    assert_eq!(label.props["color"], json!("red"));
    assert!(
        label
            .location
            .file
            .as_ref()
            .unwrap()
            .ends_with("reactive.tsx")
    );
    assert_eq!(
        &source[label.location.element.clone()],
        "<text id=\"label\" x={first.x} color={rest.color}>{props.children}</text>"
    );
    assert_eq!(second.find("clock").unwrap().props["x"], json!(1.0));
    assert_eq!(second.find("clock").unwrap().props["opacity"], json!(1.0));
    let backwards = live.sample(0.5, false).unwrap();
    assert_eq!(backwards.find("clock").unwrap().props["x"], json!(-0.5));
    assert_eq!(backwards.find("label").unwrap().text, "13:15:1:30");
    let mut fresh = LiveProject::mount(Path::new("reactive.tsx"), source, true).unwrap();
    assert_eq!(
        fresh
            .sample(0.0, false)
            .unwrap()
            .find("label")
            .unwrap()
            .text,
        "11:0:1:30"
    );
}

#[test]
fn offline_holds_settle_before_sample_and_hook_requires_mount() {
    let source = r#"
        import {createSignal,createEffect} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        export default ()=>{
            const ticker=useTicker(), [value,setValue]=createSignal(-1);
            createEffect(()=>{const time=ticker.time();ticker.hold(Promise.resolve().then(()=>setValue(time*2)))});
            return <stage><scene><rect id="r" x={value()}/></scene></stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("hold.tsx"), source, true).unwrap();
    assert_eq!(
        live.sample(2.0, false).unwrap().find("r").unwrap().props["x"],
        json!(4.0)
    );
    assert!(live.warnings.is_empty());
    let error=evaluate(Path::new("bad.tsx"),"import {useTicker} from '@diffusionstudio/jsx'; const t=useTicker(); export default ()=> <stage/>;").err().unwrap();
    assert!(error.contains("mounted project"), "{error}");
}

#[test]
fn keyed_lists_preserve_instances_indexes_and_cleanup() {
    let source = r#"
        import {For,onCleanup} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        let mounts=0,cleaned=[];
        const a={name:'a'},b={name:'b'},c={name:'c'};
        function Item(p){
            const instance=++mounts;
            onCleanup(()=>cleaned.push(p.item.name));
            return <text id={p.item.name} x={p.index()*10}>{instance}:{p.index()}</text>;
        }
        export default ()=>{
            const t=useTicker();
            const items=()=>t.time()<1?[a,b]:t.time()<2?[b,a,c]:[c,b];
            return <stage><scene><For each={items()}>{(item,index)=><Item item={item} index={index}/>}</For>
                <text id="stats">{mounts}:{cleaned.join(',')}</text>
            </scene></stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("for.tsx"), source, false).unwrap();
    let first = live.sample(0.0, false).unwrap();
    assert_eq!(first.find("a").unwrap().text, "1:0");
    assert_eq!(first.find("b").unwrap().text, "2:1");
    let next = live.sample(1.0, false).unwrap();
    assert_eq!(next.find("a").unwrap().text, "1:1");
    assert_eq!(next.find("b").unwrap().text, "2:0");
    assert_eq!(next.find("c").unwrap().text, "3:2");
    let last = live.sample(2.0, false).unwrap();
    assert!(last.find("a").is_none());
    assert_eq!(last.find("c").unwrap().text, "3:0");
    // Plain external variables are not reactive; read through an ordinary prop.
    assert_eq!(last.find("b").unwrap().props["x"], json!(10.0));
}

#[test]
fn index_and_show_own_the_correct_branch_lifetime() {
    let source = r#"
        import {Index,Show,createSignal,onCleanup} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        let mounted=0;
        export default ()=>{
            const t=useTicker(),[cleaned,setCleaned]=createSignal(0);
            const Branch=p=>{
                const n=++mounted;
                onCleanup(()=>setCleaned(v=>v+1));
                return <text id="branch">{n}:{p.value()}</text>;
            };
            return <stage><scene>
                <Index each={t.time()<1?['a','b']:['b','a']}>
                    {(item,index)=><text id={'item'+index}>{item()}:{index}</text>}
                </Index>
                <Show when={t.time()>0?t.time():false} fallback={<rect id="fallback"/>}>
                    {value=><Branch value={value}/>}
                </Show>
                <text id="cleaned">{cleaned()}</text>
            </scene></stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("show.tsx"), source, false).unwrap();
    let first = live.sample(0.0, false).unwrap();
    assert!(first.find("fallback").is_some());
    assert_eq!(first.find("item0").unwrap().text, "a:0");
    let next = live.sample(1.0, false).unwrap();
    assert_eq!(next.find("item0").unwrap().text, "b:0");
    assert_eq!(next.find("branch").unwrap().text, "1:1");
    assert_eq!(
        live.sample(2.0, false)
            .unwrap()
            .find("branch")
            .unwrap()
            .text,
        "1:2"
    );
    assert_eq!(
        live.sample(0.0, false)
            .unwrap()
            .find("cleaned")
            .unwrap()
            .text,
        "1"
    );
    assert_eq!(
        live.sample(1.0, false)
            .unwrap()
            .find("branch")
            .unwrap()
            .text,
        "2:1"
    );
}

#[test]
fn new_branches_finish_descendants_before_mount_effects_and_cleanup_reads_local_state() {
    let source = r#"
        import {Show,onMount,onCleanup,createSignal} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        const [log,setLog]=createSignal([]);
        const record=value=>setLog(items=>[...items,value]);
        const Leaf=()=>{record('leaf');return <rect/>};
        const Branch=()=>{
            const [value]=createSignal(42);
            onMount(()=>record('mounted'));
            onCleanup(()=>record('clean'+value()));
            return <group><Leaf/></group>;
        };
        export default ()=>{
            const t=useTicker();
            return <stage log={log().join(',')}><scene>
                <Show when={t.time()>0}><Branch/></Show>
                <text id="childprop" children={t.frame()}/>
            </scene></stage>;
        };
    "#;
    let mut live = LiveProject::mount(Path::new("lifecycle.tsx"), source, false).unwrap();
    let first = live.sample(1.0, true).unwrap();
    assert_eq!(first.props["log"], json!("leaf,mounted"));
    assert_eq!(first.find("childprop").unwrap().text, "30");
    let last = live.sample(0.0, false).unwrap();
    assert_eq!(last.props["log"], json!("leaf,mounted,clean42"));
}

#[test]
fn authoring_parse_time_matches_javascript_contract() {
    let source = r#"
        import {parseTime} from '@diffusionstudio/jsx';
        export default ()=> <stage times={['30f','1.5f','1.f','-01:02','1:','0:-3','2seconds','garbage',null,Infinity].map(parseTime)}/>;
    "#;
    let result = evaluate(Path::new("time.tsx"), source).unwrap();
    assert_eq!(
        result.root.props["times"],
        json!([1.0, 0.05, 1.0, -62.0, 60.0, -3.0, 2.0, null, null, null])
    );
}

#[test]
fn frame_boundaries_invalid_clocks_and_sample_independence() {
    let source = r#"import {useTicker} from '@diffusionstudio/jsx'; export default ()=>{const t=useTicker();return <stage><scene><text id="t">{t.frame()}</text></scene></stage>};"#;
    let mut live = LiveProject::mount(Path::new("frames.tsx"), source, false).unwrap();
    let mut before = live.sample(0.5 / 30.0 - 1e-12, false).unwrap();
    assert_eq!(before.find("t").unwrap().text, "0");
    before.children.clear();
    let after = live.sample(0.5 / 30.0 + 1e-12, false).unwrap();
    assert_eq!(after.find("t").unwrap().text, "1");
    assert_eq!(
        live.sample_at_rate(1.0, true, 24.0)
            .unwrap()
            .find("t")
            .unwrap()
            .text,
        "24"
    );
    let mut c = context();
    for fps in [0.0, -1.0, 241.0, f64::NAN, f64::INFINITY] {
        assert!(reactive::tick_at_rate(0.0, 0.0, false, fps, &mut c).is_err());
        assert!(live.sample_at_rate(0.0, false, fps).is_err());
    }
    assert!(reactive::tick_at_rate(f64::MAX, 0.0, false, 30.0, &mut c).is_err());
}

#[test]
fn an_offline_hold_can_be_cancelled_while_mounting() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    let source = r#"import {useTicker} from '@diffusionstudio/jsx'; export default ()=>{useTicker().hold(new Promise(()=>{}));return <stage/>};"#;
    let cancel = Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(75));
        trigger.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let result = LiveProject::mount_frozen_cancellable(
        Path::new("held.tsx"),
        source,
        true,
        None,
        Some(cancel),
    );
    worker.join().unwrap();
    assert!(result.err().unwrap().contains("cancelled"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn property_bindings_track_signals_without_polling_plain_mutable_values() {
    let source = r#"
        import {createSignal,createEffect} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        export default ()=>{
            const t=useTicker(), plain={value:1}, [data,setData]=createSignal({value:1});
            createEffect(()=>{plain.value=t.time()+100;setData({value:t.time()+10})});
            return <stage><scene><rect id="r" x={plain.value} y={data().value} config={plain}/></scene></stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("props.tsx"), source, false).unwrap();
    let first = live.sample(0.0, false).unwrap();
    assert_eq!(first.find("r").unwrap().props["x"], json!(1.0));
    assert_eq!(first.find("r").unwrap().props["y"], json!(10.0));
    let next = live.sample(1.0, false).unwrap();
    assert_eq!(next.find("r").unwrap().props["x"], json!(1.0));
    assert_eq!(
        next.find("r").unwrap().props["config"],
        json!({"value":1.0})
    );
    assert_eq!(next.find("r").unwrap().props["y"], json!(11.0));
}

#[test]
fn child_accessors_and_show_fallbacks_track_dependencies_and_preserve_owners() {
    let source = r#"
        import {Show,children,onCleanup,createSignal} from 'solid-js';
        import {useTicker} from '@diffusionstudio/jsx';
        let mounts=0;
        const Child=p=>{const n=++mounts;return <text id="c">{n}:{p.time()}</text>};
        export default ()=>{
            const t=useTicker();
            const content=children(()=>[null,()=>[<Child time={t.time}/>]]);
            return <stage>
                <scene>
                    <group>{content}</group>
                    <text id="show"><Show when={t.time()>1} fallback={t.frame()}>{t.frame()+100}</Show></text>
                </scene>
            </stage>;
        }
    "#;
    let mut live = LiveProject::mount(Path::new("children.tsx"), source, false).unwrap();
    assert_eq!(
        live.sample(0.0, false).unwrap().find("c").unwrap().text,
        "1:0"
    );
    let next = live.sample(1.0, false).unwrap();
    assert_eq!(next.find("c").unwrap().text, "1:1");
    assert_eq!(next.find("show").unwrap().text, "30");
    assert_eq!(
        live.sample(2.0, false).unwrap().find("show").unwrap().text,
        "160"
    );
    assert_eq!(
        live.sample(3.0, false).unwrap().find("show").unwrap().text,
        "190"
    );
}

#[test]
fn spread_keys_are_added_removed_and_reactive_props_keep_separate_bindings() {
    let source = r#"
        import {useTicker} from '@diffusionstudio/jsx';
        export default ()=>{const t=useTicker();return <stage><scene><rect id="r" {...(t.time()<1?{x:1}:{y:2})}/></scene></stage>};
    "#;
    let mut live = LiveProject::mount(Path::new("spread.tsx"), source, false).unwrap();
    assert_eq!(
        live.sample(0.0, false).unwrap().find("r").unwrap().props["x"],
        json!(1.0)
    );
    let next = live.sample(1.0, false).unwrap();
    assert!(!next.find("r").unwrap().props.contains_key("x"));
    assert_eq!(next.find("r").unwrap().props["y"], json!(2.0));
}

#[test]
fn real_example_mounts_samples_and_disposes_on_a_small_windows_sized_stack() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let source = include_str!("../examples/reactive/index.tsx");
            let mut live = LiveProject::mount(Path::new("example.tsx"), source, false).unwrap();
            for frame in 0..20 {
                let root = live.sample(frame as f64 / 10.0, true).unwrap();
                assert!(root.find("three").is_some());
                assert!(root.find("ready").is_some());
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

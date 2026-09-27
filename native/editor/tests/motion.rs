use ad3_editor::{color, easing::Easing, text_motion};
use ad3_editor::{document::Node, motion::Motion, source::SourceDocument, timing::Timeline};
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

fn tree(body: &str) -> Node {
    SourceDocument::parse(
        PathBuf::from("motion.tsx"),
        format!("export default <stage><scene id=\"s\" active end={{10}}>{body}</scene></stage>"),
    )
    .unwrap()
    .root
}
fn at(root: &Node, time: f64, fps: f64) -> Node {
    let timeline = Timeline::resolve_at_rate(root, &BTreeMap::new(), fps).unwrap();
    Motion::default()
        .sample(root, &timeline, time, fps)
        .unwrap()
}
fn number(root: &Node, id: &str, key: &str, default: f64) -> f64 {
    root.find(id).unwrap().number(key, default).unwrap()
}

#[test]
fn presets_match_original_ecs_function_at_every_phase_and_sample() {
    #[derive(Deserialize)]
    struct Case {
        kind: String,
        phase: String,
        frame: u32,
        duration: u32,
        text: String,
        values: BTreeMap<String, serde_json::Value>,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/presets.json")).unwrap();
    for case in cases {
        let root = tree(&format!(
            r#"<text id="target" end={{{}/30}} volume={{-6}}>{}<animation type="{}" phase="{}" duration={{{}/30}}/></text>"#,
            case.duration, case.text, case.kind, case.phase, case.duration
        ));
        let sampled = at(&root, f64::from(case.frame) / 30.0, 30.0);
        let node = sampled.find("target").unwrap();
        for (key, expected) in case.values {
            if key == "chars" {
                assert_eq!(
                    node.text,
                    expected.as_str().unwrap(),
                    "{} {} {}",
                    case.kind,
                    case.phase,
                    case.frame
                );
            } else if expected == "silence" {
                assert!(node.flag("muted"));
            } else {
                let actual = node.number(&key, f64::NAN).unwrap();
                assert!(
                    (actual - expected.as_f64().unwrap()).abs() < 1e-9,
                    "{} {} frame {} {key}: {actual} != {expected}",
                    case.kind,
                    case.phase,
                    case.frame
                );
            }
        }
        assert_eq!(root.find("target").unwrap().text, case.text);
    }
}

#[test]
fn animation_windows_reset_authored_values_on_reverse_seek_and_apply_in_child_order() {
    let root = tree(
        r#"<rect id="r" end={4} opacity={0.3} offsetX={9}>
      <animation type="slideLeft" duration={1} delay={0.5}/>
      <animation type="fade" phase="out" duration={1}/>
    </rect>"#,
    );
    let timeline = Timeline::resolve(&root, &BTreeMap::new()).unwrap();
    let mut motion = Motion::default();
    for time in [0.0, 2.0, 119.0 / 30.0, 0.0] {
        let sampled = motion.sample(&root, &timeline, time, 30.0).unwrap();
        let (opacity, offset) = if time == 0.0 {
            (0.0, 100.0)
        } else if time == 2.0 {
            (0.3, 9.0)
        } else {
            (0.0, 9.0)
        };
        assert_eq!(number(&sampled, "r", "opacity", 1.0), opacity);
        assert_eq!(number(&sampled, "r", "offsetX", 0.0), offset);
        assert!(!ad3_editor::motion::has_motion(&sampled));
    }
    assert_eq!(number(&root, "r", "opacity", 1.0), 0.3);
    let overlap = tree(
        r#"<rect id="r" end={1}><animation type="slideLeft"/><animation type="fade" phase="out"/></rect>"#,
    );
    assert_eq!(number(&at(&overlap, 0.0, 30.0), "r", "opacity", 0.0), 1.0);
    assert_eq!(number(&at(&overlap, 0.0, 30.0), "r", "offsetX", 0.0), 100.0);
}

#[test]
fn reconciler_fallbacks_keep_unknown_presets_as_fade_and_non_out_phases_as_in() {
    for props in [
        "",
        r#"type="unknown" phase="OUT""#,
        "type={false} phase={42}",
    ] {
        let root = tree(&format!(
            "<rect id=\"r\" end={{2}}><animation {props}/></rect>"
        ));
        assert_eq!(number(&at(&root, 0.0, 30.0), "r", "opacity", 1.0), 0.0);
        assert_eq!(
            number(&at(&root, 29.0 / 30.0, 30.0), "r", "opacity", 0.0),
            1.0
        );
    }
}

#[test]
fn keyframes_override_presets_sort_stably_hold_endpoints_and_use_segment_easing() {
    let root = tree(
        r#"<rect id="r" end={5} opacity={0.2} x={99}>
      <animation type="fade"/>
      <keyframeTrack property="opacity"><keyframe time={1} value={0.4}/></keyframeTrack>
      <keyframeTrack property="x">
        <keyframe time={2} value={100}/><keyframe time={1} value={10} easing="steps(2)"/>
        <keyframe time={1} value={20} easing="steps(2)"/>
      </keyframeTrack>
      <keyframeTrack property="y"/>
    </rect>"#,
    );
    for (time, expected) in [
        (0.0, 10.0),
        (1.0, 10.0),
        (1.25, 20.0),
        (1.5, 60.0),
        (2.0, 100.0),
        (4.0, 100.0),
    ] {
        let sample = at(&root, time, 30.0);
        assert_eq!(number(&sample, "r", "x", 0.0), expected, "time {time}");
        assert_eq!(number(&sample, "r", "opacity", 0.0), 0.4);
        assert!(!sample.find("r").unwrap().props.contains_key("y"));
    }
}

#[test]
fn local_motion_clock_obeys_source_trim_rate_and_fitted_group_bounds() {
    let root = tree(
        r#"<group id="g" start={1} playbackRate={2}>
      <animation type="slideLeft" duration={1}/>
      <rect id="r" start={2} sourceIn={1} sourceOut={5} playbackRate={2}>
        <animation type="fade" duration={1}/>
        <keyframeTrack property="x"><keyframe value={0}/><keyframe time={5} value={150}/></keyframeTrack>
      </rect>
    </group>"#,
    );
    let timeline = Timeline::resolve(&root, &BTreeMap::new()).unwrap();
    assert_eq!(timeline.get("r").unwrap().start, 2.0);
    assert_eq!(timeline.get("r").unwrap().playback_rate, 4.0);
    let sample = at(&root, 2.0, 30.0);
    assert_eq!(number(&sample, "g", "offsetX", 0.0), 100.0);
    assert_eq!(number(&sample, "r", "opacity", 1.0), 0.0);
    assert_eq!(number(&sample, "r", "x", 0.0), 30.0);
    assert_eq!(number(&at(&root, 2.5, 30.0), "r", "x", 0.0), 90.0);
}

#[test]
fn nested_style_tracks_share_holder_clock_color_channels_and_uniform_scale_rules() {
    let root = tree(
        r#"<rect id="r" start={1} end={4} scale={2} fill="red">
      <keyframeTrack property="scaleY"><keyframe value={9}/></keyframeTrack>
      <keyframeTrack property="color"><keyframe value="red"/><keyframe time={1} value="blue"/></keyframeTrack>
      <solidPaint id="p" color="red"><keyframeTrack property="color"><keyframe value="red"/><keyframe time={1} value="blue"/></keyframeTrack></solidPaint>
      <stroke id="stroke"><keyframeTrack property="width"><keyframe value={1}/><keyframe time={1} value={5}/></keyframeTrack></stroke>
    </rect><rect id="scale" end={5}><keyframeTrack property="scale"><keyframe value={3}/></keyframeTrack></rect>"#,
    );
    let sample = at(&root, 1.5, 30.0);
    assert_eq!(sample.find("r").unwrap().string("fill"), Some("#800080"));
    assert_eq!(sample.find("p").unwrap().string("color"), Some("#800080"));
    assert_eq!(number(&sample, "stroke", "width", 0.0), 3.0);
    assert_eq!(number(&sample, "r", "scaleY", 0.0), 2.0);
    assert_eq!(number(&sample, "scale", "scaleX", 0.0), 3.0);
    assert_eq!(number(&sample, "scale", "scaleY", 0.0), 3.0);
}

#[test]
fn scene_style_tracks_inherit_global_scene_clock_and_root_visibility() {
    let root = SourceDocument::parse(
        PathBuf::from("scene-style.tsx"),
        r#"export default <stage>
      <scene id="s" active start={1} end={5} playbackRate={2}>
        <solidPaint id="p" color="red"><keyframeTrack property="color">
          <keyframe value="red"/><keyframe time={1} value="blue"/>
        </keyframeTrack></solidPaint>
      </scene>
    </stage>"#
            .into(),
    )
    .unwrap()
    .root;
    assert_eq!(
        at(&root, 0.5, 30.0).find("p").unwrap().string("color"),
        Some("#800080")
    );
}

#[test]
fn gain_adds_decibels_and_keyframes_can_override_silence() {
    let root = tree(
        r#"<rect id="r" end={3} volume={-6}><animation type="gain"/><animation type="gain"/></rect>"#,
    );
    assert!(at(&root, 0.0, 30.0).find("r").unwrap().flag("muted"));
    let eased = ad3_editor::easing::cubic_bezier([0.4, 0.095, 0.546, 0.875], 14.0 / 29.0);
    assert!(
        (number(&at(&root, 14.0 / 30.0, 30.0), "r", "volume", 0.0) - (-6.0 + 40.0 * eased.log10()))
            .abs()
            < 1e-9
    );
    let override_root = tree(
        r#"<rect id="r" end={3}><animation type="gain"/><keyframeTrack property="volume"><keyframe value={-3}/></keyframeTrack></rect>"#,
    );
    let sample = at(&override_root, 0.0, 30.0);
    assert!(!sample.find("r").unwrap().flag("muted"));
    assert_eq!(number(&sample, "r", "volume", 0.0), -3.0);
}

#[test]
fn output_fps_controls_quantization_natural_duration_and_default_480_frames() {
    let root = SourceDocument::parse(
        PathBuf::from("fps.tsx"),
        r#"export default <stage><scene id="s" active workarea={[0,2/30]}>
      <rect id="r"/><video id="v" src="v.mp4"/>
    </scene></stage>"#
            .into(),
    )
    .unwrap()
    .root;
    let durations = BTreeMap::from([("v".into(), 1.02)]);
    let fps24 = Timeline::resolve_at_rate(&root, &durations, 24.0).unwrap();
    let fps30 = Timeline::resolve(&root, &durations).unwrap();
    assert_eq!(fps24.workareas["s"], [0.0, 2.0 / 24.0]);
    assert_eq!(fps30.workareas["s"], [0.0, 2.0 / 30.0]);
    assert_eq!(fps24.get("r").unwrap().end, 20.0);
    assert_eq!(fps30.get("r").unwrap().end, 16.0);
    assert_eq!(fps24.get("v").unwrap().end, 1.0);
    assert_eq!(fps30.get("v").unwrap().end, 31.0 / 30.0);
    let root = tree(r#"<rect id="r" end={3}><animation type="fade" duration={1}/></rect>"#);
    assert_eq!(
        number(&at(&root, 23.0 / 24.0, 24.0), "r", "opacity", 0.0),
        1.0
    );
    assert_eq!(
        number(&at(&root, 29.0 / 30.0, 30.0), "r", "opacity", 0.0),
        1.0
    );
    for fps in [0.0, -1.0, 241.0, f64::NAN] {
        assert!(Timeline::resolve_at_rate(&root, &BTreeMap::new(), fps).is_err());
    }
}

#[test]
fn motion_rejects_unsupported_targeting_and_malformed_tracks() {
    for body in [
        r#"<sequence><animation/></sequence>"#,
        r#"<rect><keyframeTrack property="invalid"/></rect>"#,
        r#"<rect><keyframe/></rect>"#,
        r#"<rect><keyframeTrack property="x"><rect/></keyframeTrack></rect>"#,
        r#"<rect><animation unsupported={1}/></rect>"#,
    ] {
        let root = tree(body);
        let timeline = Timeline::resolve(&root, &BTreeMap::new()).unwrap();
        assert!(
            Motion::default()
                .sample(&root, &timeline, 0.0, 30.0)
                .is_err(),
            "{body}"
        );
    }
}

#[test]
fn render_alpha_inherits_per_draw_and_retains_parent_for_invalid_canvas_assignments() {
    use ad3_editor::{
        media::{MediaLibrary, Tools},
        render::RenderPlan,
    };
    let root = tree(
        r#"<group opacity={0.4}>
      <rect width={10} height={10} fill="red" opacity={-1}/>
      <rect width={10} height={10} fill="green" opacity={2}/>
      <rect width={10} height={10} fill="blue" opacity={4}/>
    </group>"#,
    );
    let library = MediaLibrary::load(&root, std::path::Path::new("."), &Tools::default()).unwrap();
    let timeline = Timeline::resolve(&root, &library.durations).unwrap();
    let plan = RenderPlan::build(&root, &timeline, &library, "s", 30.0, None).unwrap();
    assert_eq!(plan.graph.matches("colorchannelmixer=aa=0.4,").count(), 2);
    assert_eq!(plan.graph.matches("colorchannelmixer=aa=0.8,").count(), 1);
}

#[test]
fn native_easing_matches_original_editor_reference_samples() {
    #[derive(Deserialize)]
    struct Curve {
        descriptor: String,
        samples: Vec<[f64; 2]>,
    }
    let curves: Vec<Curve> = serde_json::from_str(include_str!("fixtures/easing.json")).unwrap();
    for curve in curves {
        let easing = Easing::parse(&curve.descriptor).unwrap();
        for [progress, expected] in curve.samples {
            let actual = easing.sample(progress);
            assert!(
                (actual - expected).abs() < 1e-10,
                "{} at {progress}: {actual} != {expected}",
                curve.descriptor
            );
        }
    }
    assert_eq!(Easing::parse("unknown").unwrap().sample(0.3), 0.3);
    // These do NOT match the original resolveEasing regular expressions and
    // therefore use its linear fallback, despite a familiar function name.
    for unknown in ["steps(foo)", "spring(foo,300)", "cubicBezier(foo,0,1,1)"] {
        assert_eq!(Easing::parse(unknown).unwrap().sample(0.3), 0.3);
    }
    // These match that grammar, but Number conversion/the curve produces NaN.
    // Native composition data keeps its explicit finite-values restriction.
    for invalid in ["spring(1--,300)", "cubicBezier(1..2,0,1,1)"] {
        assert!(Easing::parse(invalid).is_err());
    }
    assert!(Easing::parse("steps(0)").is_err());
    let clean = Easing::parse("easeInOut").unwrap();
    let spaced = Easing::parse("\u{feff} cubicBezier ( .42, 0, .58, 1 ) ").unwrap();
    assert_eq!(clean.sample(0.3), spaced.sample(0.3));
}

#[test]
fn native_text_motion_matches_utf16_and_word_boundary_reference() {
    #[derive(Deserialize)]
    struct Case {
        text: String,
        ratio: f64,
        chars: String,
        words: String,
        scramble: String,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/text-motion.json")).unwrap();
    for case in cases {
        assert_eq!(
            text_motion::reveal_chars(&case.text, case.ratio),
            case.chars,
            "chars {:?} at {}",
            case.text,
            case.ratio
        );
        assert_eq!(
            text_motion::reveal_words(&case.text, case.ratio),
            case.words,
            "words {:?} at {}",
            case.text,
            case.ratio
        );
        assert_eq!(
            text_motion::scramble_chars(&case.text, case.ratio),
            case.scramble,
            "scramble {:?} at {}",
            case.text,
            case.ratio
        );
    }
}

#[test]
fn native_colors_match_named_css_and_original_hex_fallbacks() {
    #[derive(Deserialize)]
    struct Case {
        text: String,
        color: Option<u32>,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/colors.json")).unwrap();
    for case in cases {
        assert_eq!(color::parse(&case.text), case.color, "{}", case.text);
    }
    assert_eq!(color::number(-1.0), Some(0xff_ffff));
    assert_eq!(color::number(0x12ab_cdef_u32 as f64), Some(0xab_cdef));
    assert_eq!(color::number(f64::INFINITY), None);
    assert_eq!(color::lerp(0xff0000, 0x0000ff, 0.5), 0x800080);
}

#[test]
fn color_interpolation_matches_javascript_bitwise_wrapping_at_extreme_overshoot() {
    #[derive(Deserialize)]
    struct Case {
        from: u32,
        to: u32,
        progress: f64,
        color: u32,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/color-lerp.json")).unwrap();
    for case in cases {
        assert_eq!(
            color::lerp(case.from, case.to, case.progress),
            case.color,
            "{:06x} -> {:06x} at {}",
            case.from,
            case.to,
            case.progress
        );
    }
}

#[test]
fn renderer_uses_original_color_parsing_and_independent_opacity() {
    use ad3_editor::{
        media::MediaLibrary, render::RenderPlan, source::SourceDocument, timing::Timeline,
    };
    let document = SourceDocument::parse("color.tsx".into(), "export default <stage><scene id='s' active width={32} height={32} fill={255} workarea={[0,1]}><rect id='r' width={16} height={16} fill='rgba(255,0,0,0.1)' opacity={0.5}/><text id='t' color='hsl(120 100% 50%)'>A</text></scene></stage>".into()).unwrap();
    let library = MediaLibrary {
        sources: Default::default(),
        probes: Default::default(),
        durations: Default::default(),
    };
    let timeline = Timeline::resolve(&document.root, &library.durations).unwrap();
    let plan = RenderPlan::build(&document.root, &timeline, &library, "s", 30.0, None).unwrap();
    assert!(plan.graph.contains("color=c=0x0000FF"));
    assert!(plan.graph.contains("color=c=0xFF0000"));
    assert!(plan.graph.contains("colorchannelmixer=aa=0.5"));
    assert!(plan.graph.contains("fontcolor=0x00FF00"));
}

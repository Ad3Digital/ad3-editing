use ad3_editor::{color, easing::Easing, text_motion};
use serde::Deserialize;

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

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{media::Tools, project::ProjectRenderer, source::SourceDocument};
use std::path::Path;
fn tools() -> Tools {
    Tools {
        ffmpeg: "no-effect-codec".into(),
        ffprobe: "no-effect-probe".into(),
    }
}
fn renderer(body: &str) -> ProjectRenderer {
    let doc=SourceDocument::parse("effects.tsx".into(),format!("export default <stage><scene id=\"s\" active width={{128}} height={{128}} end={{4}}>{body}</scene></stage>")).unwrap();
    ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap()
}
fn frame(r: &mut ProjectRenderer, time: f64) -> Vec<u8> {
    let plan = r.frame_plan("s", time, 30.0, false).unwrap();
    assert!(plan.is_native());
    plan.frame(&tools(), Path::new(".")).unwrap()
}
fn premult(p: &[u8]) -> [f64; 4] {
    [
        (p[0] as f64 * p[3] as f64 / 255.0).round(),
        (p[1] as f64 * p[3] as f64 / 255.0).round(),
        (p[2] as f64 * p[3] as f64 / 255.0).round(),
        p[3] as f64,
    ]
}
#[test]
fn effects_and_shadows_match_original_runtime_canvas() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/effects.json")).unwrap();
    let mut failures = Vec::new();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 107);
    for case in fixture["cases"].as_array().unwrap() {
        let mut r = renderer(case["source"].as_str().unwrap());
        let mut plan = r.frame_plan("s", 0.0, 30.0, false).unwrap();
        let width = (128.0 * case["scale"].as_f64().unwrap_or(1.0)) as usize;
        plan.resize_output(width as u32).unwrap();
        let bytes = plan.frame(&tools(), Path::new(".")).unwrap();
        let mut worst = 0.0f64;
        let mut where_at = (0, 0);
        let mut pair = ([0.0; 4], [0.0; 4]);
        assert!(case["samples"].as_array().unwrap().len() >= 512);
        for sample in case["samples"].as_array().unwrap() {
            let x = sample[0].as_u64().unwrap() as usize;
            let y = sample[1].as_u64().unwrap() as usize;
            let actual = premult(&bytes[(y * width + x) * 4..][..4]);
            let expected = premult(&std::array::from_fn::<_, 4, _>(|i| {
                sample[2 + i].as_u64().unwrap() as u8
            }));
            for i in 0..4 {
                let d = if sample.as_array().unwrap().len() > 6 {
                    // Different edge AA is confined to the original one-pixel
                    // neighborhood, as in the existing native mask oracle.
                    (sample[6 + i].as_f64().unwrap() - actual[i])
                        .max(actual[i] - sample[10 + i].as_f64().unwrap())
                        .max(0.0)
                } else {
                    (actual[i] - expected[i]).abs()
                };
                let tolerance = if sample.as_array().unwrap().len() > 6 {
                    2.0
                } else {
                    case["tolerance"].as_f64().unwrap()
                };
                if d > tolerance && d > worst {
                    worst = d;
                    where_at = (x, y);
                    pair = (actual, expected);
                }
            }
        }
        if worst > 0.0 {
            failures.push(format!(
                "{} error {worst:.2} at {where_at:?}: {pair:?}",
                case["name"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn shape_shadow_tracks_animate_but_text_shadow_traits_remain_authored() {
    let tracks = r#"<keyframeTrack property="color"><keyframe value="red"/><keyframe time={1} value="blue"/></keyframeTrack><keyframeTrack property="blur"><keyframe value={0}/><keyframe time={1} value={10}/></keyframeTrack><keyframeTrack property="offsetX"><keyframe value={10}/><keyframe time={1} value={30}/></keyframeTrack><keyframeTrack property="opacity"><keyframe value={1}/><keyframe time={1} value={0.5}/></keyframeTrack>"#;
    for (tag, props, content) in [
        ("rect", "width={48} height={48}", ""),
        ("text", "width={110} height={80} fontSize={36}", "AD3"),
    ] {
        let mut r = renderer(&format!(
            "<{tag} {props}><shadow color=\"#ff0000\" offsetX={{10}}>{tracks}</shadow>{content}</{tag}>"
        ));
        let before = frame(&mut r, 0.0);
        let after = frame(&mut r, 1.0);
        if tag == "text" {
            assert_eq!(before, after);
        } else {
            assert_ne!(before, after);
        }
    }
}
#[test]
fn effect_values_and_blur_presets_are_sampled_on_the_holder_clock() {
    let mut r = renderer(
        r#"<rect x={24} y={24} width={56} height={56} fill="crimson"><effect type="invert"><keyframeTrack property="value"><keyframe value={0}/><keyframe time={1} value={1}/></keyframeTrack></effect></rect>"#,
    );
    assert_ne!(frame(&mut r, 0.0), frame(&mut r, 1.0));
    let mut r = renderer(
        r#"<rect x={24} y={24} width={56} height={56} fill="red"><animation type="blur" duration={1}/></rect>"#,
    );
    assert_ne!(frame(&mut r, 0.0), frame(&mut r, 1.0));
}
#[test]
fn hidden_range_shadows_replace_the_parent_list_even_when_all_are_hidden() {
    let a = r#"<text width={120} height={80} fontSize={36}>AD3<shadow color="red" offsetX={10}/><textRange start={0} end={3} hidden><shadow color="blue" hidden/></textRange></text>"#;
    let b = r#"<text width={120} height={80} fontSize={36}>AD3<shadow color="red" hidden/></text>"#;
    assert_eq!(frame(&mut renderer(a), 0.0), frame(&mut renderer(b), 0.0));
}
#[test]
fn text_shadow_uses_widest_visible_stroke_and_parent_stroke_color() {
    let source = |extra: &str| {
        format!(
            r#"<rect width={{128}} height={{128}}><stroke color="lime" width={{2}}/><text x={{15}} y={{20}} width={{110}} height={{80}} fontSize={{48}}>AD<shadow color="red" offsetX={{30}} opacity={{0.6}}/><stroke color="blue" width={{8}} opacity={{0}}/>{extra}</text></rect>"#
        )
    };
    let base = frame(&mut renderer(&source("")), 0.0);
    assert_eq!(
        base,
        frame(
            &mut renderer(&source(r#"<stroke width={20} hidden/>"#)),
            0.0
        )
    );
    assert_ne!(
        base,
        frame(
            &mut renderer(&source(r#"<stroke width={20} opacity={0}/>"#)),
            0.0
        )
    );
    // The foreground stroke left by the shadows pass uses inherited lime;
    // opacity-zero actual strokes cannot cover it with blue.
    assert!(
        base.as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[1] > 100 && p[3] > 30 && p[3] < 250)
    );
}
#[test]
fn negative_shadow_blur_and_invalid_alpha_retain_prior_assignments() {
    let a = r#"<rect x={24} y={24} width={48} height={48}><shadow color="red" blur={8} opacity={0.4}/><shadow color="blue" blur={-2} opacity={8}/></rect>"#;
    let b = a
        .replace("blur={-2}", "blur={8}")
        .replace("opacity={8}", "opacity={0.4}");
    assert_eq!(frame(&mut renderer(a), 0.0), frame(&mut renderer(&b), 0.0));
}
#[test]
fn effect_limits_fail_explicitly_and_pixel_cancellation_is_recoverable() {
    let mut r = renderer(r#"<rect blur={257} fill="red"/>"#);
    assert!(
        r.frame_plan("s", 0.0, 30.0, false)
            .err()
            .unwrap()
            .contains("256")
    );
    let mut r = renderer(&format!(
        "<rect>{}</rect>",
        r#"<effect type="invert" value={0.5}/>"#.repeat(65)
    ));
    assert!(
        r.frame_plan("s", 0.0, 30.0, false)
            .err()
            .unwrap()
            .contains("64")
    );
    let mut r = renderer(r#"<rect x={20} y={20} width={50} height={50} blur={12} fill="red"/>"#);
    let p = r.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert!(
        p.frame_cancellable(
            &tools(),
            Path::new("."),
            Some(&std::sync::atomic::AtomicBool::new(true))
        )
        .unwrap_err()
        .contains("cancelled")
    );
    assert!(p.frame(&tools(), Path::new(".")).is_ok());
}

#[test]
fn filtered_media_is_clipped_after_blur_and_cover_can_sample_beyond_its_box() {
    let folder = std::env::temp_dir().join(format!(
        "ad3-effect-media-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&folder).unwrap();
    image::RgbaImage::from_pixel(16, 8, image::Rgba([255, 0, 0, 255]))
        .save(folder.join("red.png"))
        .unwrap();
    for (fit, x, width) in [("fill", 0, 64), ("cover", -32, 128)] {
        let doc=SourceDocument::parse(folder.join("index.tsx"),format!(r#"export default <stage><scene id="s" active width={{128}} height={{128}}><image src="./red.png" x={{32}} y={{32}} width={{64}} height={{64}} cornerRadius={{12}} objectFit="{fit}" blur={{4}} opacity={{0.6}}><effect type="invert" value={{0.7}}/></image></scene></stage>"#)).unwrap();
        let mut image = ProjectRenderer::new(&doc, &folder, tools(), true).unwrap();
        let actual = frame(&mut image, 0.0);
        let expected = frame(
            &mut renderer(&format!(
                r#"<rect x={{{}}} y={{32}} width={{{width}}} height={{64}} fill="red" blur={{4}} opacity={{0.6}}><effect type="invert" value={{0.7}}/><rect mask x={{{}}} width={{64}} height={{64}} cornerRadius={{12}}/></rect>"#,
                32 + x,
                -x
            )),
            0.0,
        );
        for (a, b) in actual
            .as_chunks::<4>()
            .0
            .iter()
            .zip(expected.as_chunks::<4>().0)
        {
            for (a, b) in premult(a).iter().zip(premult(b)) {
                assert!((a - b).abs() <= 2.0, "{fit}: {a} != {b}");
            }
        }
        for y in 0..128 {
            assert_eq!(&actual[(y * 128 + 31) * 4..][..4], &[0; 4]);
        }
    }
    std::fs::remove_dir_all(folder).unwrap();
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{media::Tools, project::ProjectRenderer, source::SourceDocument};
use std::path::Path;
fn tools() -> Tools {
    Tools {
        ffmpeg: "no-paint-codec".into(),
        ffprobe: "no-paint-probe".into(),
    }
}
fn renderer(body: &str, size: [u32; 2]) -> ProjectRenderer {
    let doc=SourceDocument::parse("paints.tsx".into(),format!("export default <stage><scene id=\"s\" active width={{{}}} height={{{}}} workarea={{[0,2]}}>{body}</scene></stage>",size[0],size[1])).unwrap();
    ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap()
}
fn frame(renderer: &mut ProjectRenderer, time: f64) -> Vec<u8> {
    let plan = renderer.frame_plan("s", time, 30.0, false).unwrap();
    assert!(plan.is_native());
    assert!(plan.graph.is_empty());
    plan.frame(&tools(), Path::new(".")).unwrap()
}
fn pixel(bytes: &[u8], width: usize, x: usize, y: usize) -> &[u8] {
    &bytes[(y * width + x) * 4..(y * width + x + 1) * 4]
}
#[test]
fn native_paints_match_canvas_pixel_samples_for_gradients_and_every_blend_mode() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/paint-pixels.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 63);
    for case in cases {
        let bytes = frame(
            &mut renderer(case["source"].as_str().unwrap(), [16, 8]),
            0.0,
        );
        for sample in case["samples"].as_array().unwrap() {
            let x = sample["x"].as_u64().unwrap() as usize;
            let y = sample["y"].as_u64().unwrap() as usize;
            let actual = pixel(&bytes, 16, x, y);
            let expected = sample["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>();
            assert!((actual[3] as i32 - expected[3] as i32).abs() <= 1);
            // Compare stored color energy: unpremultiplying a one-byte color
            // difference at low alpha can amplify it by dozens of RGB levels.
            // Canvas dithering and tiny-skia quantization are not identical.
            for channel in 0..3 {
                let a = actual[channel] as f64 * actual[3] as f64 / 255.0;
                let b = expected[channel] as f64 * expected[3] as f64 / 255.0;
                assert!(
                    (a - b).abs() <= 2.0,
                    "{} at {x},{y}: {actual:?} != {}",
                    case["name"],
                    sample["rgba"]
                );
            }
        }
    }
}
#[test]
fn paint_order_hidden_children_and_inherited_blend_are_per_draw() {
    let body = r##"<rect width={40} height={40} fill="#808080"/><group blendMode="multiply"><rect width={40} height={40} fill="red" blendMode="sourceOver"><solidPaint color="blue" hidden/><solidPaint color="white" opacity={0.5}/></rect></group>"##;
    let bytes = frame(&mut renderer(body, [40, 40]), 0.0);
    // The white paint retains multiply, so it leaves the red backdrop unchanged.
    assert_eq!(pixel(&bytes, 40, 20, 20), [128, 0, 0, 255]);
}
#[test]
fn shape_strokes_follow_fills_and_invalid_width_retains_parent_canvas_line_state() {
    let body = r#"<rect x={10} y={10} width={60} height={60} fill="red"><stroke color="white" width={8}/><rect x={15} y={15} width={20} height={20}><stroke color="blue" width={0}/></rect></rect>"#;
    let bytes = frame(&mut renderer(body, [80, 80]), 0.0);
    assert_eq!(pixel(&bytes, 80, 10, 30), [255, 255, 255, 255]);
    assert_eq!(pixel(&bytes, 80, 22, 30), [0, 0, 255, 255]);
    assert_eq!(pixel(&bytes, 80, 35, 35), [255, 0, 0, 255]);
}
#[test]
fn text_range_paint_lists_replace_parent_lists_and_text_strokes_stay_below_fills() {
    let styled = r#"<text width={220} height={70} fontSize={48}>A B<solidPaint color="red"/><textRange start={2} hidden opacity={0}><solidPaint color="blue"/></textRange></text>"#;
    let intrinsic = r#"<text width={220} height={70} fontSize={48} color="red">A B<textRange start={2} color="blue"/></text>"#;
    assert_eq!(
        frame(&mut renderer(styled, [240, 80]), 0.0),
        frame(&mut renderer(intrinsic, [240, 80]), 0.0)
    );
    let base = r#"<text width={220} height={70} fontSize={48} color="red">III</text>"#;
    let stroked = r#"<text width={220} height={70} fontSize={48} color="red">III<stroke color="white" width={8}/></text>"#;
    let plain = frame(&mut renderer(base, [240, 80]), 0.0);
    let outlined = frame(&mut renderer(stroked, [240, 80]), 0.0);
    assert!(
        outlined
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p == &[255, 255, 255, 255])
    );
    for (before, after) in plain
        .as_chunks::<4>()
        .0
        .iter()
        .zip(outlined.as_chunks::<4>().0)
    {
        if before == &[255, 0, 0, 255] {
            assert_eq!(after, before);
        }
    }
}
#[test]
fn text_gradient_coordinates_belong_to_the_text_box_not_each_word() {
    let body = r#"<text width={240} height={70} fontSize={48}>MMMM MMMM<linearGradientPaint><colorStop color="red" offset={0}/><colorStop color="blue" offset={1}/></linearGradientPaint></text>"#;
    let bytes = frame(&mut renderer(body, [260, 80]), 0.0);
    let mut samples = 0;
    for (i, p) in bytes.as_chunks::<4>().0.iter().enumerate() {
        if p[3] != 255 {
            continue;
        }
        let x = i % 260;
        let red = ((1.0 - (x as f64 + 0.5) / 240.0).clamp(0.0, 1.0) * 255.0).round() as i32;
        assert!(
            (p[0] as i32 - red).abs() <= 2,
            "word gradient restarted at {x}"
        );
        samples += 1;
    }
    assert!(samples > 1000);
}
#[test]
fn shape_styles_animate_while_text_solid_ink_keeps_the_original_authored_trait_behavior() {
    let track = r#"<keyframeTrack property="color"><keyframe value="red"/><keyframe time={1} value="blue"/></keyframeTrack>"#;
    let shape = format!(
        "<rect width={{40}} height={{40}}><solidPaint color=\"red\">{track}</solidPaint></rect>"
    );
    let mut shape = renderer(&shape, [80, 80]);
    assert_eq!(pixel(&frame(&mut shape, 1.0), 80, 20, 20), [0, 0, 255, 255]);
    let text = format!(
        "<text width={{160}} height={{60}} fontSize={{48}}>ABC<solidPaint color=\"red\">{track}</solidPaint></text>"
    );
    let mut text = renderer(&text, [180, 80]);
    assert_eq!(frame(&mut text, 0.0), frame(&mut text, 1.0));
}
#[test]
fn invalid_style_hierarchies_fail_and_never_become_geometry_layers() {
    for body in [
        "<group><solidPaint color=\"red\"/></group>",
        "<rect><colorStop color=\"red\"/></rect>",
        "<rect><solidPaint><rect/></solidPaint></rect>",
    ] {
        let mut renderer = renderer(body, [40, 40]);
        assert!(renderer.frame_plan("s", 0.0, 30.0, false).is_err());
    }
}

#[test]
fn color_aliases_keep_trait_membership_and_tracks_override_the_resolved_alias() {
    let body = r#"<rect width={20} height={20} fill="red" color="blue"/><rect x={20} width={20} height={20} color="red" fill={null}/><rect y={20} width={20} height={20} color="blue"><keyframeTrack property="color"><keyframe value="lime"/></keyframeTrack></rect><rect x={20} y={20} width={20} height={20}><solidPaint color="red" fill="blue"><keyframeTrack property="color"><keyframe value="lime"/></keyframeTrack></solidPaint></rect>"#;
    let bytes = frame(&mut renderer(body, [40, 40]), 0.0);
    assert_eq!(pixel(&bytes, 40, 10, 10), [0, 0, 255, 255]);
    assert_eq!(pixel(&bytes, 40, 30, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 40, 10, 30), [0, 255, 0, 255]);
    assert_eq!(pixel(&bytes, 40, 30, 30), [0, 255, 0, 255]);
}
#[test]
fn gradient_motion_keeps_position_membership_and_independent_paint_scale_axes() {
    let stops = r#"<colorStop color="red" offset={0}/><colorStop color="blue" offset={1} hidden/>"#;
    let source = |props: &str, tracks: &str| {
        format!(
            "<rect width={{80}} height={{40}}><linearGradientPaint {props}>{stops}{tracks}</linearGradientPaint></rect>"
        )
    };
    let tracks = r#"<keyframeTrack property="x"><keyframe value={0}/><keyframe time={1} value={1}/></keyframeTrack><keyframeTrack property="scale"><keyframe value={2}/><keyframe time={1} value={3}/></keyframeTrack>"#;
    let mut animated = renderer(&source("rotation={45} scale={2}", tracks), [80, 40]);
    let expected = frame(
        &mut renderer(&source("rotation={45} scaleX={3} scaleY={2}", ""), [80, 40]),
        0.0,
    );
    assert_eq!(frame(&mut animated, 1.0), expected);
    let positioned = frame(
        &mut renderer(
            &source("x={1} rotation={45} scaleX={3} scaleY={2}", ""),
            [80, 40],
        ),
        0.0,
    );
    assert_ne!(expected, positioned);
}
#[test]
fn invalid_text_fill_alpha_keeps_the_previous_fill_assignment() {
    let a = r#"<text width={160} height={60} fontSize={48}>ABC<solidPaint color="red" opacity={0.5}/><solidPaint color="blue" opacity={-1}/></text>"#;
    let b = a.replace("opacity={-1}", "opacity={0.5}");
    assert_eq!(
        frame(&mut renderer(a, [180, 80]), 0.0),
        frame(&mut renderer(&b, [180, 80]), 0.0)
    );
}
#[test]
fn tiny_positive_strokes_never_turn_into_device_hairlines() {
    let body =
        r#"<rect x={10} y={10} width={20} height={20}><stroke color="red" width={1e-100}/></rect>"#;
    assert!(
        frame(&mut renderer(body, [40, 40]), 0.0)
            .iter()
            .all(|v| *v == 0)
    );
}

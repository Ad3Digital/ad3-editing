// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    media::Tools, project::ProjectRenderer, source::SourceDocument, timing::Timeline,
};
use std::path::Path;
fn tools() -> Tools {
    Tools {
        ffmpeg: "no-mask-codec".into(),
        ffprobe: "no-mask-probe".into(),
    }
}
fn document(body: &str) -> SourceDocument {
    SourceDocument::parse("masks.tsx".into(),format!("export default <stage><scene id=\"s\" active width={{128}} height={{128}} end={{30}} workarea={{[0,30]}}>{body}</scene></stage>")).unwrap()
}
fn renderer(body: &str) -> ProjectRenderer {
    ProjectRenderer::new(&document(body), Path::new("."), tools(), true).unwrap()
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
fn masks_match_original_runtime_canvas_samples_and_adapter_defaults() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/masks.json")).unwrap();
    assert_eq!(
        fixture["defaults"],
        serde_json::json!({"width":100,"height":100})
    );
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 56);
    let mut exact_interiors = 0;
    for case in cases {
        let doc = document(case["source"].as_str().unwrap());
        let timeline = Timeline::resolve(&doc.root, &Default::default()).unwrap();
        let nodes = doc
            .root
            .find("s")
            .unwrap()
            .nodes()
            .into_iter()
            .filter(|n| n.is_timed())
            .collect::<Vec<_>>();
        let spans = case["spans"].as_array().unwrap();
        assert_eq!(nodes.len(), spans.len());
        for (node, expected) in nodes.iter().zip(spans) {
            let actual = timeline.get(&node.id).unwrap();
            assert_eq!(node.flag("mask"), expected["mask"].as_bool().unwrap());
            assert!(
                (actual.start * 30.0 - expected["start"].as_f64().unwrap()).abs() < 1e-9,
                "{}",
                case["name"]
            );
            assert!(
                (actual.end * 30.0 - expected["end"].as_f64().unwrap()).abs() < 1e-9,
                "{}",
                case["name"]
            );
        }
        let mut renderer = renderer(case["source"].as_str().unwrap());
        let mut plan = renderer
            .frame_plan("s", case["time"].as_f64().unwrap(), 30.0, false)
            .unwrap();
        assert!(plan.is_native());
        let width = (128.0 * case["scale"].as_f64().unwrap()) as usize;
        plan.resize_output(width as u32).unwrap();
        let bytes = plan.frame(&tools(), Path::new(".")).unwrap();
        if case["name"] == "rounded@0" {
            assert!(case["alphaValues"].as_array().unwrap().len() > 2);
            assert!(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0 && p[3] < 255)
            );
        }
        for sample in case["samples"].as_array().unwrap() {
            let x = sample["x"].as_u64().unwrap() as usize;
            let y = sample["y"].as_u64().unwrap() as usize;
            let actual = pixel(&bytes, width, x, y);
            let expected = sample["rgba"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>();
            if sample["nearbyMin"] == sample["nearbyMax"] {
                exact_interiors += 1;
            }
            for channel in 0..4 {
                let a = actual[channel] as f64
                    * if channel == 3 {
                        1.0
                    } else {
                        actual[3] as f64 / 255.0
                    };
                // Native/Canvas edge coverage is not byte-identical. Restrict
                // differences to the reference's one-pixel spatial boundary;
                // uniform interiors/exteriors must agree within two levels.
                let lo = sample["nearbyMin"][channel].as_f64().unwrap();
                let hi = sample["nearbyMax"][channel].as_f64().unwrap();
                assert!(
                    a >= lo - 2.0 && a <= hi + 2.0,
                    "{} at {x},{y}: {actual:?} != {expected:?}",
                    case["name"]
                );
            }
        }
    }
    assert!(exact_interiors > 3000);
}
#[test]
fn reactive_mask_toggle_keeps_the_backend_and_restores_normal_layer_drawing() {
    let source=r#"import {useTicker} from '@diffusionstudio/jsx';
      export default ()=>{const t=useTicker();return <stage><scene id="s" active width={128} height={128} workarea={[0,3]}>
        <rect width={128} height={128} fill="red">
          <rect id="m" mask={t.time()<1} x={20} width={30} height={70} fill="blue"/>
        </rect>
      </scene></stage>};"#.to_owned();
    let path = std::path::absolute("reactive-masks.tsx").unwrap();
    let evaluated = ad3_editor::evaluator::evaluate(&path, &source).unwrap();
    let doc = SourceDocument {
        project_root: path.parent().unwrap().into(),
        path,
        source,
        root: evaluated.root,
        module_sources: evaluated.sources,
    };
    let mut r = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    for (time, clipped) in [(0.0, true), (1.0, false), (0.0, true)] {
        let bytes = frame(&mut r, time);
        assert_eq!(
            pixel(&bytes, 128, 10, 10),
            if clipped {
                &[0, 0, 0, 0]
            } else {
                &[255, 0, 0, 255]
            }
        );
        assert_eq!(
            pixel(&bytes, 128, 30, 10),
            if clipped {
                &[255, 0, 0, 255]
            } else {
                &[0, 0, 255, 255]
            }
        );
    }
}
#[test]
fn masks_do_not_extend_fitted_containers_and_default_to_480_frames() {
    let doc=SourceDocument::parse("mask-time.tsx".into(),r#"export default <stage><scene id="s" active><group id="g"><rect id="visible" end={2}/><rect id="mask" mask start={5}/></group></scene></stage>"#.into()).unwrap();
    for fps in [24.0, 30.0, 60.0] {
        let timeline = Timeline::resolve_at_rate(&doc.root, &Default::default(), fps).unwrap();
        assert_eq!(timeline.get("g").unwrap().end, 2.0);
        assert_eq!(timeline.get("s").unwrap().end, 2.0);
        assert_eq!(timeline.get("mask").unwrap().start, 5.0);
        assert_eq!(timeline.get("mask").unwrap().end, 5.0 + 480.0 / fps);
    }
    let doc = document(r#"<rect end={30} fill="red" width={128} height={128}><rect mask/></rect>"#);
    let mut r = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    assert_eq!(pixel(&frame(&mut r, 0.0), 128, 99, 99), [255, 0, 0, 255]);
    assert_eq!(pixel(&frame(&mut r, 0.0), 128, 100, 100), [0, 0, 0, 0]);
    assert_eq!(pixel(&frame(&mut r, 16.0), 128, 100, 100), [255, 0, 0, 255]);
}
#[test]
fn mask_time_edges_trim_and_reverse_seeks_release_the_holder() {
    let mut r = renderer(
        r#"<rect fill="red" width={128} height={128}><rect mask x={20} width={30} height={80} start={1} end={3} sourceIn={2} playbackRate={2}/></rect>"#,
    );
    for (time, clipped) in [
        (0.0, false),
        (1.0, true),
        (2.5, true),
        (3.0, false),
        (1.0, true),
        (0.0, false),
    ] {
        let bytes = frame(&mut r, time);
        assert_eq!(
            pixel(&bytes, 128, 10, 10),
            if clipped {
                &[0, 0, 0, 0]
            } else {
                &[255, 0, 0, 255]
            }
        );
        assert_eq!(pixel(&bytes, 128, 30, 10), [255, 0, 0, 255]);
    }
}
#[test]
fn hidden_masks_clip_but_skip_motion_and_never_draw_their_subtree() {
    let track = r#"<keyframeTrack property="x"><keyframe value={0}/><keyframe time={1} value={50}/></keyframeTrack>"#;
    for hidden in [false, true] {
        let mut r = renderer(&format!(
            r#"<rect fill="red" width={{128}} height={{128}}><rect mask hidden={{{hidden}}} opacity={{0}} fill="blue" width={{30}} height={{50}}>{track}<stroke color="lime" width={{30}}/><rect width={{128}} height={{128}} fill="lime"/></rect></rect>"#
        ));
        let bytes = frame(&mut r, 1.0);
        assert_eq!(
            pixel(&bytes, 128, 10, 10),
            if hidden {
                &[255, 0, 0, 255]
            } else {
                &[0, 0, 0, 0]
            }
        );
        assert_eq!(
            pixel(&bytes, 128, 60, 10),
            if hidden {
                &[0, 0, 0, 0]
            } else {
                &[255, 0, 0, 255]
            }
        );
    }
}
#[test]
fn mask_rects_are_excluded_from_rotated_group_bounds_on_every_frame() {
    let body = r#"<group x={30} y={20} rotation={90}><rect width={20} height={40} fill="red"/>MASK</group>"#;
    let mut plain = renderer(&body.replace("MASK", ""));
    let mut masked = renderer(&body.replace(
        "MASK",
        r#"<rect mask x={-1000} y={-1000} width={2000} height={2000}/>"#,
    ));
    for time in [0.0, 0.5, 1.0, 0.0] {
        assert_eq!(frame(&mut plain, time), frame(&mut masked, time));
    }
}
#[test]
fn root_masks_clip_scene_fill_and_children_while_sibling_stacks_restore() {
    let mut r = renderer(
        r#"<rect mask x={20} y={20} width={80} height={80}/><rect fill="red" width={128} height={128}><rect mask width={40} height={40}/></rect><rect x={70} y={70} width={50} height={50} fill="blue"/>"#,
    );
    let bytes = frame(&mut r, 0.0);
    assert_eq!(pixel(&bytes, 128, 10, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 128, 30, 30), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 128, 60, 60), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 128, 90, 90), [0, 0, 255, 255]);
    assert_eq!(pixel(&bytes, 128, 110, 110), [0, 0, 0, 0]);
    let mut doc = document(r#"<rect mask x={20} y={20} width={80} height={80}/>"#);
    doc.root.children[0]
        .props
        .insert("fill".into(), "red".into());
    let mut r = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    let bytes = frame(&mut r, 0.0);
    assert_eq!(pixel(&bytes, 128, 10, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 128, 30, 30), [255, 0, 0, 255]);
}
#[test]
fn text_is_clipped_without_changing_measurement_or_glyph_placement() {
    let body = r#"<text color="red" width={120} height={70} fontSize={48}>MASKMASK</text>"#;
    let mut plain = renderer(body);
    let mut masked = renderer(&body.replace(
        "</text>",
        r#"<rect mask x={30} width={40} height={100}/></text>"#,
    ));
    let plain = frame(&mut plain, 0.0);
    let masked = frame(&mut masked, 0.0);
    let mut ink = 0;
    for y in 0..128 {
        for x in 0..128 {
            let actual = pixel(&masked, 128, x, y);
            if (30..70).contains(&x) {
                assert_eq!(actual, pixel(&plain, 128, x, y));
                if actual[3] > 0 {
                    ink += 1
                }
            } else {
                assert_eq!(actual, [0, 0, 0, 0]);
            }
        }
    }
    assert!(ink > 100);
}

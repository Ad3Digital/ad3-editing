// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{media::Tools, project::ProjectRenderer, source::SourceDocument};
use std::path::{Path, PathBuf};
fn document(body: &str) -> SourceDocument {
    SourceDocument::parse("native-text.tsx".into(),format!("export default <stage><scene id=\"s\" active width={{240}} height={{160}} workarea={{[0,2]}}>{body}</scene></stage>")).unwrap()
}
fn tools() -> Tools {
    Tools {
        ffmpeg: "no-text-codec".into(),
        ffprobe: "no-text-probe".into(),
    }
}
fn pixels(renderer: &mut ProjectRenderer, time: f64) -> Vec<u8> {
    let plan = renderer.frame_plan("s", time, 30.0, false).unwrap();
    assert!(plan.is_native());
    assert!(plan.graph.is_empty());
    plan.frame(&tools(), Path::new(".")).unwrap()
}
fn color_pixels(pixels: &[u8], rgb: [u8; 3]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3] == rgb && p[3] > 100)
        .count()
}

#[test]
fn native_text_wraps_aligns_and_composes_range_ink_without_ffmpeg() {
    let doc = document(
        r#"<text x={10} y={10} width={140} height={120} fontSize={32} color="red" textAlign="center" textBaseline="middle">Hello World<textRange start={6} color="blue" fontWeight="bold"/></text><text x={180} fontSize={20}>Unpainted</text>"#,
    );
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    let bytes = pixels(&mut renderer, 0.0);
    assert!(color_pixels(&bytes, [255, 0, 0]) > 100);
    assert!(color_pixels(&bytes, [0, 0, 255]) > 100);
    assert!(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 240 >= 180)
            .all(|(_, p)| p[3] == 0)
    );
    let red_y = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| p[0] > 200 && p[2] == 0 && p[3] > 100)
        .map(|(i, _)| i / 240)
        .collect::<Vec<_>>();
    let blue_y = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| p[2] > 200 && p[0] == 0 && p[3] > 100)
        .map(|(i, _)| i / 240)
        .collect::<Vec<_>>();
    assert!(red_y.iter().max().unwrap() < blue_y.iter().min().unwrap());
}

#[test]
fn auto_text_measurement_affects_next_transform_frame_and_fixed_boxes_are_stable() {
    let doc =
        document(r#"<text x={75} y={55} color="white" fontSize={32} rotation={30}>Hello</text>"#);
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    let first = pixels(&mut renderer, 0.0);
    let second = pixels(&mut renderer, 0.0);
    let third = pixels(&mut renderer, 0.0);
    assert_ne!(first, second);
    assert_eq!(second, third);
    let fixed = document(
        r#"<text x={75} y={55} width={100} height={50} color="white" fontSize={32} rotation={30}>Hello</text>"#,
    );
    let mut renderer = ProjectRenderer::new(&fixed, Path::new("."), tools(), true).unwrap();
    assert_eq!(pixels(&mut renderer, 0.0), pixels(&mut renderer, 0.0));
}

#[test]
fn animated_text_color_tracks_cannot_create_a_color_trait() {
    let doc = document(
        r#"<text fontSize={32}><keyframeTrack property="color"><keyframe time={0} value="red"/></keyframeTrack>Hidden ink</text><text x={10} y={60} fill="blue" fontSize={32}><keyframeTrack property="color"><keyframe time={0} value="red"/></keyframeTrack>Visible</text>"#,
    );
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    let bytes = pixels(&mut renderer, 0.0);
    assert!(bytes[..240 * 50 * 4].iter().all(|v| *v == 0));
    assert!(color_pixels(&bytes, [255, 0, 0]) > 100);
    assert_eq!(color_pixels(&bytes, [0, 0, 255]), 0);
}

#[test]
fn temporary_font_errors_recover_and_dynamic_text_keeps_native_backend() {
    let source = r#"import {useTicker} from '@diffusionstudio/jsx';export default ()=>{const t=useTicker();return <stage><scene id="s" active width={240} height={160} workarea={[0,3]}><rect width={10} height={10} fill="blue"/>{t.time()>0?<text x={20} fontSize={32} color="red">{t.time()<2?'❤\uFE0F':'Hello'}</text>:null}</scene></stage>}"#;
    let path = std::path::absolute(PathBuf::from("dynamic-native-text.tsx")).unwrap();
    let evaluated = ad3_editor::evaluator::evaluate(&path, source).unwrap();
    let doc = SourceDocument {
        project_root: path.parent().unwrap().into(),
        path,
        source: source.into(),
        root: evaluated.root,
        module_sources: evaluated.sources,
    };
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    pixels(&mut renderer, 0.0);
    let error = renderer.frame_plan("s", 1.0, 30.0, false).err().unwrap();
    assert!(error.contains("not ported"));
    assert!(!error.contains("reopening"));
    assert!(color_pixels(&pixels(&mut renderer, 2.0), [255, 0, 0]) > 100);
}

#[test]
fn range_geometry_visibility_and_alpha_do_not_override_intrinsic_text_ink() {
    // Cache.textRanges does not filter Hidden; getIntrinsicColor ignores the
    // range's opacity, and shapeTokens reads layout from the text entity only.
    let simple = document(r#"<text fontSize={32}>Hello<textRange start={0} color="blue"/></text>"#);
    let extras = document(
        r#"<text fontSize={32}>Hello<textRange start={0} color="blue" hidden opacity={0} width={200} height={100} x={100} leading={5} textAlign="right"/></text>"#,
    );
    let mut first = ProjectRenderer::new(&simple, Path::new("."), tools(), true).unwrap();
    let mut second = ProjectRenderer::new(&extras, Path::new("."), tools(), true).unwrap();
    assert_eq!(pixels(&mut first, 0.0), pixels(&mut second, 0.0));
}

#[test]
fn hidden_unsupported_text_does_not_load_fonts_or_block_visible_layers() {
    let doc = document(
        "<rect width={20} height={20} fill=\"red\"/><text hidden x={2000} fontSize={32} color=\"white\">❤\u{fe0f}</text>",
    );
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    assert_eq!(color_pixels(&pixels(&mut renderer, 0.0), [255, 0, 0]), 400);
}

#[test]
fn offscreen_text_inherits_its_visible_scene_cull_state_like_the_original() {
    // cullEntity tests only stage children against the viewport; a text node
    // inherits its scene's cull state. Clipping does not skip its text layout.
    let doc = document("<text x={2000} fontSize={32} color=\"white\">❤\u{fe0f}</text>");
    let mut renderer = ProjectRenderer::new(&doc, Path::new("."), tools(), true).unwrap();
    let error = renderer.frame_plan("s", 0.0, 30.0, false).err().unwrap();
    assert!(error.contains("Variation-selector"));
    assert!(!error.contains("reopening"));
}

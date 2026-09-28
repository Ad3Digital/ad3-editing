// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    compositor::Compositor, document::Node, media::Tools, project::ProjectRenderer,
    source::SourceDocument, timing::Timeline,
};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

fn document(scene_props: &str, body: &str) -> SourceDocument {
    SourceDocument::parse(PathBuf::from("native-vector.tsx"), format!(
        "export default <stage background=\"blue\"><scene id=\"s\" active width={{64}} height={{48}} workarea={{[0,2]}} {scene_props}>{body}</scene></stage>"
    )).unwrap()
}
fn native(
    compositor: &mut Compositor,
    root: &Node,
    time: f64,
) -> ad3_editor::compositor::NativeFrame {
    let timeline = Timeline::resolve(root, &BTreeMap::new()).unwrap();
    compositor
        .frame(root.find("s").unwrap(), &timeline, time, 30.0, false, None)
        .unwrap()
}
fn pixel(bytes: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    bytes[(y * width + x) * 4..(y * width + x) * 4 + 4]
        .try_into()
        .unwrap()
}
fn unavailable_tools() -> Tools {
    Tools {
        ffmpeg: PathBuf::from("missing-native-vector-ffmpeg"),
        ffprobe: PathBuf::from("missing-native-vector-ffprobe"),
    }
}
fn dynamic_document(name: &str, source: String) -> SourceDocument {
    let path = std::path::absolute(name).unwrap();
    let evaluated = ad3_editor::evaluator::evaluate(&path, &source).unwrap();
    SourceDocument {
        project_root: path.parent().unwrap().into(),
        path,
        source,
        root: evaluated.root,
        module_sources: evaluated.sources,
    }
}

#[test]
fn vectors_render_without_codec_processes_and_normalize_the_scene_transform() {
    let doc = document(
        "x={999} y={-99} scale={3} rotation={34}",
        r#"
      <rect x={16} y={12} offsetX={4} width={16} height={8} rotation={90} fill="red"/>
      <rect x={44} y={8} width={4} height={4} scale={2} scaleX={9} scaleY={0} fill="lime"/>
      <rect x={0} y={0} width={4} height={4} fill="white" hidden/>
    "#,
    );
    let tools = unavailable_tools();
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), tools.clone(), true).unwrap();
    let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert!(plan.is_native());
    assert!(plan.graph.is_empty());
    assert!(plan.inputs.is_empty());
    let bytes = plan.frame(&tools, std::path::Path::new(".")).unwrap();
    assert_eq!(pixel(&bytes, 64, 28, 9), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 21, 15), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 43, 7), [0, 255, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 41, 7), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 1, 1), [0, 0, 0, 0]);
    let cancelled = AtomicBool::new(true);
    assert!(
        plan.frame_cancellable(&tools, std::path::Path::new("."), Some(&cancelled))
            .unwrap_err()
            .contains("cancelled")
    );
}

#[test]
fn scene_clip_rounding_and_output_scale_share_one_draw_list() {
    let doc = document(
        "cornerRadius={16}",
        r#"<rect x={-20} y={-20} width={120} height={120} fill="red"/>"#,
    );
    let frame = native(&mut Compositor::default(), &doc.root, 0.0);
    let bytes = frame.rgba(64, 48, false, None).unwrap();
    assert_eq!(pixel(&bytes, 64, 0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 16, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 32, 24), [255, 0, 0, 255]);
    let smaller = frame.rgba(32, 24, false, None).unwrap();
    assert_eq!(pixel(&smaller, 32, 0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&smaller, 32, 16, 12), [255, 0, 0, 255]);
    let mixed = document(
        "",
        r#"<rect width={32} height={32} cornerRadius={8} cornerRadiusTopLeft={0} cornerRadiusBottomRight={100} fill="blue"/>"#,
    );
    let bytes = native(&mut Compositor::default(), &mixed.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&bytes, 64, 0, 0), [0, 0, 255, 255]);
    assert_eq!(pixel(&bytes, 64, 31, 31), [0, 0, 0, 0]);
}

#[test]
fn opacity_is_inherited_per_draw_and_alpha_is_flattened_only_for_video() {
    let doc = document(
        "",
        r#"<group opacity={0.5}>
      <rect width={40} height={40} fill="rgba(255,0,0,0.01)"/>
      <group opacity={3}><rect x={20} width={40} height={40} fill="blue"/></group>
    </group>"#,
    );
    let frame = native(&mut Compositor::default(), &doc.root, 0.0);
    let rgba = frame.rgba(64, 48, false, None).unwrap();
    assert_eq!(pixel(&rgba, 64, 8, 8), [255, 0, 0, 128]);
    let overlap = pixel(&rgba, 64, 28, 8);
    assert!(
        (84..=86).contains(&overlap[0]) && (169..=171).contains(&overlap[2]) && overlap[3] == 192,
        "{overlap:?}"
    );
    let flattened = frame.rgba(64, 48, true, None).unwrap();
    assert_eq!(pixel(&flattened, 64, 8, 8), [128, 0, 0, 255]);
    assert_eq!(pixel(&flattened, 64, 28, 8), [64, 0, 128, 255]);
    assert_eq!(pixel(&flattened, 64, 63, 47), [0, 0, 0, 255]);
}

#[test]
fn group_bounds_use_previous_child_transforms_and_sequences_ignore_spatial_props() {
    let doc = document(
        "",
        r#"<group x={24} y={8} rotation={90}>
      <rect width={8} height={8} fill="red"/>
      <rect x={20} width={8} height={8} fill="blue"/>
      <rect x={400} width={400} height={400} hidden/>
    </group><sequence x={900} scale={0} rotation={45}>
      <rect x={2} y={38} width={4} height={4} fill="lime" end={1}/>
    </sequence>"#,
    );
    let mut compositor = Compositor::default();
    let first = native(&mut compositor, &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    let second = native(&mut compositor, &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&first, 64, 28, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&second, 64, 38, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&second, 64, 28, 12), [0, 0, 0, 0]);
    assert_eq!(pixel(&first, 64, 3, 39), [0, 255, 0, 255]);
    assert_eq!(pixel(&second, 64, 3, 39), [0, 255, 0, 255]);
    compositor.reset();
    assert_eq!(
        first,
        native(&mut compositor, &doc.root, 0.0)
            .rgba(64, 48, false, None)
            .unwrap()
    );
}

#[test]
fn visibility_uses_rounded_frame_boundaries_and_unpainted_nodes_stay_transparent() {
    let doc = document(
        "",
        r#"<rect width={64} height={48}/><rect width={8} height={8} fill="red" start={1} end={2}/>
      <rect x={16} width={8} height={8} fill="blue" scale={0}/>
      <rect x={40} y={20} width={-8} height={-8} fill="lime"/>
    "#,
    );
    let mut compositor = Compositor::default();
    let before = native(&mut compositor, &doc.root, 0.98)
        .rgba(64, 48, false, None)
        .unwrap();
    let start = native(&mut compositor, &doc.root, 0.99)
        .rgba(64, 48, false, None)
        .unwrap();
    let end = native(&mut compositor, &doc.root, 2.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&before, 64, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel(&start, 64, 2, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&end, 64, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel(&start, 64, 19, 3), [0, 0, 0, 0]);
    assert_eq!(pixel(&start, 64, 35, 15), [0, 255, 0, 255]);
}

#[test]
fn unsupported_native_properties_fail_explicitly_and_failed_plans_do_not_advance_geometry() {
    for (props, body) in [
        ("skewX={2}", ""),
        ("", r#"<rect keepAspectRatio/>"#),
        ("", r#"<rect cornerRadius={-1} fill="red"/>"#),
    ] {
        let doc = document(props, body);
        let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
        assert!(
            Compositor::default()
                .frame(
                    doc.root.find("s").unwrap(),
                    &timeline,
                    0.0,
                    30.0,
                    false,
                    None
                )
                .is_err()
        );
    }
    let nested = document("", "<scene/>");
    assert!(
        Timeline::resolve(&nested.root, &BTreeMap::new())
            .unwrap_err()
            .contains("Scenes cannot be nested")
    );
    let doc = document("", r#"<html/>"#);
    assert!(!Compositor::supports(doc.root.find("s").unwrap()));
    let doc = document(
        "",
        r#"<group x={24} y={8} rotation={90}><rect width={8} height={8} fill="red"/><rect x={20} width={8} height={8} fill="blue"/></group>"#,
    );
    let mut compositor = Compositor::default();
    let first = native(&mut compositor, &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    let mut bad = doc.root.clone();
    bad.children[0]
        .props
        .insert("cornerRadius".into(), (-1).into());
    let timeline = Timeline::resolve(&bad, &BTreeMap::new()).unwrap();
    assert!(
        compositor
            .frame(bad.find("s").unwrap(), &timeline, 0.0, 30.0, false, None)
            .is_err()
    );
    let actual = native(&mut compositor, &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    let mut expected = Compositor::default();
    native(&mut expected, &doc.root, 0.0);
    assert_eq!(
        actual,
        native(&mut expected, &doc.root, 0.0)
            .rgba(64, 48, false, None)
            .unwrap()
    );
    assert_ne!(first, actual);
}

#[test]
fn native_capture_keeps_alpha_refuses_overwrite_and_preview_uses_stage_background() {
    let doc = document(
        "",
        r#"<rect width={16} height={16} fill="red" opacity={0.5}/>"#,
    );
    let tools = unavailable_tools();
    let project = std::path::Path::new(".");
    let mut preview = ProjectRenderer::new(&doc, project, tools.clone(), false).unwrap();
    let bytes = preview
        .frame_plan("s", 0.0, 30.0, false)
        .unwrap()
        .frame(&tools, project)
        .unwrap();
    let overlap = pixel(&bytes, 64, 8, 8);
    assert_eq!(overlap, [128, 0, 127, 255]);
    assert_eq!(pixel(&bytes, 64, 60, 40), [0, 0, 255, 255]);
    let mut offline = ProjectRenderer::new(&doc, project, tools.clone(), true).unwrap();
    let mut plan = offline.frame_plan("s", 0.0, 30.0, false).unwrap();
    plan.resize_output(24).unwrap();
    assert_eq!((plan.width, plan.height), (32, 24));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder =
        std::env::temp_dir().join(format!("ad3-native-vector-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&folder).unwrap();
    let output = folder.join("capture.png");
    plan.capture(&tools, &folder, &output).unwrap();
    let before = std::fs::read(&output).unwrap();
    let png = tiny_skia::Pixmap::decode_png(&before).unwrap();
    assert_eq!((png.width(), png.height()), (32, 24));
    assert_eq!(pixel(&png.take_demultiplied(), 32, 3, 3), [255, 0, 0, 128]);
    assert!(plan.capture(&tools, &folder, &output).is_err());
    assert_eq!(before, std::fs::read(&output).unwrap());
    std::fs::remove_file(&output).unwrap();
    std::fs::remove_dir(folder.join(".diffusion/native-render")).unwrap();
    std::fs::remove_dir(folder.join(".diffusion")).unwrap();
    std::fs::remove_dir(&folder).unwrap();
}

#[test]
fn corner_and_color_tracks_preserve_authored_trait_membership() {
    let doc = document(
        "",
        r#"
      <rect id="mixed" width={24} height={24} fill="red" cornerRadius={8} cornerRadiusTopLeft={0}>
        <keyframeTrack property="cornerRadius"><keyframe value={0}/></keyframeTrack>
        <keyframeTrack property="cornerRadiusBottomLeft"><keyframe value={2}/></keyframeTrack>
      </rect>
      <rect id="uniform" x={32} width={24} height={24} fill="blue" cornerRadius={8}>
        <keyframeTrack property="cornerRadiusTopLeft"><keyframe value={0}/></keyframeTrack>
      </rect>
      <rect id="unpainted" y={30} width={64} height={18}>
        <keyframeTrack property="color"><keyframe value="lime"/></keyframeTrack>
      </rect>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let sampled = ad3_editor::motion::Motion::default()
        .sample(&doc.root, &timeline, 0.0, 30.0)
        .unwrap();
    let mixed = sampled.find("mixed").unwrap();
    assert_eq!(mixed.number("cornerRadiusTopRight", 0.0).unwrap(), 8.0);
    assert_eq!(mixed.number("cornerRadiusBottomLeft", 0.0).unwrap(), 2.0);
    assert!(
        !sampled
            .find("uniform")
            .unwrap()
            .props
            .contains_key("cornerRadiusTopLeft")
    );
    assert!(
        !sampled
            .find("unpainted")
            .unwrap()
            .props
            .contains_key("fill")
    );
    let frame = Compositor::default()
        .frame(sampled.find("s").unwrap(), &timeline, 0.0, 30.0, true, None)
        .unwrap();
    let bytes = frame.rgba(64, 48, false, None).unwrap();
    assert_eq!(pixel(&bytes, 64, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 23, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 32, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 5, 35), [0, 0, 0, 0]);
}

#[test]
fn rounded_output_width_keeps_original_uniform_capture_resolution() {
    let doc = document("fill=\"red\"", "");
    let tools = unavailable_tools();
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), tools.clone(), true).unwrap();
    let mut plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    plan.resize_output(26).unwrap();
    assert_eq!((plan.width, plan.height), (34, 26));
    // The original encoder sets one uniform resolution and independently
    // rounds its canvas bounds. It does not stretch to the rounded aspect.
    let bytes = plan.frame(&tools, std::path::Path::new(".")).unwrap();
    assert_eq!(pixel(&bytes, 34, 33, 12), [255, 0, 0, 255]);
    plan.resize_output(28).unwrap();
    // Resizing always starts from the authored native frame dimensions.
    assert_eq!((plan.width, plan.height), (38, 28));
    let bytes = plan.frame(&tools, std::path::Path::new(".")).unwrap();
    assert!(pixel(&bytes, 38, 37, 12)[3] < 100);
}

#[test]
fn a_mounted_scene_keeps_its_backend_and_rejects_new_unsupported_nodes() {
    let source = r#"import {useTicker} from '@diffusionstudio/jsx';
      export default ()=>{const t=useTicker();return <stage><scene id="s" active width={64} height={48} workarea={[0,3]}>
        <group x={24} y={8} rotation={90}><rect width={8} height={8} fill="red"/><rect x={t.time()<1?20:30} width={8} height={8} fill="blue"/></group>
        {t.time()>=1&&t.time()<2?<html/>:null}
      </scene></stage>};"#;
    let doc = dynamic_document("backend.tsx", source.into());
    let tools = unavailable_tools();
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), tools.clone(), true).unwrap();
    assert!(
        renderer
            .frame_plan("s", 0.0, 30.0, false)
            .unwrap()
            .is_native()
    );
    assert!(
        renderer
            .frame_plan("s", 1.0, 30.0, false)
            .err()
            .unwrap()
            .contains("switching compositors")
    );
    assert!(
        renderer
            .frame_plan("s", 2.0, 30.0, false)
            .err()
            .unwrap()
            .contains("reopening")
    );
    let mut reopened =
        ProjectRenderer::new(&doc, std::path::Path::new("."), tools.clone(), true).unwrap();
    assert!(
        reopened
            .frame_plan("s", 2.0, 30.0, false)
            .unwrap()
            .is_native()
    );
    let legacy_doc = dynamic_document("legacy-backend.tsx", source.replace("rotation={90}", ""));
    let mut legacy =
        ProjectRenderer::new(&legacy_doc, std::path::Path::new("."), tools, true).unwrap();
    assert!(legacy.frame_plan("s", 1.0, 30.0, false).is_err());
    assert!(
        !legacy
            .frame_plan("s", 2.0, 30.0, false)
            .unwrap()
            .is_native()
    );
}

#[test]
fn switching_scenes_retains_each_backend_and_group_history() {
    let doc=dynamic_document("scenes.tsx",r#"import {useTicker} from '@diffusionstudio/jsx';
      export default ()=>{const t=useTicker();return <stage>
        <scene id="a" active width={64} height={48} workarea={[0,3]}>
          <group x={24} y={8} rotation={90}><rect width={8} height={8} fill="red"/><rect x={20} width={8} height={8}/></group>
        </scene>
        <scene id="b" width={64} height={48} workarea={[0,3]}><rect fill="blue"/></scene>
        <scene id="l" width={64} height={48} workarea={[0,3]}>{t.time()<1?<html/>:null}</scene>
      </stage>};"#.into());
    let tools = unavailable_tools();
    let project = std::path::Path::new(".");
    let mut renderer = ProjectRenderer::new(&doc, project, tools.clone(), true).unwrap();
    assert!(renderer.frame_plan("l", 0.0, 30.0, false).is_err());
    let first = renderer
        .frame_plan("a", 0.0, 30.0, false)
        .unwrap()
        .frame(&tools, project)
        .unwrap();
    assert!(
        renderer
            .frame_plan("b", 0.0, 30.0, false)
            .unwrap()
            .is_native()
    );
    let second = renderer
        .frame_plan("a", 0.0, 30.0, false)
        .unwrap()
        .frame(&tools, project)
        .unwrap();
    assert_eq!(pixel(&first, 64, 28, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&second, 64, 38, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&second, 64, 28, 12), [0, 0, 0, 0]);
    assert!(
        !renderer
            .frame_plan("l", 2.0, 30.0, false)
            .unwrap()
            .is_native()
    );
}

#[test]
fn unsupported_properties_stop_native_history_until_remount_but_pixel_cancellation_does_not() {
    let doc=dynamic_document("transient-prop.tsx",r#"import {useTicker} from '@diffusionstudio/jsx';
      export default ()=>{const t=useTicker();return <stage><scene id="s" active width={64} height={48} workarea={[0,3]}>
        <group x={24} y={8} rotation={90} {...(t.time()>=1&&t.time()<2?{skewX:2}:{})}>
          <rect width={8} height={8} fill="red"/><rect x={t.time()<1?20:30} width={8} height={8} fill="blue"/>
        </group></scene></stage>};"#.into());
    let tools = unavailable_tools();
    let project = std::path::Path::new(".");
    let mut renderer = ProjectRenderer::new(&doc, project, tools.clone(), true).unwrap();
    let first = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert!(
        first
            .frame_cancellable(&tools, project, Some(&AtomicBool::new(true)))
            .unwrap_err()
            .contains("cancelled")
    );
    let second = renderer
        .frame_plan("s", 0.0, 30.0, false)
        .unwrap()
        .frame(&tools, project)
        .unwrap();
    assert_eq!(pixel(&second, 64, 38, 2), [255, 0, 0, 255]);
    let failed = renderer.frame_plan("s", 1.0, 30.0, false).err().unwrap();
    assert!(failed.contains("skewX") && failed.contains("reopening"));
    assert_eq!(
        renderer.frame_plan("s", 2.0, 30.0, false).err().unwrap(),
        failed
    );
    let mut reopened = ProjectRenderer::new(&doc, project, tools, true).unwrap();
    assert!(
        reopened
            .frame_plan("s", 2.0, 30.0, false)
            .unwrap()
            .is_native()
    );
}

#[test]
fn canvas_rounds_odd_dimensions_and_stays_fixed_while_scene_motion_changes_its_path() {
    let doc=SourceDocument::parse("odd.tsx".into(),r#"export default <stage>
      <scene id="s" active width={63} height={47} workarea={[0,2]}>
        <keyframeTrack property="width"><keyframe value={62.5}/><keyframe time={1} value={70.5}/></keyframeTrack>
        <rect width={100} height={100} fill="red"/>
      </scene></stage>"#.into()).unwrap();
    let tools = unavailable_tools();
    let project = std::path::Path::new(".");
    let mut renderer = ProjectRenderer::new(&doc, project, tools.clone(), true).unwrap();
    let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert_eq!((plan.width, plan.height), (64, 48));
    let bytes = plan.frame(&tools, project).unwrap();
    assert!((110..=145).contains(&pixel(&bytes, 64, 62, 4)[3]));
    assert_eq!(pixel(&bytes, 64, 63, 4), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 4, 47), [0, 0, 0, 0]);
    let plan = renderer.frame_plan("s", 1.0, 30.0, false).unwrap();
    assert_eq!((plan.width, plan.height), (64, 48));
    assert_eq!(
        pixel(&plan.frame(&tools, project).unwrap(), 64, 63, 4),
        [255, 0, 0, 255]
    );
    let doc = document("fill=\"red\"", "");
    let mut renderer = ProjectRenderer::new(&doc, project, tools.clone(), true).unwrap();
    let mut plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    plan.resize_output(25).unwrap();
    // scale=0.520833; both dimensions round independently after that.
    assert_eq!((plan.width, plan.height), (34, 24));
    let bytes = plan.frame(&tools, project).unwrap();
    assert!(pixel(&bytes, 34, 33, 12)[3] < 100);
}

#[test]
fn authored_sizes_round_before_motion_but_keyframed_sizes_remain_fractional() {
    let doc = document(
        "",
        r#"<rect id="r" width={10.4} height={8.6} fill="red">
      <keyframeTrack property="width"><keyframe value={10.5}/></keyframeTrack>
    </rect>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let sampled = ad3_editor::motion::Motion::default()
        .sample(&doc.root, &timeline, 0.0, 30.0)
        .unwrap();
    assert_eq!(
        sampled.find("r").unwrap().number("height", 0.0).unwrap(),
        9.0
    );
    assert_eq!(
        sampled.find("r").unwrap().number("width", 0.0).unwrap(),
        10.5
    );
    let frame = Compositor::default()
        .frame(sampled.find("s").unwrap(), &timeline, 0.0, 30.0, true, None)
        .unwrap();
    let bytes = frame.rgba(64, 48, false, None).unwrap();
    let edge = pixel(&bytes, 64, 10, 3);
    assert!(edge[3] > 110 && edge[3] < 145, "{edge:?}");
    assert_eq!(pixel(&bytes, 64, 9, 8), [255, 0, 0, 255]);
}

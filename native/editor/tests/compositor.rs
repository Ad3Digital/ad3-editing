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

#[test]
fn negative_rounded_size_reflects_each_logical_corner_and_preserves_pixels() {
    for (width, height, scale_x, scale_y) in [
        (24, 16, 1, 1),
        (-24, 16, -1, 1),
        (24, -16, 1, -1),
        (-24, -16, -1, -1),
    ] {
        let props = format!(
            r#"<rect id="r" x={{32}} y={{24}} width={{{width}}} height={{{height}}} cornerRadiusTopLeft={{2}} cornerRadiusTopRight={{4}} cornerRadiusBottomRight={{6}} cornerRadiusBottomLeft={{8}} fill="red"/>"#
        );
        let reflected = format!(
            r#"<rect id="r" x={{32}} y={{24}} width={{24}} height={{16}} scaleX={{{scale_x}}} scaleY={{{scale_y}}} cornerRadiusTopLeft={{2}} cornerRadiusTopRight={{4}} cornerRadiusBottomRight={{6}} cornerRadiusBottomLeft={{8}} fill="red"/>"#
        );
        // Scale is around the center, so compensate its pivot translation.
        let reflected = document("", &reflected)
            .set("r", "x", Some(serde_json::json!(32 + (width - 24) / 2)))
            .unwrap()
            .set("r", "y", Some(serde_json::json!(24 + (height - 16) / 2)))
            .unwrap();
        let negative = document("", &props);
        assert_eq!(
            native(&mut Compositor::default(), &negative.root, 0.0)
                .rgba(64, 48, false, None)
                .unwrap(),
            native(&mut Compositor::default(), &reflected.root, 0.0)
                .rgba(64, 48, false, None)
                .unwrap(),
            "{width} x {height}"
        );
    }
}

#[test]
fn text_mask_uses_authored_box_and_never_paints_glyphs() {
    let doc = document(
        "",
        r#"<text id="mask" mask x={8} y={8} width={24} height={16} fill="white">Mask</text><rect id="r" width={64} height={48} fill="red"/>"#,
    );
    let bytes = native(&mut Compositor::default(), &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&bytes, 64, 18, 16), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 4, 16), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 34, 16), [0, 0, 0, 0]);
}

#[test]
fn image_mask_uses_transformed_rounded_box_without_drawing_its_media() {
    let doc = document(
        "",
        r#"<image id="mask" mask x={8} y={8} width={24} height={16} cornerRadius={6} rotation={180}/><rect id="r" width={64} height={48} fill="red"/>"#,
    );
    let bytes = native(&mut Compositor::default(), &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&bytes, 64, 18, 16), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 4, 16), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 34, 16), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 8, 8), [0, 0, 0, 0]);
}

#[test]
fn text_aspect_lock_preserves_auto_size_and_applies_when_a_bound_is_authored() {
    let auto = document(
        "",
        r#"<text id="t" fontFamily="Inter" fontSize={16} keepAspectRatio fill="white">Text</text>"#,
    );
    let mut compositor = Compositor::default();
    native(&mut compositor, &auto.root, 0.0);
    let old = compositor.edit_geometry("t").unwrap().frame.size;
    assert!(old[0] > 0.0 && old[1] > 0.0);
    let authored = auto
        .set("t", "width", Some(serde_json::json!(old[0] * 2.0)))
        .unwrap();
    native(&mut compositor, &authored.root, 0.0);
    let resized = compositor.edit_geometry("t").unwrap().frame.size;
    assert_eq!(resized[0], (old[0] * 2.0).round());
    assert_eq!(resized[1], (old[1] * 2.0).round());
    let unlocked = authored.set("t", "keepAspectRatio", None).unwrap();
    let changed = unlocked
        .set("t", "width", Some(serde_json::json!(old[0] * 3.0)))
        .unwrap();
    native(&mut compositor, &changed.root, 0.0);
    assert_eq!(
        compositor.edit_geometry("t").unwrap().frame.size[1],
        resized[1]
    );
}

#[test]
fn aspect_locked_rect_size_is_resolved_before_motion_tracks() {
    let doc = document(
        "",
        r#"<rect id="r" width={20} keepAspectRatio fill="red"/>"#,
    );
    let mut compositor = Compositor::default();
    let bytes = native(&mut compositor, &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(
        compositor.edit_geometry("r").unwrap().frame.size,
        [20.0, 20.0]
    );
    assert_eq!(pixel(&bytes, 64, 10, 19), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 10, 21), [0, 0, 0, 0]);
    let animated = document(
        "",
        r#"<rect id="r" width={20} keepAspectRatio fill="red"><keyframeTrack property="height"><keyframe time={0} value={30}/></keyframeTrack></rect>"#,
    );
    let mut renderer = ProjectRenderer::new(
        &animated,
        std::path::Path::new("."),
        unavailable_tools(),
        false,
    )
    .unwrap();
    renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert_eq!(
        renderer.edit_geometries("s").unwrap()["r"].frame.size,
        [20.0, 30.0]
    );
}

#[test]
fn nested_scene_clips_descendants_and_restores_clip_for_siblings() {
    let doc = document(
        "",
        r#"<scene id="inner" x={8} y={6} width={20} height={16} fill="green"><rect id="r" x={-8} y={-6} width={64} height={48} fill="red"/></scene><rect id="sibling" x={40} y={30} width={8} height={8} fill="white"/>"#,
    );
    let frame = native(&mut Compositor::default(), &doc.root, 0.0);
    let bytes = frame.rgba(64, 48, false, None).unwrap();
    assert_eq!(pixel(&bytes, 64, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 4, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 30, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 10, 24), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 42, 32), [255, 255, 255, 255]);
}

#[test]
fn nested_scene_rounded_clip_intersects_ancestor_mask_in_world_space() {
    let doc = document(
        "",
        r#"<rect id="mask" mask x={16} width={20} height={48}/><scene id="inner" x={8} y={8} width={32} height={24} cornerRadius={8} rotation={180}><rect id="r" x={-100} y={-100} width={240} height={240} fill="red"/></scene>"#,
    );
    let bytes = native(&mut Compositor::default(), &doc.root, 0.0)
        .rgba(64, 48, false, None)
        .unwrap();
    assert_eq!(pixel(&bytes, 64, 20, 20), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 64, 12, 20), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 64, 38, 20), [0, 0, 0, 0]);
    assert!(pixel(&bytes, 64, 35, 8)[3] < 64);
    assert_eq!(pixel(&bytes, 64, 35, 16), [255, 0, 0, 255]);
}

#[test]
fn nested_scene_timing_gates_children_and_keeps_parent_source_clock() {
    let doc = document(
        "",
        r#"<scene id="inner" x={8} y={6} width={20} height={16} start={0.5} sourceIn={1} sourceOut={3} playbackRate={2}><rect id="r" width={64} height={48} fill="red" end={6}/></scene>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let child = timeline.get("r").unwrap();
    assert_eq!(child.scene_id, "s");
    assert_eq!(child.start, 0.5);
    assert_eq!(child.end, 1.5);
    assert_eq!(child.source_at(1.0), 2.0);
    let mut compositor = Compositor::default();
    for (time, expected) in [(0.0, 0), (0.5, 255), (1.0, 255), (1.5, 0)] {
        let bytes = native(&mut compositor, &doc.root, time)
            .rgba(64, 48, false, None)
            .unwrap();
        assert_eq!(pixel(&bytes, 64, 12, 12)[3], expected, "time {time}");
    }
}

#[test]
fn runtime_skew_is_rendered_without_changing_authored_source() {
    let doc = document(
        "",
        r#"<rect id="r" x={16} y={12} width={16} height={12} fill="red"/>"#,
    );
    let source = doc.source.clone();
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), unavailable_tools(), false).unwrap();
    renderer
        .set_runtime_skews(BTreeMap::from([("r".into(), [30.0, 0.0])]))
        .unwrap();
    let skewed = renderer
        .frame_plan("s", 0.0, 30.0, false)
        .unwrap()
        .frame(&unavailable_tools(), std::path::Path::new("."))
        .unwrap();
    let geometry = renderer.edit_geometries("s").unwrap()["r"];
    assert_eq!(geometry.transform.skew, [30.0, 0.0]);
    assert_eq!(geometry.local, geometry.transform.local_matrix());
    assert!(
        renderer
            .set_runtime_skews(BTreeMap::from([("r".into(), [f64::NAN, 0.0])]))
            .is_err()
    );
    renderer.set_runtime_skews(BTreeMap::new()).unwrap();
    let plain = renderer
        .frame_plan("s", 0.0, 30.0, false)
        .unwrap()
        .frame(&unavailable_tools(), std::path::Path::new("."))
        .unwrap();
    assert_ne!(skewed, plain);
    assert_eq!(
        renderer.edit_geometries("s").unwrap()["r"].transform.skew,
        [0.0, 0.0]
    );
    assert_eq!(doc.source, source);
}

#[test]
fn renderer_edit_geometry_uses_the_sampled_preview_frame() {
    let doc = document(
        "",
        r#"<rect id="r" width={8} height={8}><keyframeTrack property="x"><keyframe time={0} value={0}/><keyframe time={2} value={20}/></keyframeTrack></rect>"#,
    );
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), unavailable_tools(), false).unwrap();
    assert!(renderer.edit_geometries("s").is_none());
    renderer.frame_plan("s", 1.0, 30.0, false).unwrap();
    assert_eq!(
        renderer.edit_geometries("s").unwrap()["r"]
            .transform
            .position[0],
        10.0
    );
    renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert_eq!(
        renderer.edit_geometries("s").unwrap()["r"]
            .transform
            .position[0],
        0.0
    );
}

#[test]
fn dissolving_group_bakes_transform_preserves_timing_and_pixels() {
    let doc = document(
        "",
        r#"<group id="g" x={8} y={4} start={0.5} sourceIn={1} playbackRate={2}><rect id="r" x={4} y={4} width={12} height={10} fill="red" start={1} end={3}/></group>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let mut compositor = Compositor::default();
    native(&mut compositor, &doc.root, 1.0);
    native(&mut compositor, &doc.root, 1.0);
    let geometry = doc
        .root
        .nodes()
        .into_iter()
        .filter_map(|node| {
            compositor
                .edit_geometry(&node.id)
                .map(|geometry| (node.id.clone(), geometry))
        })
        .collect();
    let (next, released, _) = doc
        .dissolve_container(&timeline, "g", 1.0, &geometry)
        .unwrap();
    assert_eq!(released, vec!["r"]);
    assert!(next.root.find("g").is_none());
    let next_timeline = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    for time in [0.5, 1.0, 1.25] {
        assert_eq!(
            timeline.get("r").unwrap().source_at(time),
            next_timeline.get("r").unwrap().source_at(time)
        );
        let before = native(&mut compositor, &doc.root, time)
            .rgba(64, 48, false, None)
            .unwrap();
        let after = native(&mut Compositor::default(), &next.root, time)
            .rgba(64, 48, false, None)
            .unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn wrapping_scene_inside_rate_sequence_preserves_parent_space_and_clip_windows() {
    let doc = document(
        "",
        r#"<group id="parent" x={8} y={4}><sequence id="seq" start={0.5} sourceIn={1} playbackRate={2}><rect id="a" x={4} y={4} width={12} height={10} fill="red" start={1} end={2}/><rect id="b" x={20} y={4} width={8} height={10} fill="green" start={2} end={3}/></sequence></group>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let mut compositor = Compositor::default();
    native(&mut compositor, &doc.root, 0.75);
    native(&mut compositor, &doc.root, 0.75);
    let (next, scene) = doc
        .wrap_scene(
            &timeline,
            &["a".into(), "b".into()],
            0.75,
            compositor.edit_geometries(),
        )
        .unwrap();
    assert_eq!(
        next.root.find("seq").unwrap().children[0].id,
        scene.unwrap()
    );
    let resolved = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    for id in ["a", "b"] {
        let before = timeline.get(id).unwrap();
        let after = resolved.get(id).unwrap();
        assert_eq!(
            [
                before.start,
                before.end,
                before.origin,
                before.playback_rate
            ],
            [after.start, after.end, after.origin, after.playback_rate]
        );
    }
    let mut after = Compositor::default();
    for time in [0.5, 0.75, 1.0, 1.25] {
        native(&mut after, &next.root, time);
        assert_eq!(
            native(&mut compositor, &doc.root, time)
                .rgba(64, 48, false, None)
                .unwrap(),
            native(&mut after, &next.root, time)
                .rgba(64, 48, false, None)
                .unwrap()
        );
    }
}

#[test]
fn wrapping_scene_rounds_rotated_bounds_and_syncs_animated_position() {
    let doc = document(
        "",
        r#"<rect id="r" x={10} y={12} width={12} height={8} rotation={30} fill="red"><keyframeTrack id="track" property="x"><keyframe id="k0" time={0} value={10}/><keyframe id="k1" time={2} value={30}/></keyframeTrack></rect>"#,
    );
    let mut renderer =
        ProjectRenderer::new(&doc, std::path::Path::new("."), unavailable_tools(), false).unwrap();
    let before = renderer
        .frame_plan("s", 1.0, 30.0, false)
        .unwrap()
        .frame(&unavailable_tools(), std::path::Path::new("."))
        .unwrap();
    let geometry = renderer.edit_geometries("s").unwrap();
    let rect = geometry["r"]
        .frame
        .bounds(geometry["r"].transform.local_matrix());
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let (next, scene) = doc
        .wrap_scene(&timeline, &["r".into()], 1.0, geometry)
        .unwrap();
    let scene = next.root.find(scene.as_ref().unwrap()).unwrap();
    assert_eq!(scene.number("x", 0.0).unwrap(), rect.min_x.floor());
    assert_eq!(
        scene.number("width", 0.0).unwrap(),
        rect.max_x.ceil() - rect.min_x.floor()
    );
    let resolved = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    let points = next.keyframe_points(&resolved, "r", "x").unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points[1].1, 1.0);
    assert_eq!(points[1].2, serde_json::json!(20.0 - rect.min_x.floor()));
    assert_eq!(
        next.root.find("k0").unwrap().number("value", 0.0).unwrap(),
        10.0
    );
    let mut after_renderer =
        ProjectRenderer::new(&next, std::path::Path::new("."), unavailable_tools(), false).unwrap();
    let after = after_renderer
        .frame_plan("s", 1.0, 30.0, false)
        .unwrap()
        .frame(&unavailable_tools(), std::path::Path::new("."))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn wrapping_scene_measures_sequence_leaves_and_preserves_pixels_and_clock() {
    let doc = document(
        "",
        r#"<sequence id="seq"><rect id="a" x={8} y={6} width={12} height={10} fill="red" end={1}/><rect id="b" x={24} y={10} width={8} height={12} fill="green" start={1} end={2}/></sequence><rect id="other" x={40} width={4} height={4} fill="blue"/>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let mut compositor = Compositor::default();
    native(&mut compositor, &doc.root, 0.0);
    let (next, scene) = doc
        .wrap_scene(
            &timeline,
            &["seq".into()],
            0.0,
            compositor.edit_geometries(),
        )
        .unwrap();
    let scene = next.root.find(scene.as_ref().unwrap()).unwrap();
    assert_eq!(scene.tag, "scene");
    assert_eq!(scene.number("x", 0.0).unwrap(), 8.0);
    assert_eq!(scene.number("y", 0.0).unwrap(), 6.0);
    assert_eq!(scene.number("width", 0.0).unwrap(), 24.0);
    assert_eq!(scene.number("height", 0.0).unwrap(), 16.0);
    let resolved = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    for time in [0.0, 0.5, 1.0, 1.5] {
        assert_eq!(
            timeline.get("b").unwrap().source_at(time),
            resolved.get("b").unwrap().source_at(time)
        );
        assert_eq!(
            native(&mut compositor, &doc.root, time)
                .rgba(64, 48, false, None)
                .unwrap(),
            native(&mut Compositor::default(), &next.root, time)
                .rgba(64, 48, false, None)
                .unwrap()
        );
    }
}

#[test]
fn dissolving_nested_scene_bakes_position_and_preserves_child_clock_and_pixels() {
    let doc = document(
        "",
        r#"<scene id="inner" x={8} y={4} width={40} height={32} start={0.5} sourceIn={1} playbackRate={2}><sequence id="seq"><rect id="r" x={4} y={4} width={12} height={10} fill="red" start={1} end={3}/></sequence></scene>"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let mut compositor = Compositor::default();
    native(&mut compositor, &doc.root, 1.0);
    let geometry = compositor.edit_geometries().clone();
    let (next, released, _) = doc
        .dissolve_container(&timeline, "inner", 1.0, &geometry)
        .unwrap();
    assert_eq!(released, vec!["seq"]);
    assert!(next.root.find("inner").is_none());
    assert_eq!(next.root.find("r").unwrap().number("x", 0.0).unwrap(), 12.0);
    assert_eq!(next.root.find("r").unwrap().number("y", 0.0).unwrap(), 8.0);
    let after = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    for time in [0.5, 1.0, 1.25] {
        assert_eq!(
            timeline.get("r").unwrap().source_at(time),
            after.get("r").unwrap().source_at(time)
        );
        assert_eq!(
            native(&mut compositor, &doc.root, time)
                .rgba(64, 48, false, None)
                .unwrap(),
            native(&mut Compositor::default(), &next.root, time)
                .rgba(64, 48, false, None)
                .unwrap()
        );
    }
    assert!(
        doc.dissolve_container(&timeline, "s", 1.0, &geometry)
            .unwrap_err()
            .contains("root scene")
    );
}

#[test]
fn edit_geometry_tracks_successful_frame_and_resets_without_stale_nodes() {
    let doc = document(
        "",
        r#"<group id="g" x={12} rotation={15}><rect id="r" x={8} y={6} width={12} height={10} fill="red"/></group>"#,
    );
    let mut compositor = Compositor::default();
    assert!(compositor.edit_geometry("r").is_none());
    native(&mut compositor, &doc.root, 0.0);
    let rect = compositor.edit_geometry("r").unwrap();
    let group = compositor.edit_geometry("g").unwrap();
    assert_eq!(rect.transform.position, [8.0, 6.0]);
    assert_eq!(rect.transform.size, [12.0, 10.0]);
    assert_eq!(rect.local, rect.transform.local_matrix());
    assert_eq!(rect.world, group.world.multiply(rect.local));
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    assert!(
        compositor
            .frame(
                doc.root.find("s").unwrap(),
                &timeline,
                f64::NAN,
                30.0,
                false,
                None
            )
            .is_err()
    );
    assert_eq!(compositor.edit_geometry("r").unwrap().world, rect.world);
    let next = document("", r#"<rect id="new" width={4} height={4}/>"#);
    native(&mut compositor, &next.root, 0.0);
    assert!(compositor.edit_geometry("r").is_none());
    assert!(compositor.edit_geometry("new").is_some());
    compositor.reset();
    assert!(compositor.edit_geometry("new").is_none());
}

#[test]
fn source_group_preserves_rendered_positions_across_frames() {
    let doc = document(
        "",
        r#"<rect id="a" x={8} y={6} width={12} height={10} rotation={15} fill="red" start={0} end={2}/><rect id="b" x={32} y={16} width={18} height={14} fill="lime" start={0.5} end={2}/>"#,
    );
    let (grouped, _) = doc.group(&["a".into(), "b".into()]).unwrap();
    let mut original_compositor = Compositor::default();
    let mut grouped_compositor = Compositor::default();
    for time in [0.0, 1.0 / 30.0, 0.5, 1.0, 1.5] {
        let before = native(&mut original_compositor, &doc.root, time)
            .rgba(64, 48, false, None)
            .unwrap();
        let after = native(&mut grouped_compositor, &grouped.root, time)
            .rgba(64, 48, false, None)
            .unwrap();
        assert_eq!(after, before, "group changed pixels at {time}");
    }
}

#[test]
fn source_restack_changes_composited_overlap_and_back_restores_it() {
    let doc = document(
        "",
        r#"<rect id="red" width={32} height={32} fill="red"/><rect id="blue" width={32} height={32} fill="blue"/>"#,
    );
    let sample = |doc: &SourceDocument| {
        let pixels = native(&mut Compositor::default(), &doc.root, 0.0)
            .rgba(64, 48, false, None)
            .unwrap();
        pixel(&pixels, 64, 16, 16)
    };
    assert_eq!(sample(&doc), [0, 0, 255, 255]);
    let front = doc.restack(&["red".into()], true).unwrap();
    assert_eq!(sample(&front), [255, 0, 0, 255]);
    let back = front.restack(&["red".into()], false).unwrap();
    assert_eq!(sample(&back), [0, 0, 255, 255]);
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
        ("", r#"<rect skewY={2}/>"#),
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
    let source = r#"import {useTicker} from '@ad3-editing/jsx';
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
    let doc=dynamic_document("scenes.tsx",r#"import {useTicker} from '@ad3-editing/jsx';
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
    let doc=dynamic_document("transient-prop.tsx",r#"import {useTicker} from '@ad3-editing/jsx';
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

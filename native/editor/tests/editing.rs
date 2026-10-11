// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    source::{History, SourceDocument},
    timing::Timeline,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn parse(source: &str) -> SourceDocument {
    SourceDocument::parse(PathBuf::from("index.tsx"), source.into()).unwrap()
}

const PROJECT: &str = r##"// edição humana — preserve este comentário
const size = { width: 1920, height: 1080 };
export default function Project() {
  const rate = 2;
  return <stage><scene id="main" active width={size.width} height={size.height} workarea={[1, 7]}>
    <sequence id="track">
      <video id="take" src="take.mp4" start={1} sourceIn={2} sourceOut={14} playbackRate={rate} volume={-6} />
      <text id="title" start={2} end={4} fill="#fff">Ação &amp; café</text>
    </sequence>
  </scene></stage>;
}
"##;

#[test]
fn moving_container_carries_synced_descendants_without_realigning_to_external_reference() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g" playbackRate={2}><audio id="audio" src="a.wav" syncTo="reference"/></group><audio id="reference" src="b.wav"/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("audio".into(), 2.0), ("reference".into(), 3.0)]);
    let offsets = BTreeMap::from([("audio".into(), 0.4)]);
    let timeline =
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &offsets).unwrap();
    let moved = doc
        .move_clips(&timeline, &["g".into(), "audio".into()], 1.0)
        .unwrap();
    let after = Timeline::resolve(&moved.root, &durations).unwrap();
    assert!(
        !moved
            .root
            .find("audio")
            .unwrap()
            .props
            .contains_key("syncTo")
    );
    assert_eq!(
        [
            after.get("audio").unwrap().start,
            after.get("audio").unwrap().end
        ],
        [1.4, 2.4]
    );
    assert_eq!(after.get("audio").unwrap().playback_rate, 2.0);
    assert_eq!(after.get("reference").unwrap().start, 0.0);
}

#[test]
fn moving_synced_audio_freezes_placement_and_persists_manual_offset() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><audio id="audio" src="a.wav" syncTo="reference"/><audio id="reference" src="b.wav"/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("audio".into(), 2.0), ("reference".into(), 3.0)]);
    let offsets = BTreeMap::from([("audio".into(), 0.4)]);
    let timeline =
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &offsets).unwrap();
    let moved = doc.move_clip(&timeline, "audio", 1.4).unwrap();
    assert!(
        !moved
            .root
            .find("audio")
            .unwrap()
            .props
            .contains_key("syncTo")
    );
    let reopened = parse(&moved.source);
    let resolved = Timeline::resolve(&reopened.root, &durations).unwrap();
    let audio = resolved.get("audio").unwrap();
    assert_eq!(
        [audio.start, audio.end, audio.source_in, audio.source_out],
        [1.4, 3.4, 0.0, 2.0]
    );
    assert!((audio.source_at(2.0) - timeline.get("audio").unwrap().source_at(1.0)).abs() < 1e-12);
    assert_eq!(resolved.get("reference").unwrap().start, 0.0);
    let unchanged = doc.move_clips(&timeline, &["audio".into()], 0.0).unwrap();
    assert!(
        unchanged
            .root
            .find("audio")
            .unwrap()
            .props
            .contains_key("syncTo")
    );
}

#[test]
fn synced_clip_split_and_trim_preserve_recording_alignment() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><audio id="audio" src="a.wav" syncTo="reference"/><audio id="reference" src="b.wav"/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([
        ("audio".into(), 2.0),
        ("audio-2".into(), 2.0),
        ("reference".into(), 3.0),
    ]);
    let offsets = BTreeMap::from([("audio".into(), 0.403), ("audio-2".into(), 0.403)]);
    let timeline =
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &offsets).unwrap();
    let split = doc.split(&timeline, "audio", 1.0).unwrap();
    let split_time =
        Timeline::resolve_with_sync_offsets(&split.root, &durations, 30.0, &offsets).unwrap();
    assert_eq!(split_time.get("audio").unwrap().end, 1.0);
    assert_eq!(split_time.get("audio-2").unwrap().start, 1.0);
    for id in ["audio", "audio-2"] {
        assert_eq!(
            split.root.find(id).unwrap().string("syncTo"),
            Some("reference")
        );
        assert_eq!(
            split_time.get(id).unwrap().origin,
            timeline.get("audio").unwrap().origin
        );
    }
    let trimmed = split.trim(&split_time, "audio-2", 1.2, 2.0).unwrap();
    let trimmed_time =
        Timeline::resolve_with_sync_offsets(&trimmed.root, &durations, 30.0, &offsets).unwrap();
    let trimmed = trimmed_time.get("audio-2").unwrap();
    assert_eq!([trimmed.start, trimmed.end], [1.2, 2.0]);
    assert_eq!(
        trimmed.source_at(1.5),
        timeline.get("audio").unwrap().source_at(1.5)
    );
}

#[test]
fn linked_timeline_snaps_visual_edges_but_preserves_audio_origin_and_duration_cap() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><audio id="audio" src="a.wav" syncTo="reference" sourceOut={8}/><audio id="reference" src="b.wav"/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("audio".into(), 2.0), ("reference".into(), 10.0)]);
    let offsets = BTreeMap::from([("audio".into(), 0.403)]);
    for fps in [24.0, 30.0, 60.0] {
        let timeline =
            Timeline::resolve_with_sync_offsets(&doc.root, &durations, fps, &offsets).unwrap();
        let audio = timeline.get("audio").unwrap();
        assert_eq!(audio.origin, 0.403);
        assert_eq!(audio.source_out, 2.0);
        assert!((audio.start * fps - (audio.start * fps).round()).abs() < 1e-12);
        assert!((audio.end * fps - (audio.end * fps).round()).abs() < 1e-12);
        assert!((audio.start - 0.403).abs() <= 0.5 / fps);
        assert!((audio.end - 2.403).abs() <= 0.5 / fps);
    }
}

#[test]
fn linked_timeline_resolves_forward_chains_and_refits_containers() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g"><audio id="second" src="second.wav" syncTo="first"/><audio id="first" src="first.wav" syncTo="reference"/><video id="reference" src="reference.mp4" start={1} sourceIn={2} sourceOut={8}/></group></scene></stage>;"#,
    );
    let durations = BTreeMap::from([
        ("second".into(), 10.0),
        ("first".into(), 10.0),
        ("reference".into(), 10.0),
    ]);
    let offsets = BTreeMap::from([("first".into(), 0.403), ("second".into(), 0.2)]);
    let timeline =
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &offsets).unwrap();
    assert!((timeline.get("first").unwrap().origin + 0.597).abs() < 1e-12);
    assert!((timeline.get("second").unwrap().origin + 0.397).abs() < 1e-12);
    for id in ["reference", "first", "second", "g", "s"] {
        let span = timeline.get(id).unwrap();
        assert!((span.start - 1.0).abs() < 1e-12, "{id}");
        assert!((span.end - 7.0).abs() < 1e-12, "{id}");
    }
    assert!(doc.root.find("first").unwrap().props.contains_key("syncTo"));
    assert!(
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &BTreeMap::new())
            .unwrap_err()
            .contains("missing measured")
    );
}

#[test]
fn linked_timeline_rejects_cycles_and_missing_references() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><audio id="a" src="a.wav" syncTo="b"/><audio id="b" src="b.wav" syncTo="a"/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("a".into(), 4.0), ("b".into(), 4.0)]);
    assert!(
        Timeline::resolve_with_sync_offsets(&doc.root, &durations, 30.0, &BTreeMap::new())
            .unwrap_err()
            .contains("cycle")
    );
    let missing = doc.set("a", "syncTo", Some(json!("absent"))).unwrap();
    assert!(
        Timeline::resolve_with_sync_offsets(&missing.root, &durations, 30.0, &BTreeMap::new())
            .unwrap_err()
            .contains("does not exist")
    );
}

#[test]
fn clipboard_remaps_links_across_selected_roots_and_inserts_before_anchor() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><video id="v" src="take.mp4"/><audio id="audio" src="take.mp4" syncTo="v"/><rect id="anchor"/></scene></stage>;"#,
    );
    let clipboard = doc
        .copy_selection(&["v".into(), "audio".into(), "v".into()])
        .unwrap()
        .unwrap();
    let (next, copies) = doc
        .paste_selection(&clipboard, "s", Some("anchor"))
        .unwrap();
    assert_eq!(copies, vec!["v-2", "audio-2"]);
    assert_eq!(
        next.root.find("audio-2").unwrap().string("syncTo"),
        Some("v-2")
    );
    assert_eq!(
        next.root
            .find("s")
            .unwrap()
            .children
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["v", "audio", "v-2", "audio-2", "anchor"]
    );
    assert!(
        doc.paste_selection(&clipboard, "s", Some("missing"))
            .is_err()
    );
}

#[test]
fn clipboard_snapshot_survives_cut_and_pastes_outside_original_sequence() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><sequence id="seq"><rect id="a" selected x={8}><rect id="mask" mask/></rect><rect id="b"/></sequence><group id="empty"/></scene></stage>;"#,
    );
    let clipboard = doc
        .copy_selection(&["a".into(), "mask".into()])
        .unwrap()
        .unwrap();
    let cut = doc.remove("a").unwrap();
    let (pasted, copies) = cut.paste_selection(&clipboard, "seq", None).unwrap();
    assert_eq!(copies, vec!["a-2"]);
    assert_eq!(
        pasted
            .root
            .find("s")
            .unwrap()
            .children
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["seq", "a-2", "empty"]
    );
    let copy = pasted.root.find("a-2").unwrap();
    assert_eq!(copy.number("x", 0.0).unwrap(), 8.0);
    assert!(!copy.props.contains_key("selected"));
    assert_eq!(copy.children[0].id, "mask-2");
    let (again, copies) = pasted.paste_selection(&clipboard, "empty", None).unwrap();
    assert_eq!(copies, vec!["a-3"]);
    assert_eq!(again.root.find("empty").unwrap().children[0].id, "a-3");
    assert!(doc.copy_selection(&[]).unwrap().is_none());
}

#[test]
fn wrapping_sequence_preserves_earlier_clip_and_trims_or_removes_later_overlaps() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="a" end={2}/><rect id="covered" start={1} end={1.5}/><rect id="b" start={1} end={4}/></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let (next, id) = doc
        .wrap_sequence(&timeline, &["a".into(), "covered".into(), "b".into()])
        .unwrap();
    let sequence = next.root.find(id.as_ref().unwrap()).unwrap();
    assert_eq!(sequence.tag, "sequence");
    assert_eq!(sequence.string("name"), Some("Sequence 1"));
    assert_eq!(
        sequence
            .children
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    let resolved = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    assert_eq!(resolved.get("a").unwrap().end, 2.0);
    assert_eq!(resolved.get("b").unwrap().start, 2.0);
    assert_eq!(resolved.get("b").unwrap().source_in, 1.0);
    assert_eq!(resolved.get("b").unwrap().end, 4.0);
}

#[test]
fn wrapping_sequence_trims_nested_rate_without_changing_source_clock() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="a" end={2}/><group id="g" start={1} playbackRate={2}><rect id="covered" end={1}/><group id="nested"><video id="v" src="take.mp4" sourceIn={4} sourceOut={10}/></group></group></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("v".into(), 10.0)]);
    let timeline = Timeline::resolve(&doc.root, &durations).unwrap();
    let original = timeline.get("v").unwrap();
    let (next, _) = doc
        .wrap_sequence(&timeline, &["a".into(), "g".into()])
        .unwrap();
    assert!(next.root.find("covered").is_none());
    let resolved = Timeline::resolve(&next.root, &durations).unwrap();
    let trimmed = resolved.get("v").unwrap();
    assert_eq!(trimmed.start, 2.0);
    assert_eq!(trimmed.end, original.end);
    assert_eq!(trimmed.source_in, 6.0);
    for time in [2.0, 2.5, 3.5] {
        assert_eq!(trimmed.source_at(time), original.source_at(time));
    }
    assert_eq!(resolved.get("g").unwrap().start, 2.0);
    assert_eq!(resolved.get("nested").unwrap().start, 2.0);
}

#[test]
fn container_transform_bake_writes_props_and_existing_keyframes() {
    use ad3_editor::geometry::{Mat2D, TransformValues};
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="r" x={0} scaleX={1} scaleY={1}><keyframeTrack id="t" property="x"><keyframe time={0} value={10}/><keyframe time={2} value={30}/></keyframeTrack></rect></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let child = TransformValues {
        position: [20.0, 0.0],
        ..Default::default()
    };
    let (next, baked) = doc
        .bake_container_transform(
            &timeline,
            "r",
            1.0,
            child,
            Mat2D::translate(8.0, 4.0).multiply(Mat2D::scale(2.0, 2.0)),
        )
        .unwrap();
    let node = next.root.find("r").unwrap();
    assert_eq!(node.number("x", 0.0).unwrap(), 48.0);
    assert_eq!(node.number("y", 0.0).unwrap(), 4.0);
    assert_eq!(node.number("scale", 1.0).unwrap(), 2.0);
    assert!(!node.props.contains_key("scaleX"));
    assert!(!node.props.contains_key("scaleY"));
    let sample = ad3_editor::motion::Motion::default()
        .sample(&next.root, &timeline, 1.0, 30.0)
        .unwrap();
    assert_eq!(
        sample.find("r").unwrap().number("x", 0.0).unwrap(),
        baked.position[0]
    );
    assert_eq!(next.root.find("t").unwrap().children.len(), 3);
    let (unchanged, _) = doc
        .bake_container_transform(&timeline, "r", 1.0, child, Mat2D::IDENTITY)
        .unwrap();
    assert_eq!(unchanged.source, doc.source);
    let (nonuniform, _) = doc
        .bake_container_transform(&timeline, "r", 1.0, child, Mat2D::scale(2.0, 3.0))
        .unwrap();
    let node = nonuniform.root.find("r").unwrap();
    assert_eq!(node.number("scaleX", 1.0).unwrap(), 2.0);
    assert_eq!(node.number("scaleY", 1.0).unwrap(), 3.0);
    assert!(!node.props.contains_key("scale"));
}

#[test]
fn moving_keyframe_batch_respects_each_owner_rate_and_deduplicates_ids() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="a" playbackRate={2}><keyframeTrack property="x"><keyframe id="pa" time={1} value={10}/></keyframeTrack></rect><rect id="b" playbackRate={0.5}><keyframeTrack property="x"><keyframe id="pb" time={1} value={20}/></keyframeTrack></rect></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let next = doc
        .move_keyframes(&timeline, &["pa".into(), "pb".into(), "pa".into()], 0.5)
        .unwrap();
    assert_eq!(
        next.root.find("pa").unwrap().number("time", 0.0).unwrap(),
        2.0
    );
    assert_eq!(
        next.root.find("pb").unwrap().number("time", 0.0).unwrap(),
        ad3_editor::timing::snap(1.25)
    );
    assert!(
        doc.move_keyframes(&timeline, &["pa".into()], f64::NAN)
            .is_err()
    );
}

#[test]
fn keyframe_targets_include_nested_paints_without_duplicating_child_clip_tracks() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g"><keyframeTrack property="x"/><rect id="r"><solidPaint id="fill"><keyframeTrack property="color"/></solidPaint><stroke id="stroke"><keyframeTrack property="width"/></stroke></rect></group></scene></stage>;"#,
    );
    assert_eq!(
        doc.keyframe_targets("g").unwrap(),
        vec![("g".into(), "x".into())]
    );
    assert_eq!(
        doc.keyframe_targets("r").unwrap(),
        vec![
            ("fill".into(), "color".into()),
            ("stroke".into(), "width".into())
        ]
    );
}

#[test]
fn moving_keyframe_uses_source_clock_and_preserves_value_and_easing() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g" start={1} sourceIn={2} playbackRate={2}><rect id="r" start={3} sourceIn={4} playbackRate={0.5} end={7}><keyframeTrack property="x"><keyframe id="p" time={4} value={10} easing="easeIn"/></keyframeTrack></rect></group></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let span = timeline.get("r").unwrap();
    let at = span.start + 0.5;
    let next = doc.move_keyframe(&timeline, "p", at).unwrap();
    let point = next.root.find("p").unwrap();
    assert_eq!(point.props["time"].as_f64(), Some(span.source_at(at)));
    assert_eq!(point.props["value"].as_f64(), Some(10.0));
    assert_eq!(point.string("easing"), Some("easeIn"));
    let clamped = doc.move_keyframe(&timeline, "p", -100.0).unwrap();
    assert_eq!(
        clamped.root.find("p").unwrap().props["time"].as_f64(),
        Some(0.0)
    );
    assert!(doc.move_keyframe(&timeline, "p", f64::NAN).is_err());
    assert!(doc.move_keyframe(&timeline, "r", at).is_err());
}

#[test]
fn keyframe_easing_edits_change_interpolation_and_linear_removes_override() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="r"><keyframeTrack property="x"><keyframe id="p" time={0} value={0}/><keyframe time={2} value={100}/></keyframeTrack></rect></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let eased = doc
        .set_keyframe_easing("p", "cubicBezier(0.42,0,1,1)")
        .unwrap();
    let sample = ad3_editor::motion::Motion::default()
        .sample(&eased.root, &timeline, 1.0, 30.0)
        .unwrap();
    assert!(sample.find("r").unwrap().number("x", 0.0).unwrap() < 40.0);
    let linear = eased.set_keyframe_easing("p", "linear").unwrap();
    assert!(!linear.root.find("p").unwrap().props.contains_key("easing"));
    let sample = ad3_editor::motion::Motion::default()
        .sample(&linear.root, &timeline, 1.0, 30.0)
        .unwrap();
    assert_eq!(sample.find("r").unwrap().number("x", 0.0).unwrap(), 50.0);
    assert!(doc.set_keyframe_easing("r", "easeIn").is_err());
    assert!(doc.set_keyframe_easing("p", "steps(0)").is_err());
}

#[test]
fn keyframe_navigation_sorts_points_and_converts_nested_source_clock() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g" start={1} sourceIn={2} playbackRate={2}><rect id="r" start={3} sourceIn={4} playbackRate={0.5} end={7}><keyframeTrack property="x"><keyframe id="late" time={5} value={20}/><keyframe id="early" time={4} value={10}/></keyframeTrack></rect></group></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let points = doc.keyframe_points(&timeline, "r", "x").unwrap();
    assert_eq!(
        points
            .iter()
            .map(|point| point.0.as_str())
            .collect::<Vec<_>>(),
        vec!["early", "late"]
    );
    let span = timeline.get("r").unwrap();
    for ((_, scene_time, _), source_time) in points.iter().zip([4.0, 5.0]) {
        assert!((span.source_at(*scene_time) - source_time).abs() < 1e-9);
    }
    assert!(doc.keyframe_points(&timeline, "r", "y").unwrap().is_empty());
}

#[test]
fn removing_property_track_preserves_other_tracks_and_authored_value() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="r" x={8}><keyframeTrack id="x-track" property="x"><keyframe time={0} value={1}/><keyframe time={2} value={20}/></keyframeTrack><keyframeTrack id="y-track" property="y"><keyframe time={0} value={4}/></keyframeTrack></rect></scene></stage>;"#,
    );
    let next = doc.remove_keyframe_track("r", "x").unwrap();
    assert!(next.root.find("x-track").is_none());
    assert!(next.root.find("y-track").is_some());
    assert_eq!(next.root.find("r").unwrap().number("x", 0.0).unwrap(), 8.0);
    assert_eq!(
        next.remove_keyframe_track("r", "x").unwrap().source,
        next.source
    );
    assert!(doc.remove_keyframe_track("missing", "x").is_err());
}

#[test]
fn keyframe_toggle_creates_track_and_removes_it_with_last_point() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><text id="r">Keep this text</text></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let added = doc
        .toggle_keyframe_at_playhead(&timeline, "r", "x", json!(12), 1.0)
        .unwrap();
    let node = added.root.find("r").unwrap();
    assert_eq!(node.text, "Keep this text");
    assert_eq!(node.children[0].tag, "keyframeTrack");
    let two = added
        .toggle_keyframe_at_playhead(&timeline, "r", "x", json!(24), 2.0)
        .unwrap();
    assert_eq!(two.root.find("r").unwrap().children[0].children.len(), 2);
    let one = two
        .toggle_keyframe_at_playhead(&timeline, "r", "x", json!(99), 1.0)
        .unwrap();
    assert_eq!(one.root.find("r").unwrap().children[0].children.len(), 1);
    let removed = one
        .toggle_keyframe_at_playhead(&timeline, "r", "x", json!(99), 2.0)
        .unwrap();
    assert!(removed.root.find("r").unwrap().children.is_empty());
    assert_eq!(removed.root.find("r").unwrap().text, "Keep this text");
    let leaf =
        parse(r#"export default <stage><scene id="s" active><rect id="r"/></scene></stage>;"#);
    assert!(
        leaf.toggle_keyframe_at_playhead(&timeline, "r", "x", json!(1), 0.0)
            .unwrap()
            .root
            .find("r")
            .unwrap()
            .children
            .len()
            == 1
    );
    assert!(
        doc.toggle_keyframe_at_playhead(&timeline, "r", "id", json!(1), 0.0)
            .is_err()
    );
}

#[test]
fn nudge_uses_sampled_position_and_keeps_existing_track_in_step() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="r" x={0}><keyframeTrack id="t" property="x"><keyframe time={0} value={10}/><keyframe time={2} value={30}/></keyframeTrack></rect><rect id="b" x={3}/></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let next = doc
        .nudge(
            &timeline,
            &["r".into(), "b".into(), "r".into()],
            1.0,
            [10.0, 0.0],
        )
        .unwrap();
    assert_eq!(next.root.find("r").unwrap().props["x"].as_f64(), Some(30.0));
    assert_eq!(next.root.find("b").unwrap().props["x"].as_f64(), Some(13.0));
    let sampled = ad3_editor::motion::Motion::default()
        .sample(&next.root, &timeline, 1.0, 30.0)
        .unwrap();
    assert_eq!(sampled.find("r").unwrap().number("x", 0.0).unwrap(), 30.0);
    assert_eq!(next.root.find("t").unwrap().children.len(), 3);
}

#[test]
fn property_edit_syncs_keyframe_at_nested_source_clock() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><group id="g" start={1} sourceIn={2} playbackRate={2}><rect id="r" start={3} sourceIn={4} playbackRate={0.5} end={7} x={0}><keyframeTrack id="t" property="x"><keyframe id="p" time={4} value={0}/></keyframeTrack></rect></group></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&doc.root, &BTreeMap::new()).unwrap();
    let at = timeline.get("r").unwrap().start + 0.5;
    let expected = timeline.get("r").unwrap().source_at(at);
    let edited = doc
        .set_at_playhead(&timeline, "r", "x", json!(28), at)
        .unwrap();
    assert_eq!(
        edited.root.find("r").unwrap().props["x"].as_f64(),
        Some(28.0)
    );
    let points = &edited.root.find("t").unwrap().children;
    assert_eq!(points.len(), 2);
    assert_eq!(points[1].props["time"].as_f64(), Some(expected));
    assert_eq!(points[1].props["value"].as_f64(), Some(28.0));
}

#[test]
fn keyframe_sync_updates_same_frame_preserves_easing_and_inserts_when_needed() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="r" x={0}><keyframeTrack id="t" property="x"><keyframe id="p" time={1} value={10} easing="easeInQuad"/></keyframeTrack></rect></scene></stage>;"#,
    );
    let updated = doc.sync_keyframe("r", "x", json!(24), 1.01).unwrap();
    let point = updated.root.find("p").unwrap();
    assert_eq!(point.props["value"].as_f64(), Some(24.0));
    assert_eq!(point.string("easing"), Some("easeInQuad"));
    assert_eq!(updated.root.find("t").unwrap().children.len(), 1);
    let inserted = updated.sync_keyframe("r", "x", json!(40), 1.5).unwrap();
    let track = inserted.root.find("t").unwrap();
    assert_eq!(track.children.len(), 2);
    assert_eq!(track.children[1].props["time"].as_f64(), Some(1.5));
    assert_eq!(track.children[1].props["value"].as_f64(), Some(40.0));
    assert_eq!(
        doc.sync_keyframe("r", "y", json!(20), 1.0).unwrap().source,
        doc.source
    );
    assert!(doc.sync_keyframe("r", "x", json!(0), f64::NAN).is_err());
    let empty = parse(
        r#"export default <stage><scene active><rect id="r"><keyframeTrack id="t" property="x" /></rect></scene></stage>;"#,
    );
    let inserted = empty.sync_keyframe("r", "x", json!(8), 0.0).unwrap();
    assert_eq!(inserted.root.find("t").unwrap().children.len(), 1);
}

#[test]
fn grouping_excludes_masks_and_uses_next_authored_group_name() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="mask" mask/><rect id="a"/><group id="g" name="Group 12"><rect id="b"/></group><group id="h" name="Group 20x"/></scene></stage>;"#,
    );
    let (grouped, id) = doc.group(&["mask".into(), "a".into()]).unwrap();
    let group = grouped.root.find(id.as_ref().unwrap()).unwrap();
    assert_eq!(group.string("name"), Some("Group 13"));
    assert_eq!(group.children.len(), 1);
    assert_eq!(group.children[0].id, "a");
    assert!(
        grouped
            .root
            .find("s")
            .unwrap()
            .children
            .iter()
            .any(|node| node.id == "mask")
    );
    let (untouched, id) = doc.group(&["mask".into()]).unwrap();
    assert!(id.is_none());
    assert_eq!(untouched.source, doc.source);
}

#[test]
fn grouping_uses_first_selected_parent_and_preserves_sibling_order() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><rect id="a"/><rect id="stay"/><rect id="b"/><group id="other"><rect id="c"/></group></scene></stage>;"#,
    );
    let (grouped, id) = doc.group(&["b".into(), "c".into(), "a".into()]).unwrap();
    let group = grouped.root.find(id.as_ref().unwrap()).unwrap();
    assert_eq!(
        group
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        grouped
            .root
            .find("s")
            .unwrap()
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec!["group-1", "stay", "other"]
    );
    assert!(grouped.root.find("other").unwrap().find("c").is_some());
    let (nested, _) = doc.group(&["other".into(), "c".into()]).unwrap();
    assert_eq!(nested.root.find("group-1").unwrap().children.len(), 1);
    assert_eq!(doc.group(&[]).unwrap().0.source, doc.source);
    assert!(doc.group(&["missing".into()]).is_err());
}

#[test]
fn duplicate_subtrees_remaps_internal_links_and_leaves_sequences() {
    let doc = parse(
        r#"export default <stage><scene id="s" active><sequence id="q"><group id="g"><rect id="a"/><rect id="b" syncTo="a"/><rect id="c" syncTo="external"/></group></sequence></scene></stage>;"#,
    );
    let (copy, ids) = doc.duplicate(&["g".into(), "a".into()]).unwrap();
    assert_eq!(ids, vec!["g-2"]);
    let scene = copy.root.find("s").unwrap();
    assert_eq!(
        scene
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec!["q", "g-2"]
    );
    assert_eq!(copy.root.find("b-2").unwrap().string("syncTo"), Some("a-2"));
    assert_eq!(
        copy.root.find("c-2").unwrap().string("syncTo"),
        Some("external")
    );
    assert_eq!(copy.root.find("q").unwrap().children.len(), 1);
    let (again, ids) = copy.duplicate(&["g".into()]).unwrap();
    assert_eq!(ids, vec!["g-3"]);
    assert!(again.root.find("a-3").is_some());
    let (scene_copy, _) = doc.duplicate(&["s".into()]).unwrap();
    assert!(!scene_copy.root.find("s-2").unwrap().flag("active"));
    assert!(scene_copy.root.find("s").unwrap().flag("active"));
}

#[test]
fn restack_preserves_selected_order_across_nested_parents_and_noop_source() {
    let doc = SourceDocument::parse(PathBuf::from("index.tsx"), r#"export default <stage><scene id="s" active><group id="g"><rect id="a"/><rect id="b"/><rect id="c"/></group><rect id="d"/><rect id="e"/></scene></stage>;"#.into()).unwrap();
    let selected = vec!["g".into(), "a".into(), "b".into()];
    let front = doc.restack(&selected, true).unwrap();
    assert_eq!(
        front
            .root
            .find("s")
            .unwrap()
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec!["d", "e", "g"]
    );
    assert_eq!(
        front
            .root
            .find("g")
            .unwrap()
            .children
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        vec!["c", "a", "b"]
    );
    assert_eq!(front.restack(&selected, true).unwrap().source, front.source);
    let back = front.restack(&selected, false).unwrap();
    assert_eq!(back.source, doc.stamp_ids().unwrap().source);
    assert!(doc.restack(&["missing".into()], true).is_err());
}

#[test]
fn ripple_trims_keep_source_alignment_shift_sequence_followers_and_preserve_other_rows() {
    let document = parse(
        r#"// preserve human source
        export default <stage><scene id="main" workarea={[0,10]}>
            <sequence id="track" playbackRate={2}>
                <video id="take" src="take.mp4" start={2} sourceIn={4} sourceOut={16} playbackRate={2}/>
                <rect id="next" start={10} end={14}/>
                <rect id="last" start={16} end={18}/>
            </sequence>
            <text id="overlay" start={5} end={9}>Keep me</text>
        </scene></stage>;"#,
    );
    let durations = BTreeMap::from([("take".into(), 20.0)]);
    let timeline = Timeline::resolve(&document.root, &durations).unwrap();
    assert_eq!(
        (
            timeline.get("take").unwrap().start,
            timeline.get("take").unwrap().end
        ),
        (1.0, 4.0)
    );
    for left in [true, false] {
        let (next, units, playhead) = document
            .ripple_trim(&timeline, "main", &["track".into()], 2.0, left)
            .unwrap();
        assert_eq!(units, vec!["take"]);
        assert_eq!(playhead, if left { 1.0 } else { 2.0 });
        let edited = Timeline::resolve(&next.root, &durations).unwrap();
        let clip = edited.get("take").unwrap();
        assert_eq!(clip.start, 1.0);
        assert_eq!(clip.end, if left { 3.0 } else { 2.0 });
        assert_eq!(clip.source_at(1.0), if left { 8.0 } else { 4.0 });
        let removed = if left { 1.0 } else { 2.0 };
        assert_eq!(edited.get("next").unwrap().start, 5.0 - removed);
        assert_eq!(edited.get("next").unwrap().end, 7.0 - removed);
        assert_eq!(edited.get("last").unwrap().start, 8.0 - removed);
        assert_eq!(edited.get("overlay").unwrap().start, 5.0);
        assert!(next.source.contains("// preserve human source"));
    }
}

#[test]
fn ripple_without_selection_trims_all_visible_rows_but_never_hidden_or_edge_clips() {
    let document = parse(
        r#"export default <stage><scene id="s">
        <sequence id="track"><rect id="a" end={4}/><rect id="b" start={5} end={7}/></sequence>
        <rect id="overlay" start={1} end={5}/><rect id="hidden" hidden end={8}/>
        <rect id="edge" start={2} end={8}/>
        </scene><scene id="other"><rect id="foreign" end={4}/></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let (next, units, time) = document
        .ripple_trim(&timeline, "s", &[], 2.0, true)
        .unwrap();
    assert_eq!(units, vec!["a", "overlay"]);
    assert_eq!(time, 1.0);
    let edited = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    assert_eq!(edited.get("a").unwrap().end, 2.0);
    assert_eq!(edited.get("b").unwrap().start, 3.0);
    assert_eq!(edited.get("overlay").unwrap().end, 4.0);
    assert_eq!(
        next.root.find("hidden").unwrap().props,
        document.root.find("hidden").unwrap().props
    );
    assert_eq!(
        next.root.find("edge").unwrap().props,
        document.root.find("edge").unwrap().props
    );
    assert!(
        document
            .ripple_trim(&timeline, "s", &["foreign".into()], 2.0, true)
            .is_err()
    );
    assert!(
        document
            .ripple_trim(&timeline, "s", &[], f64::NAN, true)
            .is_err()
    );
    let (no_op, units, _) = document
        .ripple_trim(&timeline, "s", &["a".into()], 0.0, true)
        .unwrap();
    assert!(units.is_empty());
    assert_eq!(no_op.source, document.source);
}

#[test]
fn ripple_multi_selection_moves_each_follower_container_once() {
    let document = parse(
        r#"export default <stage><scene id="s"><sequence id="track">
        <rect id="a" end={4}/><rect id="overlap" start={1} end={6}/>
        <group id="follower" start={7}><rect id="inside" start={1} end={3}/></group>
        </sequence></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let (next, units, _) = document
        .ripple_trim(
            &timeline,
            "s",
            &["a".into(), "a".into(), "overlap".into()],
            2.0,
            true,
        )
        .unwrap();
    assert_eq!(units, vec!["a", "overlap"]);
    let edited = Timeline::resolve(&next.root, &BTreeMap::new()).unwrap();
    assert_eq!(
        edited.get("follower").unwrap().start,
        timeline.get("follower").unwrap().start - 2.0
    );
    assert_eq!(edited.get("inside").unwrap().start, 6.0);
    assert_eq!(
        next.root.find("inside").unwrap().props,
        document.root.find("inside").unwrap().props
    );
    assert!(
        document
            .ripple_trim(&timeline, "s", &[], f64::MAX, true)
            .is_err()
    );
}

#[test]
fn multi_clip_moves_use_final_sequence_positions_and_keep_nested_offsets() {
    let document = parse(
        r#"export default <stage><scene id="s">
        <sequence id="track" playbackRate={2}><rect id="a" end={2}/><rect id="b" start={2} end={4}/><rect id="obstacle" start={8} end={10}/></sequence>
        <group id="group" start={3}><rect id="inside" start={1} end={2}/></group>
        </scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let moved = document
        .move_clips(
            &timeline,
            &["a".into(), "b".into(), "group".into(), "inside".into()],
            1.0,
        )
        .unwrap();
    let edited = Timeline::resolve(&moved.root, &BTreeMap::new()).unwrap();
    for id in ["a", "b", "group", "inside"] {
        assert_eq!(
            edited.get(id).unwrap().start,
            timeline.get(id).unwrap().start + 1.0
        );
        assert_eq!(
            edited.get(id).unwrap().end,
            timeline.get(id).unwrap().end + 1.0
        );
    }
    assert_eq!(
        moved.root.find("inside").unwrap().props,
        document.root.find("inside").unwrap().props
    );
    assert!(
        document
            .move_clips(&timeline, &["a".into(), "b".into()], 3.0)
            .is_err()
    );
    assert!(document.move_clips(&timeline, &["s".into()], 1.0).is_err());
    let clamped = moved
        .move_clips(&edited, &["a".into(), "b".into()], -10.0)
        .unwrap();
    let clamped = Timeline::resolve(&clamped.root, &BTreeMap::new()).unwrap();
    assert_eq!(clamped.get("a").unwrap().start, 0.0);
    assert_eq!(clamped.get("b").unwrap().start, 1.0);
}

#[test]
fn playhead_cut_opens_sequences_and_cuts_visible_rows_atomically() {
    let document = parse(
        r#"// keep source
        export default <stage><scene id="s"><sequence id="track" playbackRate={2}>
        <video id="take" src="take.mp4" sourceIn={2} sourceOut={10}/><rect id="later" start={12} end={14}/>
        </sequence><text id="title" start={1} end={5}>Hello</text><rect id="hidden" hidden end={8}/><rect id="edge" start={2} end={4}/>
        </scene></stage>;"#,
    );
    let durations = BTreeMap::from([("take".into(), 20.0), ("take-2".into(), 20.0)]);
    let timeline = Timeline::resolve(&document.root, &durations).unwrap();
    let (next, units) = document
        .split_at_playhead(&timeline, "s", &[], 2.0)
        .unwrap();
    assert_eq!(units, vec!["take", "title"]);
    let edited = Timeline::resolve(&next.root, &durations).unwrap();
    for id in ["take", "title"] {
        let right = format!("{id}-2");
        assert_eq!(edited.get(id).unwrap().end, 2.0);
        assert_eq!(edited.get(&right).unwrap().start, 2.0);
        assert_eq!(
            edited.get(&right).unwrap().end,
            timeline.get(id).unwrap().end
        );
        assert_eq!(
            edited.get(&right).unwrap().source_at(2.0),
            timeline.get(id).unwrap().source_at(2.0)
        );
    }
    assert!(next.root.find("hidden-2").is_none());
    assert!(next.root.find("edge-2").is_none());
    assert!(next.source.contains("// keep source"));
    let (selected, units) = document
        .split_at_playhead(&timeline, "s", &["track".into()], 2.0)
        .unwrap();
    assert_eq!(units, vec!["take"]);
    assert!(selected.root.find("title-2").is_none());
    let (no_op, units) = document
        .split_at_playhead(&timeline, "s", &["edge".into()], 2.0)
        .unwrap();
    assert!(units.is_empty());
    assert_eq!(no_op.source, document.source);
}

#[test]
fn playhead_cut_deduplicates_selected_groups_and_their_children() {
    let document = parse(
        r#"export default <stage><scene id="s"><group id="g"><rect id="r" end={4}/></group></scene><scene id="other"><rect id="foreign" end={4}/></scene></stage>;"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let (next, units) = document
        .split_at_playhead(&timeline, "s", &["r".into(), "g".into(), "g".into()], 2.0)
        .unwrap();
    assert_eq!(units, vec!["g"]);
    assert!(next.root.find("g-2").unwrap().find("r-2").is_some());
    assert!(next.root.find("r-3").is_none());
    assert!(
        document
            .split_at_playhead(&timeline, "s", &["foreign".into()], 2.0)
            .is_err()
    );
    assert!(
        document
            .split_at_playhead(&timeline, "s", &[], f64::MAX)
            .is_err()
    );
}

#[test]
fn close_gap_uses_occupied_union_and_ripples_later_scene_rows_once() {
    let document = parse(
        r#"export default <stage><scene id="s">
        <sequence id="track" playbackRate={2}><rect id="a" end={4}/><rect id="overlap" start={2} end={6}/><rect id="b" start={10} end={12}/></sequence>
        <group id="group" start={6}><rect id="inside" start={1} end={3}/></group>
        <audio id="sound" start={5} end={9}/><text id="crossing" start={2} end={7}>Keep me</text>
        <sequence id="secondary"><rect id="cross-row" end={7}/><rect id="later-row" start={8} end={10}/></sequence>
        </scene><scene id="other"><rect id="foreign" start={5} end={8}/></scene></stage>;"#,
    );
    let durations = BTreeMap::from([("sound".into(), 20.0)]);
    let timeline = Timeline::resolve(&document.root, &durations).unwrap();
    assert_eq!(
        document.sequence_gaps(&timeline, "track").unwrap(),
        vec![[3.0, 5.0]]
    );
    let next = document
        .close_gap(&timeline, "s", "track", [3.0, 5.0])
        .unwrap();
    let edited = Timeline::resolve(&next.root, &durations).unwrap();
    assert_eq!(edited.get("b").unwrap().start, 3.0);
    for id in ["group", "inside", "sound", "later-row"] {
        assert_eq!(
            edited.get(id).unwrap().start,
            timeline.get(id).unwrap().start - 2.0
        );
    }
    for id in ["a", "overlap", "crossing", "foreign", "cross-row"] {
        assert_eq!(
            edited.get(id).unwrap().start,
            timeline.get(id).unwrap().start
        );
    }
    assert_eq!(
        next.root.find("inside").unwrap().props,
        document.root.find("inside").unwrap().props
    );
    assert!(next.sequence_gaps(&edited, "track").unwrap().is_empty());
    assert!(next.close_gap(&edited, "s", "track", [3.0, 5.0]).is_err());
    assert!(
        document
            .close_gap(&timeline, "other", "track", [3.0, 5.0])
            .is_err()
    );
    assert!(
        document
            .close_gap(&timeline, "s", "track", [0.0, 1.0])
            .is_err()
    );
}

#[test]
fn source_roundtrip_split_rate_workarea_and_caps() {
    let document = parse(PROJECT);
    let mut durations = BTreeMap::from([("take".into(), 12.0)]);
    let timeline = Timeline::resolve(&document.root, &durations).unwrap();
    let clip = timeline.get("take").unwrap();
    assert_eq!((clip.start, clip.end, clip.origin), (1.0, 6.0, 0.0));
    assert_eq!(timeline.workareas["main"], [1.0, 7.0]);
    let split = document.split(&timeline, "take", 3.0).unwrap();
    durations.insert("take-2".into(), 12.0);
    let timeline = Timeline::resolve(&split.root, &durations).unwrap();
    assert_eq!(
        (
            timeline.get("take").unwrap().end,
            timeline.get("take-2").unwrap().start
        ),
        (3.0, 3.0)
    );
    assert_eq!(timeline.get("take-2").unwrap().source_at(3.0), 6.0);
    assert_eq!(timeline.get("take-2").unwrap().end, 6.0);
    assert!(
        split
            .source
            .contains("// edição humana — preserve este comentário")
    );
    assert!(
        split
            .source
            .contains("<text id=\"title\" start={2} end={4} fill=\"#fff\">Ação &amp; café</text>")
    );
    assert_eq!(split.root.find("title").unwrap().text, "Ação & café");
    let trimmed = split.trim(&timeline, "take-2", 4.0, 5.0).unwrap();
    let trim_timeline = Timeline::resolve(&trimmed.root, &durations).unwrap();
    let clip = trim_timeline.get("take-2").unwrap();
    assert_eq!(
        (clip.start, clip.end, clip.source_in, clip.source_out),
        (4.0, 5.0, 8.0, 10.0)
    );
    let changed = trimmed
        .set("title", "name", Some(json!("aspas \" e < > { } café")))
        .unwrap();
    assert_eq!(
        changed.root.find("title").unwrap().string("name"),
        Some("aspas \" e < > { } café")
    );
}

#[test]
fn parent_rates_and_trims_gate_children_without_changing_origin() {
    let document = parse(
        r#"export default () => <stage><scene id="s" active width={100} height={100}>
      <group id="g" start={2} sourceIn={4} sourceOut={10} playbackRate={2} volume={-6}>
        <rect id="r" start={4} end={12} volume={-3}/>
      </group>
    </scene></stage>"#,
    );
    let timeline = Timeline::resolve(&document.root, &BTreeMap::new()).unwrap();
    let span = timeline.get("r").unwrap();
    assert_eq!(
        (
            span.start,
            span.end,
            span.playback_rate,
            span.source_in,
            span.source_out,
            span.volume_db
        ),
        (2.0, 5.0, 2.0, 0.0, 6.0, -9.0)
    );
}

#[test]
fn dynamic_projects_fail_without_producing_a_partial_document() {
    for source in [
        "export default () => <stage>{clips.map(c => <video src={c.src}/>)}</stage>",
        "export default function P(){ if (true) return <stage/>; return <stage/>; }",
        "import './side-effect'; export default () => <stage/>;",
        "export default () => <stage><scene id='same'/><scene id='same'/></stage>",
    ] {
        assert!(
            SourceDocument::parse("index.tsx".into(), source.into()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn atomic_save_undo_redo_and_external_edit_conflict() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let folder =
        std::env::temp_dir().join(format!("ad3-rust-editing-{}-{unique}", std::process::id()));
    fs::create_dir(&folder).unwrap();
    let path = folder.join("index.tsx");
    fs::write(&path, PROJECT).unwrap();
    let mut history = History::new(SourceDocument::open(&folder).unwrap());
    history
        .commit(
            history
                .document
                .set("title", "x", Some(json!(100)))
                .unwrap(),
        )
        .unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("x={100}"));
    let before = history.document.source.clone();
    assert!(
        history
            .restore::<()>(false, |_| Err("Original media is now missing".into()))
            .is_err()
    );
    assert_eq!(history.document.source, before);
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    history.undo().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), PROJECT);
    history.redo().unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("x={100}"));
    fs::write(&path, "// changed by a human").unwrap();
    assert!(history.undo().unwrap_err().contains("changed on disk"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "// changed by a human");
    // Only remove the unique test directory created above.
    let absolute = fs::canonicalize(&folder).unwrap();
    assert!(absolute.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
    assert!(
        absolute
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ad3-rust-editing-")
    );
    fs::remove_dir_all(absolute).unwrap();
}

#[test]
fn static_text_uses_javascript_primitive_and_array_spelling() {
    let document = parse(
        r#"export default () => <stage><scene id="s"><text id="t">{3}{[1,2] + ""}{({a: 1}) + ""}{-0}{1e21}{[1] + 2}{[] + []}{({a: 1}) + 2}</text></scene></stage>"#,
    );
    assert_eq!(
        document.root.find("t").unwrap().text,
        "31,2[object Object]01e+2112[object Object]2"
    );
}

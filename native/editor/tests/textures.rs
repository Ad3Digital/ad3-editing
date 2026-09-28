// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
use ad3_editor::{
    media::{MediaLibrary, Probe, ProbeCache, Tools},
    project::ProjectRenderer,
    source::SourceDocument,
    texture::{Fit, TextureFrame, TextureRequest, TextureStore, source_time},
};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ad3-textures-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn png(&self, name: &str, width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 4]) {
        image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(color(x, y)))
            .save(self.0.join(name))
            .unwrap();
    }
    fn document(&self, body: &str) -> SourceDocument {
        SourceDocument::parse(self.0.join("index.tsx"),format!("export default <stage><scene id=\"s\" active width={{64}} height={{48}} workarea={{[0,2]}}>{body}</scene></stage>")).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let path = fs::canonicalize(&self.0).unwrap();
        assert!(path.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ad3-textures-")
        );
        fs::remove_dir_all(path).unwrap();
    }
}
fn no_codecs() -> Tools {
    Tools {
        ffmpeg: "absent-texture-ffmpeg".into(),
        ffprobe: "absent-texture-ffprobe".into(),
    }
}
fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 4] {
    bytes[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4]
        .try_into()
        .unwrap()
}
fn frame(project: &Project, body: &str) -> Vec<u8> {
    let tools = no_codecs();
    let doc = project.document(body);
    let mut renderer = ProjectRenderer::new(&doc, &project.0, tools.clone(), true).unwrap();
    let plan = renderer.frame_plan("s", 0.0, 30.0, false).unwrap();
    assert!(plan.is_native());
    plan.frame(&tools, &project.0).unwrap()
}

#[test]
fn image_masks_intersect_media_geometry_and_parent_clips() {
    let p = Project::new();
    p.png("mask.png", 8, 4, |_, _| [255, 0, 0, 255]);
    let bytes = frame(
        &p,
        r#"<rect x={8} y={4} width={48} height={36}>
      <rect mask x={16} width={32} height={36}/>
      <image src="mask.png" width={48} height={36} objectFit="fill">
        <rect mask x={12} y={8} width={20} height={20}/>
      </image></rect>"#,
    );
    assert_eq!(pixel(&bytes, 22, 15), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 25, 15), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 40, 15), [0, 0, 0, 0]);
    assert_eq!(pixel(&bytes, 25, 32), [0, 0, 0, 0]);
}
#[test]
fn cancellation_reaches_image_and_video_probing_before_io() {
    let p = Project::new();
    p.png("image.png", 8, 4, |_, _| [255, 0, 0, 255]);
    let token = AtomicBool::new(true);
    let tools = no_codecs();
    let error = tools
        .probe_cancellable(&p.0.join("missing.mp4"), Some(&token))
        .unwrap_err();
    assert!(error.contains("cancelled"));
    let doc = p.document(r#"<image src="image.png"/>"#);
    let error = MediaLibrary::load_cached_cancellable(
        &doc.root,
        &p.0,
        &tools,
        &mut ProbeCache::default(),
        Some(&token),
    )
    .unwrap_err();
    assert!(error.contains("cancelled"));
}

#[test]
fn wide_images_respect_total_pixel_limit_and_unknown_images_never_use_video_decoder() {
    let p = Project::new();
    p.png("wide.png", 20_000, 4, |_, _| [255, 0, 0, 255]);
    let bytes = frame(
        &p,
        r#"<image src="wide.png" width={16} height={16} objectFit="fill"/>"#,
    );
    assert_eq!(pixel(&bytes, 8, 8), [255, 0, 0, 255]);
    for name in ["unknown.bin", "vector.svg"] {
        fs::write(
            p.0.join(name),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"/>",
        )
        .unwrap();
        let store = TextureStore::new(no_codecs());
        let request = TextureRequest {
            path: p.0.join(name),
            probe: Probe::default(),
            frame: TextureFrame::Image,
        };
        assert_eq!(
            store.frame(&request, None).err().unwrap(),
            "Image format is not ported to the Rust decoder yet"
        );
    }
}

#[test]
fn rust_decodes_and_fits_images_without_ffmpeg_or_ffprobe() {
    let p = Project::new();
    p.png("wide.png", 8, 4, |x, _| {
        if x < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let contain = frame(
        &p,
        r#"<image src="wide.png" width={16} height={16} objectFit="contain"/>"#,
    );
    assert_eq!(pixel(&contain, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel(&contain, 2, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(&contain, 13, 8), [0, 0, 255, 255]);
    let cover = frame(&p, r#"<image src="wide.png" width={16} height={16}/>"#);
    assert_eq!(pixel(&cover, 2, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&cover, 13, 2), [0, 0, 255, 255]);
    let fill = frame(
        &p,
        r#"<image src="wide.png" width={16} height={16} objectFit="fill"/>"#,
    );
    assert_eq!(pixel(&fill, 2, 2), [255, 0, 0, 255]);
    assert_eq!(pixel(&fill, 13, 13), [0, 0, 255, 255]);
    assert_eq!(
        cover,
        frame(
            &p,
            r#"<image src="wide.png" width={16} height={16} objectFit="unknown"/>"#
        )
    );
}

#[test]
fn media_transform_rounding_and_opacity_share_native_vector_composition() {
    let p = Project::new();
    p.png("solid.png", 8, 4, |_, _| [255, 0, 0, 128]);
    let bytes = frame(
        &p,
        r#"<rect width={64} height={48} fill="blue"/>
      <group opacity={0.5}><image src="solid.png" x={16} y={12} width={16} height={8} rotation={90} cornerRadius={2} objectFit="fill"/></group>"#,
    );
    let center = pixel(&bytes, 24, 16);
    assert!(
        (62..=65).contains(&center[0]) && (190..=193).contains(&center[2]) && center[3] == 255,
        "{center:?}"
    );
    assert_eq!(pixel(&bytes, 16, 12), [0, 0, 255, 255]);
    let corner = pixel(&bytes, 20, 8);
    assert!(
        corner[0] < 32 && corner[2] > 220,
        "Rounded corner has partial pixel coverage: {corner:?}"
    );
    let round = frame(
        &p,
        r#"<image src="solid.png" width={16} height={16} cornerRadius={8} objectFit="fill"/>"#,
    );
    assert_eq!(pixel(&round, 0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel(&round, 8, 8), [255, 0, 0, 128]);
}

#[test]
fn negative_media_destination_size_expands_backwards_without_flipping_pixels() {
    let p = Project::new();
    p.png("stripe.png", 8, 4, |x, _| {
        if x < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let bytes = frame(
        &p,
        r#"<image src="stripe.png" x={24} y={16} width={-16} height={-8} objectFit="fill"/>"#,
    );
    assert_eq!(pixel(&bytes, 10, 10), [255, 0, 0, 255]);
    assert_eq!(pixel(&bytes, 21, 10), [0, 0, 255, 255]);
    assert_eq!(pixel(&bytes, 25, 17), [0, 0, 0, 0]);
}

#[test]
fn decoded_textures_share_pixels_invalidate_on_file_change_and_honor_cancellation() {
    let p = Project::new();
    p.png("cached.png", 8, 4, |_, _| [255, 0, 0, 128]);
    let request = TextureRequest {
        path: p.0.join("cached.png"),
        probe: Probe {
            width: 8,
            height: 4,
            has_video: true,
            still: true,
            ..Default::default()
        },
        frame: TextureFrame::Image,
    };
    let store = TextureStore::new(no_codecs());
    let a = store.frame(&request, None).unwrap();
    let b = store.frame(&request, None).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(store.cached_bytes(), 128);
    assert!(
        store
            .frame(&request, Some(&AtomicBool::new(true)))
            .err()
            .unwrap()
            .contains("cancelled")
    );
    p.png("cached.png", 12, 4, |_, _| [0, 0, 255, 255]);
    let changed = store.frame(&request, None).unwrap();
    assert!(!Arc::ptr_eq(&a, &changed));
    assert_eq!(changed.width(), 12);
    assert_eq!(store.cached_bytes(), 192);
    assert_eq!(&changed.data()[..4], &[0, 0, 255, 255]);
    assert_eq!(
        &a.data()[..4],
        &[128, 0, 0, 128],
        "Already issued frames remain immutable"
    );
}

#[test]
fn native_jpeg_probe_and_pixels_apply_exif_orientation() {
    let p = Project::new();
    let rgb = image::RgbImage::from_fn(12, 8, |x, _| {
        image::Rgb(if x < 6 { [255, 0, 0] } else { [0, 0, 255] })
    });
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
        .encode_image(&rgb)
        .unwrap();
    // APP1: Exif little-endian TIFF, one orientation tag (6: rotate90 CW).
    let mut exif=b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00".to_vec();
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    bytes.append(&mut exif);
    bytes.extend_from_slice(&jpeg[2..]);
    fs::write(p.0.join("rotate.jpg"), bytes).unwrap();
    let doc = p
        .document(r#"<image id="photo" src="rotate.jpg" width={8} height={12} objectFit="fill"/>"#);
    let tools = no_codecs();
    let mut renderer = ProjectRenderer::new(&doc, &p.0, tools.clone(), true).unwrap();
    let probe = &renderer.library.probes["photo"];
    assert_eq!((probe.width, probe.height), (8, 12));
    let bytes = renderer
        .frame_plan("s", 0.0, 30.0, false)
        .unwrap()
        .frame(&tools, &p.0)
        .unwrap();
    assert!(pixel(&bytes, 4, 2)[0] > 230 && pixel(&bytes, 4, 9)[2] > 230);
}

#[test]
fn media_source_clock_preserves_composition_then_source_rounding_and_track_origin() {
    let probe = Probe {
        frame_rate: 24.0,
        duration: Some(2.0),
        start_time: 3.0,
        ..Default::default()
    };
    assert_eq!(
        source_time(0.15, 0.0, 1.0, 30.0, &probe).unwrap(),
        3.0 + 4.0 / 24.0
    );
    assert_eq!(source_time(1.0, 0.5, 2.0, 30.0, &probe).unwrap(), 4.0);
    assert_eq!(source_time(-1.0, 0.0, 1.0, 30.0, &probe).unwrap(), 3.0);
    assert_eq!(
        source_time(20.0, 0.0, 1.0, 30.0, &probe).unwrap(),
        3.0 + 47.0 / 24.0
    );
    assert!(source_time(0.0, 0.0, 1.0, 0.0, &probe).is_err());
}

#[test]
fn fitting_matches_original_render_media_function() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/media-fit.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let array = |key: &str| {
            case[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_f64().unwrap())
                .collect::<Vec<_>>()
        };
        let source = array("source");
        let target = array("target");
        let expected = array("result");
        let actual = Fit::from_prop(case["mode"].as_str())
            .rectangle([source[0], source[1]], [target[0], target[1]]);
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-9, "{case}");
        }
    }
}

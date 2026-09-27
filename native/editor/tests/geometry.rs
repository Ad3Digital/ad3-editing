use ad3_editor::geometry::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransformCase {
    input: TransformValues,
    origin: [f64; 2],
    camera: Mat2D,
    resolution: f64,
    parent: Option<Mat2D>,
    surface: Option<[f64; 2]>,
    parent_culled: Option<bool>,
    local: Value,
    world: Value,
    bounds: Value,
    quad: Value,
    inverse: Value,
    decomposed: Value,
    view: Value,
    camera_inverse: Value,
    culled: bool,
    points: Vec<Pointer>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pointer {
    point: Point,
    inside: bool,
    in_quad: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupCase {
    children: Vec<GroupChild>,
    previous_origin: [f64; 2],
    frame: Value,
}
#[derive(Deserialize)]
struct Pair {
    a: Quad,
    b: Quad,
    intersects: bool,
    contains: bool,
}
#[derive(Deserialize)]
struct Fixture {
    cases: Vec<TransformCase>,
    groups: Vec<GroupCase>,
    pairs: Vec<Pair>,
}

fn compare(actual: Value, expected: &Value, path: &str) {
    match (&actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(
                (a - b).abs() <= 2e-11 * a.abs().max(b.abs()).max(1.0),
                "{path}: {a} != {b}"
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                compare(a.clone(), b, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (key, b) in b {
                compare(a[key].clone(), b, &format!("{path}.{key}"));
            }
        }
        _ => assert_eq!(&actual, expected, "{path}"),
    }
}
fn ieee(value: f64) -> Value {
    if value.is_nan() {
        Value::String("NaN".into())
    } else if value == f64::INFINITY {
        Value::String("Infinity".into())
    } else if value == f64::NEG_INFINITY {
        Value::String("-Infinity".into())
    } else {
        serde_json::json!(value)
    }
}
fn matrix_value(m: Mat2D) -> Value {
    serde_json::json!({"a":ieee(m.a),"b":ieee(m.b),"c":ieee(m.c),"d":ieee(m.d),"e":ieee(m.e),"f":ieee(m.f)})
}

#[test]
fn native_geometry_matches_original_transform_and_selection_functions() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/geometry.json")).unwrap();
    assert_eq!(fixture.cases.len(), 180);
    for (i, c) in fixture.cases.iter().enumerate() {
        let local = c.input.local_matrix();
        let view = c.camera.view(c.resolution);
        let world = c.parent.unwrap_or(view).multiply(local);
        let frame = Frame {
            origin: c.origin,
            size: c.input.size,
        };
        let bounds = frame.bounds(world);
        let quad = frame.quad(world);
        for (name, actual, expected) in [
            ("local", matrix_value(local), &c.local),
            ("world", matrix_value(world), &c.world),
            ("inverse", matrix_value(world.inverse()), &c.inverse),
            ("view", matrix_value(view), &c.view),
            (
                "cameraInverse",
                serde_json::to_value(c.camera.camera_inverse()).unwrap(),
                &c.camera_inverse,
            ),
            ("bounds", serde_json::to_value(bounds).unwrap(), &c.bounds),
            ("quad", serde_json::to_value(quad).unwrap(), &c.quad),
            (
                "decomposed",
                serde_json::to_value(world.decompose()).unwrap(),
                &c.decomposed,
            ),
        ] {
            compare(actual, expected, &format!("case {i}.{name}"));
        }
        assert_eq!(
            is_culled(bounds, c.parent_culled, c.surface),
            c.culled,
            "case {i}.culled"
        );
        for p in &c.points {
            assert_eq!(
                frame.contains_pointer(world, bounds, p.point),
                p.inside,
                "case {i}.pointer {:?}",
                p.point
            );
            assert_eq!(
                point_in_quad(p.point, quad),
                p.in_quad,
                "case {i}.raycast {:?}",
                p.point
            );
        }
    }
    assert_eq!(fixture.groups.len(), 45);
    for (i, g) in fixture.groups.iter().enumerate() {
        compare(
            serde_json::to_value(group_frame(&g.children, g.previous_origin)).unwrap(),
            &g.frame,
            &format!("group {i}"),
        );
    }
    for p in &fixture.pairs {
        assert_eq!(quads_intersect(p.a, p.b), p.intersects);
        assert_eq!(quad_contains_quad(p.a, p.b), p.contains);
    }
}

#[test]
fn native_transform_edge_cases_preserve_runtime_contract() {
    let absent = TransformValues {
        size: [20.0, 10.0],
        rotation: 180.0,
        ..Default::default()
    }
    .local_matrix();
    let zero = TransformValues {
        size: [20.0, 10.0],
        rotation: 180.0,
        anchor: [Some(0.0); 2],
        ..Default::default()
    }
    .local_matrix();
    assert!((absent.e - 20.0).abs() < 1e-12 && (absent.f - 10.0).abs() < 1e-12);
    assert_eq!((zero.e, zero.f), (0.0, 0.0));
    let simultaneous = Mat2D::skew(30.0, 20.0);
    let sequential = Mat2D::skew(30.0, 0.0).multiply(Mat2D::skew(0.0, 20.0));
    assert_eq!(simultaneous.a, 1.0);
    assert!((simultaneous.a - sequential.a).abs() > 0.2);
    let frame = Frame {
        origin: [0.0; 2],
        size: [20.0, 10.0],
    };
    let singular = Mat2D::scale(0.0, 1.0);
    assert!(!frame.contains_pointer(singular, frame.bounds(singular), Point { x: 0.0, y: 5.0 }));
    assert!(singular.camera_inverse().is_none());
    assert!(Mat2D::scale(1e-12, 1.0).camera_inverse().is_some());
    assert!(Mat2D::scale(-1e-13, 1.0).camera_inverse().is_none());
    let at_edge = Aabb {
        min_x: 100.0,
        min_y: 0.0,
        max_x: 110.0,
        max_y: 10.0,
    };
    assert!(!is_culled(at_edge, None, Some([100.0, 100.0])));
    assert!(is_culled(at_edge, Some(true), None));
    assert!(!is_culled(at_edge, Some(false), Some([1.0, 1.0])));
    assert!(!is_culled(at_edge, None, None));
    // Invalid inverse/bounds stay IEEE values, not plausible identity/empty boxes.
    let invalid = Mat2D {
        a: f64::NAN,
        ..Default::default()
    }
    .bounds(10.0, 10.0);
    assert!(invalid.min_x.is_nan() && invalid.max_x.is_nan());
    let children = [GroupChild {
        local: Mat2D::translate(-20.0, 30.0),
        size: [10.0, 15.0],
        hidden: false,
        mask: false,
    }];
    assert_eq!(
        group_frame(&children, [0.0; 2]),
        Frame {
            origin: [-20.0, 30.0],
            size: [10.0, 15.0]
        }
    );
    assert_eq!(
        group_frame(&[], [-20.0, 30.0]),
        Frame {
            origin: [-20.0, 30.0],
            size: [0.0; 2]
        }
    );
    let q = frame.quad(Mat2D::IDENTITY);
    assert_eq!(quad_center(q), Point { x: 10.0, y: 5.0 });
    // The pointer box includes its far edge; the polygon raycast is half-open.
    // Integral geometry avoids platform libm roundoff at random rotated edges.
    for point in q {
        assert!(frame.contains_pointer(Mat2D::IDENTITY, frame.bounds(Mat2D::IDENTITY), point));
    }
    assert!(point_in_quad(Point { x: 0.0, y: 0.0 }, q));
    assert!(!point_in_quad(Point { x: 20.0, y: 10.0 }, q));
    assert!(point_in_quad(
        Point {
            x: 20.0 - 1e-8,
            y: 5.0
        },
        q
    ));
    assert!(!point_in_quad(
        Point {
            x: 20.0 + 1e-8,
            y: 5.0
        },
        q
    ));
}

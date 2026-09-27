// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Native geometry kernels from runtime/math, systems/transform and
//! queries/interaction. Inputs are resolved runtime values, not JSX properties.
//! Keep their arithmetic/order separate from the eventual scene traversal.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat2D {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Mat2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat2D {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Parent * local, using Canvas/DOMMatrix coefficient order.
    pub fn multiply(self, q: Self) -> Self {
        Self {
            a: self.a * q.a + self.c * q.b,
            b: self.b * q.a + self.d * q.b,
            c: self.a * q.c + self.c * q.d,
            d: self.b * q.c + self.d * q.d,
            e: self.e + self.a * q.e + self.c * q.f,
            f: self.f + self.b * q.e + self.d * q.f,
        }
    }
    pub fn translate(x: f64, y: f64) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }
    pub fn scale(x: f64, y: f64) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }
    pub fn rotate(degrees: f64) -> Self {
        let radians = degrees * (std::f64::consts::PI / 180.0);
        let cos = radians.cos();
        let sin = radians.sin();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }
    /// The runtime uses simultaneous shear, not SkewX * SkewY.
    pub fn skew(x_degrees: f64, y_degrees: f64) -> Self {
        let radians = std::f64::consts::PI / 180.0;
        Self {
            b: (y_degrees * radians).tan(),
            c: (x_degrees * radians).tan(),
            ..Self::IDENTITY
        }
    }
    pub fn point(self, x: f64, y: f64) -> Point {
        Point {
            x: self.a * x + self.c * y + self.e,
            y: self.b * x + self.d * y + self.f,
        }
    }
    pub fn determinant(self) -> f64 {
        self.a * self.d - self.b * self.c
    }
    /// Like invert2D, a singular matrix produces IEEE non-finite values.
    /// Pointer containment consequently returns false. Camera inversion has
    /// its own threshold and should use camera_inverse instead.
    pub fn inverse(self) -> Self {
        let inv = 1.0 / self.determinant();
        Self {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        }
    }
    pub fn camera_inverse(self) -> Option<Self> {
        if self.determinant().abs() < 1e-12 {
            None
        } else {
            Some(self.inverse())
        }
    }
    /// Camera maps document to CSS pixels; resolution maps CSS to device pixels.
    pub fn view(self, resolution: f64) -> Self {
        Self {
            a: self.a * resolution,
            b: self.b * resolution,
            c: self.c * resolution,
            d: self.d * resolution,
            e: self.e * resolution,
            f: self.f * resolution,
        }
    }
    pub fn quad(self, width: f64, height: f64) -> Quad {
        // Match rectToQuad exactly, including the reduced corner expressions.
        [
            Point {
                x: self.e,
                y: self.f,
            },
            Point {
                x: self.a * width + self.e,
                y: self.b * width + self.f,
            },
            self.point(width, height),
            Point {
                x: self.c * height + self.e,
                y: self.d * height + self.f,
            },
        ]
    }
    pub fn bounds(self, width: f64, height: f64) -> Aabb {
        let q = self.quad(width, height);
        Aabb {
            min_x: js_min(q.map(|p| p.x)),
            min_y: js_min(q.map(|p| p.y)),
            max_x: js_max(q.map(|p| p.x)),
            max_y: js_max(q.map(|p| p.y)),
        }
    }
    pub fn decompose(self) -> Decomposed {
        let scale_x = self.a.hypot(self.b);
        if scale_x < 1e-12 {
            return Decomposed {
                x: self.e,
                y: self.f,
                scale_y: self.c.hypot(self.d),
                ..Default::default()
            };
        }
        let r0x = self.a / scale_x;
        let r0y = self.b / scale_x;
        let mut shear = r0x * self.c + r0y * self.d;
        let c2 = self.c - r0x * shear;
        let d2 = self.d - r0y * shear;
        let mut scale_y = c2.hypot(d2);
        let rotation = r0y.atan2(r0x) * 180.0 / std::f64::consts::PI;
        if scale_y < 1e-12 {
            return Decomposed {
                x: self.e,
                y: self.f,
                rotation,
                scale_x,
                ..Default::default()
            };
        }
        shear /= scale_y;
        if self.determinant() < 0.0 {
            scale_y = -scale_y;
            shear = -shear;
        }
        Decomposed {
            x: self.e,
            y: self.f,
            rotation,
            scale_x,
            scale_y,
            skew_x: shear.atan() * 180.0 / std::f64::consts::PI,
            skew_y: 0.0,
        }
    }
}

// Rust min/max ignore NaN; JavaScript propagates it, with defined signed-zero
// handling. Retain that behavior for degenerate/infinite geometry calculations.
fn js_min(values: [f64; 4]) -> f64 {
    values.into_iter().fold(f64::INFINITY, |a, b| {
        if a.is_nan() || b.is_nan() {
            f64::NAN
        } else if b < a || (a == b && b.is_sign_negative()) {
            b
        } else {
            a
        }
    })
}
fn js_max(values: [f64; 4]) -> f64 {
    values.into_iter().fold(f64::NEG_INFINITY, |a, b| {
        if a.is_nan() || b.is_nan() {
            f64::NAN
        } else if b > a || (a == b && b.is_sign_positive()) {
            b
        } else {
            a
        }
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
pub type Quad = [Point; 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aabb {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}
impl Aabb {
    /// Touching edges count as overlap in the original culling system.
    pub fn intersects(self, other: Self) -> bool {
        !(self.max_x < other.min_x
            || self.min_x > other.max_x
            || self.max_y < other.min_y
            || self.min_y > other.max_y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decomposed {
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub skew_x: f64,
    pub skew_y: f64,
}

/// Fully resolved values consumed by computeLocalMatrix. Missing Anchor store
/// entries use .5; an explicitly present Anchor trait can contain zero instead.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TransformValues {
    pub position: [f64; 2],
    pub offset: [f64; 2],
    pub size: [f64; 2],
    pub anchor: [Option<f64>; 2],
    pub flip: [Option<f64>; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
    pub skew: [f64; 2],
}
impl Default for TransformValues {
    fn default() -> Self {
        Self {
            position: [0.0; 2],
            offset: [0.0; 2],
            size: [0.0; 2],
            anchor: [None; 2],
            flip: [None; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            skew: [0.0; 2],
        }
    }
}
impl TransformValues {
    pub fn local_matrix(self) -> Mat2D {
        let pivot_x = self.anchor[0].unwrap_or(0.5) * self.size[0];
        let pivot_y = self.anchor[1].unwrap_or(0.5) * self.size[1];
        Mat2D::translate(
            self.position[0] + self.offset[0],
            self.position[1] + self.offset[1],
        )
        .multiply(Mat2D::translate(pivot_x, pivot_y))
        .multiply(Mat2D::rotate(self.rotation))
        .multiply(Mat2D::skew(self.skew[0], self.skew[1]))
        .multiply(Mat2D::scale(
            self.flip[0].unwrap_or(1.0) * self.scale[0],
            self.flip[1].unwrap_or(1.0) * self.scale[1],
        ))
        .multiply(Mat2D::translate(-pivot_x, -pivot_y))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: [f64; 2],
    pub size: [f64; 2],
}
impl Frame {
    pub fn matrix(self, world: Mat2D) -> Mat2D {
        world.multiply(Mat2D::translate(self.origin[0], self.origin[1]))
    }
    pub fn bounds(self, world: Mat2D) -> Aabb {
        self.matrix(world).bounds(self.size[0], self.size[1])
    }
    pub fn quad(self, world: Mat2D) -> Quad {
        self.matrix(world).quad(self.size[0], self.size[1])
    }
    pub fn contains_pointer(self, world: Mat2D, bounds: Aabb, point: Point) -> bool {
        if point.x < bounds.min_x
            || point.x > bounds.max_x
            || point.y < bounds.min_y
            || point.y > bounds.max_y
        {
            return false;
        }
        let local = world.inverse().point(point.x, point.y);
        local.x >= self.origin[0]
            && local.x <= self.origin[0] + self.size[0]
            && local.y >= self.origin[1]
            && local.y <= self.origin[1] + self.size[1]
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GroupChild {
    #[serde(default)]
    pub local: Mat2D,
    pub size: [f64; 2],
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub mask: bool,
}
/// Direct Geometry/Group children only. The caller supplies the local matrices
/// already in the runtime store; the original parent-first walk does not update
/// children first. Child origin/visibility are intentionally not part of this
/// calculation. An empty group retains its previous origin and becomes size 0.
pub fn group_frame(children: &[GroupChild], previous_origin: [f64; 2]) -> Frame {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut any = false;
    for child in children.iter().filter(|child| !child.hidden && !child.mask) {
        let bounds = child.local.bounds(child.size[0], child.size[1]);
        if bounds.min_x < min_x {
            min_x = bounds.min_x;
        }
        if bounds.min_y < min_y {
            min_y = bounds.min_y;
        }
        if bounds.max_x > max_x {
            max_x = bounds.max_x;
        }
        if bounds.max_y > max_y {
            max_y = bounds.max_y;
        }
        any = true;
    }
    if !any {
        Frame {
            origin: previous_origin,
            size: [0.0; 2],
        }
    } else {
        Frame {
            origin: [min_x, min_y],
            size: [
                js_max([0.0, max_x - min_x, 0.0, 0.0]),
                js_max([0.0, max_y - min_y, 0.0, 0.0]),
            ],
        }
    }
}

/// Only top-level bounds are tested against the surface; descendants inherit
/// their parent's cull state even when outside its box. No surface means no
/// top-level culling, as in a headless world.
pub fn is_culled(bounds: Aabb, parent_culled: Option<bool>, surface: Option<[f64; 2]>) -> bool {
    parent_culled.unwrap_or_else(|| {
        surface.is_some_and(|size| {
            !bounds.intersects(Aabb {
                min_x: 0.0,
                min_y: 0.0,
                max_x: size[0],
                max_y: size[1],
            })
        })
    })
}

pub fn point_in_quad(point: Point, quad: Quad) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let p = quad[i];
        let q = quad[j];
        if ((p.y > point.y) != (q.y > point.y))
            && point.x < (q.x - p.x) * (point.y - p.y) / (q.y - p.y) + p.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}
/// Preserves the original vertex-containment test, including its exclusion of
/// crossing quads with no contained vertex. This is not a general SAT test.
pub fn quads_intersect(a: Quad, b: Quad) -> bool {
    (0..4).any(|i| point_in_quad(a[i], b) || point_in_quad(b[i], a))
}
pub fn quad_contains_quad(outer: Quad, inner: Quad) -> bool {
    inner.into_iter().all(|p| point_in_quad(p, outer))
}
pub fn quad_center(q: Quad) -> Point {
    Point {
        x: (q[0].x + q[1].x + q[2].x + q[3].x) / 4.0,
        y: (q[0].y + q[1].y + q[2].y + q[3].y) / 4.0,
    }
}

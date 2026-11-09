//! Rectangles, points and 2D affine maps, in pixels.

/// A rectangle of pixels: corners `x0, y0` (included) to `x1, y1`
/// (excluded). Signed, so a layer moved partly off the frame (into the
/// canvas's margin) needs no other type. Empty as soon as
/// `x1 <= x0` or `y1 <= y0`; every operation accepts empty rects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect { x0: 0, y0: 0, x1: 0, y1: 0 };

    #[inline]
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect { Rect { x0, y0, x1, y1 } }

    /// From a corner and a size (wgpu extents, image sizes).
    #[inline]
    pub const fn from_xywh(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x0: x, y0: y, x1: x + w as i32, y1: y + h as i32 }
    }

    /// `(0, 0)` to `(w, h)`: a canvas or an image.
    #[inline]
    pub const fn of_size(w: u32, h: u32) -> Rect { Rect::from_xywh(0, 0, w, h) }

    /// Width, 0 when empty.
    #[inline]
    pub fn w(&self) -> u32 { (self.x1 - self.x0).max(0) as u32 }

    /// Height, 0 when empty.
    #[inline]
    pub fn h(&self) -> u32 { (self.y1 - self.y0).max(0) as u32 }

    #[inline]
    pub fn is_empty(&self) -> bool { self.x1 <= self.x0 || self.y1 <= self.y0 }

    /// Pixels inside (0 when empty).
    #[inline]
    pub fn area(&self) -> u64 { self.w() as u64 * self.h() as u64 }

    /// The common part, maybe empty (no branch, no `Option`).
    #[inline]
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) }
    }

    /// The common part, `None` when empty.
    #[inline]
    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let r = self.intersect(o);
        (!r.is_empty()).then_some(r)
    }

    /// The smallest rect holding both; an empty side is ignored.
    #[inline]
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() { return *o; }
        if o.is_empty() { return *self; }
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    #[inline]
    pub fn overlaps(&self, o: &Rect) -> bool { !self.intersect(o).is_empty() }

    #[inline]
    pub fn contains(&self, x: i32, y: i32) -> bool { x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1 }

    /// Holds all of `o` (an empty `o` is held by anything).
    #[inline]
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.is_empty() || (o.x0 >= self.x0 && o.y0 >= self.y0 && o.x1 <= self.x1 && o.y1 <= self.y1)
    }

    /// Larger by `n` on every side (smaller when negative).
    #[inline]
    pub fn grown(&self, n: i32) -> Rect { Rect { x0: self.x0 - n, y0: self.y0 - n, x1: self.x1 + n, y1: self.y1 + n } }

    #[inline]
    pub fn moved(&self, dx: i32, dy: i32) -> Rect { Rect { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy } }

    /// Grown out to multiples of `step` (a tile grid with the same
    /// origin): what a dirty rect covers in whole cells.
    #[inline]
    pub fn snapped_out(&self, step: u32) -> Rect {
        let s = step.max(1) as i32;
        Rect {
            x0: self.x0.div_euclid(s) * s,
            y0: self.y0.div_euclid(s) * s,
            x1: (self.x1 + s - 1).div_euclid(s) * s,
            y1: (self.y1 + s - 1).div_euclid(s) * s,
        }
    }

    /// The pixels of `self` inside `0..w, 0..h`, as unsigned corners for
    /// indexing (clip first, then convert); `None` when nothing is left.
    #[inline]
    pub fn clipped_u32(&self, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        let r = self.intersection(&Rect::of_size(w, h))?;
        Some((r.x0 as u32, r.y0 as u32, r.x1 as u32, r.y1 as u32))
    }

    /// The smallest rect holding a box of floats (bounds of a transformed
    /// shape), whole pixels outward. Values within 1e-9 of a whole pixel
    /// count as on it (rounding noise of a rotation must not add a row).
    pub fn around(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        const EPS: f64 = 1e-9;
        let c = |v: f64| v.clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        Rect { x0: c((x0 + EPS).floor()), y0: c((y0 + EPS).floor()), x1: c((x1 - EPS).ceil()), y1: c((y1 - EPS).ceil()) }
    }
}

/// A point or a vector in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Same type: a vector is the move between two points.
pub type Vec2 = Point;

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Point { Point { x, y } }

    #[inline]
    pub fn length(self) -> f64 { self.x.hypot(self.y) }

    #[inline]
    pub fn distance(self, o: Point) -> f64 { (self - o).length() }

    #[inline]
    pub fn dot(self, o: Point) -> f64 { self.x * o.x + self.y * o.y }

    /// z of the cross product: > 0 when `o` turns counterclockwise from
    /// `self` in a y-up frame (clockwise on screen, y down).
    #[inline]
    pub fn cross(self, o: Point) -> f64 { self.x * o.y - self.y * o.x }

    #[inline]
    pub fn lerp(self, o: Point, t: f64) -> Point { Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t) }
}

impl std::ops::Add for Point {
    type Output = Point;
    #[inline]
    fn add(self, o: Point) -> Point { Point::new(self.x + o.x, self.y + o.y) }
}

impl std::ops::Sub for Point {
    type Output = Point;
    #[inline]
    fn sub(self, o: Point) -> Point { Point::new(self.x - o.x, self.y - o.y) }
}

impl std::ops::Mul<f64> for Point {
    type Output = Point;
    #[inline]
    fn mul(self, k: f64) -> Point { Point::new(self.x * k, self.y * k) }
}

impl std::ops::Neg for Point {
    type Output = Point;
    #[inline]
    fn neg(self) -> Point { Point::new(-self.x, -self.y) }
}

impl From<(f64, f64)> for Point {
    #[inline]
    fn from((x, y): (f64, f64)) -> Point { Point::new(x, y) }
}

impl From<(f32, f32)> for Point {
    #[inline]
    fn from((x, y): (f32, f32)) -> Point { Point::new(x as f64, y as f64) }
}

/// A 2D affine map: `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (the
/// order of SVG / canvas `matrix(a, b, c, d, e, f)`). For the view, the
/// tools' transform box and the camera; projective maps (distort,
/// perspective) stay in `aurastudio-transform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Affine {
    fn default() -> Self { Affine::IDENTITY }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    #[inline]
    pub const fn translate(x: f64, y: f64) -> Affine { Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: x, f: y } }

    #[inline]
    pub const fn scale(sx: f64, sy: f64) -> Affine { Affine { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 } }

    /// Turn by `radians` (clockwise on screen, y down).
    #[inline]
    pub fn rotate(radians: f64) -> Affine {
        let (s, c) = radians.sin_cos();
        Affine { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }

    /// `self` then `next` (applies `self` first).
    #[inline]
    pub fn then(&self, next: &Affine) -> Affine {
        let (m, n) = (self, next);
        Affine {
            a: n.a * m.a + n.c * m.b,
            b: n.b * m.a + n.d * m.b,
            c: n.a * m.c + n.c * m.d,
            d: n.b * m.c + n.d * m.d,
            e: n.a * m.e + n.c * m.f + n.e,
            f: n.b * m.e + n.d * m.f + n.f,
        }
    }

    #[inline]
    pub fn apply(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    /// The linear part only (a vector does not move).
    #[inline]
    pub fn apply_vector(&self, v: Vec2) -> Vec2 { Point::new(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y) }

    #[inline]
    pub fn determinant(&self) -> f64 { self.a * self.d - self.b * self.c }

    /// `None` when the map flattens the plane.
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if !det.is_finite() || det.abs() < 1e-12 { return None; }
        let i = 1.0 / det;
        let (a, b, c, d) = (self.d * i, -self.b * i, -self.c * i, self.a * i);
        Some(Affine { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) })
    }

    /// Bounds of `r` once mapped, whole pixels outward.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        if r.is_empty() { return Rect::EMPTY; }
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)].map(|(x, y)| self.apply(Point::new(x as f64, y as f64)));
        let (mut lo, mut hi) = (corners[0], corners[0]);
        for p in &corners[1..] {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        Rect::around(lo.x, lo.y, hi.x, hi.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rects() {
        let a = Rect::new(0, 0, 10, 10);
        let far = Rect::new(20, 20, 30, 30);
        assert!(a.intersect(&far).is_empty());
        assert_eq!(a.intersection(&far), None);
        assert_eq!(Rect::new(5, 5, 5, 9).area(), 0);
        assert_eq!(Rect::new(5, 5, 2, 9).w(), 0);
        // An empty side does not stretch a union.
        assert_eq!(a.union(&Rect::new(100, 100, 100, 100)), a);
        assert_eq!(Rect::EMPTY.union(&far), far);
        assert!(a.contains_rect(&Rect::new(50, 50, 40, 40)));
    }

    #[test]
    fn negative_and_clipping() {
        let r = Rect::new(-30, -10, 20, 5);
        assert_eq!((r.w(), r.h()), (50, 15));
        assert_eq!(r.clipped_u32(10, 10), Some((0, 0, 10, 5)));
        assert_eq!(Rect::new(-5, -5, -1, -1).clipped_u32(10, 10), None);
        assert_eq!(r.intersect(&Rect::of_size(64, 64)), Rect::new(0, 0, 20, 5));
    }

    #[test]
    fn snapping_to_a_grid() {
        assert_eq!(Rect::new(3, 17, 33, 64).snapped_out(16), Rect::new(0, 16, 48, 64));
        assert_eq!(Rect::new(-1, -17, 1, 1).snapped_out(16), Rect::new(-16, -32, 16, 16));
    }

    #[test]
    fn affine_round_trip() {
        let m = Affine::translate(10.0, -4.0).then(&Affine::rotate(0.7)).then(&Affine::scale(2.0, 3.0));
        let p = Point::new(3.5, -8.25);
        let q = m.inverse().unwrap().apply(m.apply(p));
        assert!(p.distance(q) < 1e-9);
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
        // `then` keeps the order: translate first, then scale.
        let t = Affine::translate(1.0, 0.0).then(&Affine::scale(2.0, 2.0));
        assert_eq!(t.apply(Point::ZERO), Point::new(2.0, 0.0));
    }

    #[test]
    fn mapped_rect_bounds() {
        let r = Rect::new(0, 0, 10, 20);
        assert_eq!(Affine::translate(0.5, -0.5).map_rect(&r), Rect::new(0, -1, 11, 20));
        assert_eq!(Affine::rotate(std::f64::consts::FRAC_PI_2).map_rect(&r), Rect::new(-20, 0, 0, 10));
    }
}

//! The transform tool's box: what a press grabs, how a drag changes the
//! quad (Photoshop's gestures in the box's own frame, PhotoCraft's
//! approach), the warp grid, the numbers in the options. What it moves
//! (layers, a selection) is the document's business.

use aurastudio_core::{Point, Rect};
use aurastudio_transform::{corners, keep_convex, Homography, Mesh, Quad, Warp};

/// How the transform tool's handles work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransformMode {
    /// Move, scale, rotate, skew; Ctrl+corner distorts.
    #[default]
    Free,
    /// Each corner on its own.
    Perspective,
    /// A smooth grid to bend.
    Warp,
}

/// The transform as numbers, when its box is a parallelogram: center x,
/// y (px), width and height (% of the source), angle and skew (degrees).
pub type TransformNumbers = [f64; 6];

/// Reach of the handles, as parts of the tolerance a press gets: the
/// pivot a little less (it sits in the middle of everything), the sides
/// (skew) less again, and a box smaller than this many tolerances on a
/// side only moves (its handles would cover it).
const PIVOT_REACH: f64 = 0.8;
const SIDE_REACH: f64 = 0.6;
const SMALL_BOX: f64 = 5.0;

/// A box never gets thinner than this part of its size.
const FLAT: f64 = 1e-4;

/// A transform under way.
#[derive(Clone, Debug)]
pub struct Xform {
    /// What moves (canvas pixels).
    pub src:   Rect,
    pub warp:  Warp,
    /// Rotation / scaling center (canvas pixels).
    pub pivot: Point,
    /// Earlier states, for Ctrl+Z inside the transform.
    pub undo:  Vec<(Warp, Point)>,
    pub redo:  Vec<(Warp, Point)>,
    pub drag:  Option<Drag>,
}

/// What a press grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Inside,
    Outside,
    Pivot,
    /// Clockwise from top-left.
    Corner(usize),
    /// The handle in the middle of edge i (from corner i to i + 1).
    Edge(usize),
    /// Along edge i, off its handle: skew.
    Side(usize),
    /// A warp grid point.
    Point(usize),
}

/// A drag under way: what was grabbed, where, and the box before it.
#[derive(Clone, Debug)]
pub struct Drag {
    pub hit:    Hit,
    pub start:  Point,
    pub quad0:  Quad,
    /// The grid before (warp drags only).
    pub mesh0:  Option<Mesh>,
    pub pivot0: Point,
    pub moved:  bool,
}

/// Keys held during a drag.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods { pub shift: bool, pub alt: bool, pub ctrl: bool }

/// Lines to draw, as one buffer of points cut into polylines (kept by the
/// caller from frame to frame: no allocation per frame).
#[derive(Clone, Debug, Default)]
pub struct Polylines {
    pub pts:    Vec<Point>,
    /// Where each polyline starts in `pts`.
    pub starts: Vec<usize>,
}

impl Polylines {
    pub fn clear(&mut self) { self.pts.clear(); self.starts.clear(); }

    fn begin(&mut self) { self.starts.push(self.pts.len()); }

    /// Each polyline's points.
    pub fn iter(&self) -> impl Iterator<Item = &[Point]> {
        self.starts.iter().enumerate().map(|(k, &s)| &self.pts[s..self.starts.get(k + 1).copied().unwrap_or(self.pts.len())])
    }

    pub fn len(&self) -> usize { self.starts.len() }
    pub fn is_empty(&self) -> bool { self.starts.is_empty() }
}

/// Distance from p to segment a-b.
fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + d * t)
}

fn inside(q: &Quad, p: Point) -> bool {
    let mut c = false;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) { c = !c; }
    }
    c
}

fn turned(q: Quad, c: Point, angle: f64) -> Quad {
    let (sn, cs) = angle.sin_cos();
    q.map(|p| { let d = p - c; c + Point::new(d.x * cs - d.y * sn, d.x * sn + d.y * cs) })
}

/// Where the pivot lands when the box goes from `q0` to `q`: the same place
/// in it.
fn carry_pivot(q0: Quad, q: Quad, pivot0: Point) -> Point {
    let (Some(h0), Some(h1)) = (Homography::square_to_quad(q0), Homography::square_to_quad(q)) else { return pivot0 };
    let Some(inv) = h0.inverse() else { return pivot0 };
    let p = h1.apply(inv.apply(pivot0));
    if p.x.is_finite() && p.y.is_finite() { p } else { pivot0 }
}

fn finite(q: &Quad) -> bool { q.iter().all(|c| c.x.is_finite() && c.y.is_finite()) }

impl Xform {
    pub fn new(src: Rect) -> Self {
        let pivot = Point::new((src.x0 + src.x1) as f64 / 2.0, (src.y0 + src.y1) as f64 / 2.0);
        Self { src, warp: Warp::identity(src), pivot, undo: Vec::new(), redo: Vec::new(), drag: None }
    }

    /// Changed at all since it began.
    pub fn changed(&self) -> bool { self.warp != Warp::identity(self.src) }

    /// Keep the state before a change (Ctrl+Z goes back to it).
    pub fn remember(&mut self) {
        self.undo.push((self.warp.clone(), self.pivot));
        self.redo.clear();
    }

    pub fn step_back(&mut self) -> bool {
        let Some((w, p)) = self.undo.pop() else { return false };
        self.redo.push((std::mem::replace(&mut self.warp, w), std::mem::replace(&mut self.pivot, p)));
        true
    }

    pub fn step_forward(&mut self) -> bool {
        let Some((w, p)) = self.redo.pop() else { return false };
        self.undo.push((std::mem::replace(&mut self.warp, w), std::mem::replace(&mut self.pivot, p)));
        true
    }

    /// The box's homography: unit square → canvas.
    fn h(&self) -> Option<Homography> { Homography::square_to_quad(self.warp.quad) }

    /// Middle of each edge, on the box.
    pub fn edge_handles(&self) -> [Point; 4] {
        let q = self.warp.quad;
        let Some(h) = self.h() else { return std::array::from_fn(|i| q[i].lerp(q[(i + 1) % 4], 0.5)) };
        [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)].map(|(u, v)| h.apply(Point::new(u, v)))
    }

    /// Warp grid points on the canvas.
    pub fn mesh_points(&self) -> Vec<Point> {
        let (Some(h), Some(m)) = (self.h(), &self.warp.mesh) else { return Vec::new() };
        m.pts.iter().map(|p| h.apply(*p)).collect()
    }

    /// The lines to draw into `out` (emptied first): the box (bent by the
    /// grid), and the grid in warp mode.
    pub fn lines(&self, mode: TransformMode, out: &mut Polylines) {
        out.clear();
        let s = self.src;
        let Some(map) = self.warp.mapper(s) else { return };
        let (sw, sh) = (s.w() as f64, s.h() as f64);
        let curve = |out: &mut Polylines, f: &dyn Fn(f64) -> (f64, f64)| {
            out.begin();
            for k in 0..=32 {
                let (u, v) = f(k as f64 / 32.0);
                let p = map.map(Point::new(s.x0 as f64 + u * sw, s.y0 as f64 + v * sh));
                if p.x.is_finite() && p.y.is_finite() { out.pts.push(p); }
            }
        };
        match &self.warp.mesh {
            Some(m) if mode == TransformMode::Warp || !m.is_identity() => {
                for j in 0..=m.rows {
                    let v = j as f64 / m.rows as f64;
                    if mode == TransformMode::Warp || j == 0 || j == m.rows { curve(out, &|t| (t, v)); }
                }
                for i in 0..=m.cols {
                    let u = i as f64 / m.cols as f64;
                    if mode == TransformMode::Warp || i == 0 || i == m.cols { curve(out, &|t| (u, t)); }
                }
            }
            _ => {
                out.begin();
                out.pts.extend((0..=4).map(|i| self.warp.quad[i % 4]));
            }
        }
    }

    /// What a press at `p` grabs, `tol` canvas pixels around the handles.
    pub fn hit(&self, p: Point, tol: f64, mode: TransformMode) -> Hit {
        let q = self.warp.quad;
        let body = if inside(&q, p) { Hit::Inside } else { Hit::Outside };
        if mode == TransformMode::Warp {
            return match self.mesh_points().iter().position(|m| m.distance(p) <= tol) { Some(i) => Hit::Point(i), None => body };
        }
        // The nearest corner in reach.
        let corner = (0..4).filter(|&i| q[i].distance(p) <= tol).min_by(|&a, &b| q[a].distance(p).total_cmp(&q[b].distance(p)));
        if let Some(i) = corner { return Hit::Corner(i); }
        let small = (0..4).map(|i| q[(i + 1) % 4].distance(q[i])).fold(f64::MAX, f64::min) < tol * SMALL_BOX;
        if small { return body; }
        if mode == TransformMode::Free && self.pivot.distance(p) <= tol * PIVOT_REACH { return Hit::Pivot; }
        if let Some(i) = (0..4).find(|&i| self.edge_handles()[i].distance(p) <= tol) { return Hit::Edge(i); }
        if mode == TransformMode::Free {
            if let Some(i) = (0..4).find(|&i| seg_dist(p, q[i], q[(i + 1) % 4]) <= tol * SIDE_REACH) { return Hit::Side(i); }
        }
        body
    }

    pub fn press(&mut self, hit: Hit, p: Point) {
        let mesh0 = matches!(hit, Hit::Point(_)).then(|| self.warp.mesh.clone()).flatten();
        self.drag = Some(Drag { hit, start: p, quad0: self.warp.quad, mesh0, pivot0: self.pivot, moved: false });
    }

    /// The pointer at `p` during a drag. False if nothing changed.
    pub fn drag_to(&mut self, p: Point, m: Mods, mode: TransformMode, keep_aspect: bool) -> bool {
        let Some(g) = &self.drag else { return false };
        let d = p - g.start;
        if !g.moved && d.x.abs() < 1e-9 && d.y.abs() < 1e-9 { return false; }
        let (hit, q0, pivot0, start, first) = (g.hit, g.quad0, g.pivot0, g.start, !g.moved);
        if first { self.remember(); }
        if let Some(g) = &mut self.drag { g.moved = true; }
        let distort = mode == TransformMode::Perspective || m.ctrl;
        let next = match hit {
            Hit::Inside => Some(move_by(q0, pivot0, d, m)),
            Hit::Pivot => Some((q0, p)),
            Hit::Outside => Some((rotate(q0, pivot0, start, p, m), pivot0)),
            Hit::Corner(i) if distort => Some(distort_corner(q0, pivot0, i, d)),
            Hit::Edge(i) if mode == TransformMode::Perspective => Some(move_edge(q0, pivot0, i, d)),
            Hit::Side(i) => Some(skew(q0, pivot0, i, d, m)),
            Hit::Edge(i) if m.ctrl => Some(skew(q0, pivot0, i, d, m)),
            Hit::Corner(_) | Hit::Edge(_) => scale(q0, pivot0, hit, p, m, keep_aspect),
            Hit::Point(i) => { self.move_point(i, p); return true; }
        };
        if let Some((quad, pivot)) = next {
            if finite(&quad) && Homography::square_to_quad(quad).is_some() {
                self.warp.quad = quad;
                self.pivot = pivot;
            }
        }
        true
    }

    /// A warp grid point dragged to `p` (in the box's own frame).
    fn move_point(&mut self, i: usize, p: Point) {
        let Some(g) = &self.drag else { return };
        let Some(inv) = Homography::square_to_quad(g.quad0).and_then(|h| h.inverse()) else { return };
        let u = inv.apply(p);
        if let (Some(mesh), Some(m0)) = (&mut self.warp.mesh, &g.mesh0) {
            if u.x.is_finite() && u.y.is_finite() && i < mesh.pts.len() {
                mesh.pts.copy_from_slice(&m0.pts);
                mesh.pts[i] = u;
            }
        }
    }

    /// The drag ended. True if it changed something.
    pub fn release(&mut self) -> bool { self.drag.take().is_some_and(|d| d.moved) }

    /// Nudge by whole pixels (arrow keys).
    pub fn nudge(&mut self, dx: f64, dy: f64) {
        self.remember();
        let d = Point::new(dx, dy);
        self.warp.quad = self.warp.quad.map(|c| c + d);
        self.pivot = self.pivot + d;
    }

    pub fn flip(&mut self, horizontal: bool) {
        self.remember();
        let q = self.warp.quad;
        self.warp.quad = if horizontal { [q[1], q[0], q[3], q[2]] } else { [q[3], q[2], q[1], q[0]] };
        if let Some(m) = &mut self.warp.mesh {
            for p in &mut m.pts { if horizontal { p.x = 1.0 - p.x; } else { p.y = 1.0 - p.y; } }
            let (c, r) = (m.cols + 1, m.rows + 1);
            let old = m.pts.clone();
            for j in 0..r {
                for i in 0..c {
                    let (si, sj) = if horizontal { (c - 1 - i, j) } else { (i, r - 1 - j) };
                    m.pts[j * c + i] = old[sj * c + si];
                }
            }
        }
    }

    pub fn rotate(&mut self, degrees: f64) {
        self.remember();
        self.warp.quad = turned(self.warp.quad, self.pivot, degrees.to_radians());
    }

    pub fn reset(&mut self) {
        self.remember();
        let undo = std::mem::take(&mut self.undo);
        *self = Self { undo, ..Self::new(self.src) };
    }

    pub fn set_grid(&mut self, n: u32) {
        let n = n.clamp(1, 8) as usize;
        if self.warp.mesh.as_ref().is_some_and(|m| m.cols == n && m.rows == n) { return; }
        self.remember();
        self.warp.mesh = Some(Mesh::new(n, n));
    }

    /// Center, size (% of the source), angle, skew: for a box that is a
    /// parallelogram.
    pub fn numbers(&self) -> Option<TransformNumbers> {
        if self.warp.bent().is_some() { return None; }
        let q = self.warp.quad;
        if ((q[0] + q[2]) - (q[1] + q[3])).length() > 1e-6 * (1.0 + q[0].length()) { return None; }
        let (sw, sh) = (self.src.w() as f64, self.src.h() as f64);
        let c = q[0].lerp(q[2], 0.5);
        let (e0, e1) = (q[1] - q[0], q[3] - q[0]);
        let angle = e0.y.atan2(e0.x);
        let (sn, cs) = (-angle).sin_cos();
        let (a, b) = (e1.x * cs - e1.y * sn, e1.x * sn + e1.y * cs);
        let skew = if b.abs() > 1e-9 { (a / b).atan().to_degrees() } else { 0.0 };
        let round = |v: f64| (v * 100.0).round() / 100.0;
        Some([round(c.x), round(c.y), round(e0.length() / sw * 100.0), round(b / sh * 100.0), round(angle.to_degrees()), round(skew)])
    }

    pub fn set_numbers(&mut self, n: TransformNumbers) {
        let (sw, sh) = (self.src.w() as f64, self.src.h() as f64);
        let (w, h) = (sw * n[2] / 100.0, sh * n[3] / 100.0);
        if w.abs() < 0.01 || h.abs() < 0.01 || n[5].abs() >= 89.0 { return; }
        self.remember();
        let (sn, cs) = n[4].to_radians().sin_cos();
        let rot = |v: Point| Point::new(v.x * cs - v.y * sn, v.x * sn + v.y * cs);
        let ex = rot(Point::new(w, 0.0));
        let ey = rot(Point::new(n[5].to_radians().tan() * h, h));
        let q0 = Point::new(n[0], n[1]) - (ex + ey) * 0.5;
        let old = self.warp.quad;
        self.warp.quad = [q0, q0 + ex, q0 + ex + ey, q0 + ey];
        self.pivot = carry_pivot(old, self.warp.quad, self.pivot);
    }

    /// The source's corners.
    pub fn source_corners(&self) -> Quad { corners(self.src) }
}

// --- The gestures: (box, pivot) after the drag --------------------------

/// Inside: the whole box moves (Shift: along the closest of 8 directions).
fn move_by(q0: Quad, pivot0: Point, d: Point, m: Mods) -> (Quad, Point) {
    let d = if m.shift {
        let a = (d.y.atan2(d.x) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
        let l = d.x * a.cos() + d.y * a.sin();
        Point::new(l * a.cos(), l * a.sin())
    } else { d };
    (q0.map(|c| c + d), pivot0 + d)
}

/// Outside: turn around the pivot (Shift: the top edge by 15° steps).
fn rotate(q0: Quad, c: Point, start: Point, p: Point, m: Mods) -> Quad {
    let a0 = (start.y - c.y).atan2(start.x - c.x);
    let a1 = (p.y - c.y).atan2(p.x - c.x);
    let mut da = a1 - a0;
    if m.shift {
        let base = (q0[1].y - q0[0].y).atan2(q0[1].x - q0[0].x);
        let step = 15f64.to_radians();
        da = ((base + da) / step).round() * step - base;
    }
    turned(q0, c, da)
}

/// Ctrl + corner (or perspective): the corner alone, never folding over.
fn distort_corner(q0: Quad, pivot0: Point, i: usize, d: Point) -> (Quad, Point) {
    let mut q = q0;
    q[i] = q0[i] + d;
    let quad = keep_convex(q0, q);
    (quad, carry_pivot(q0, quad, pivot0))
}

/// Perspective, an edge handle: both its corners.
fn move_edge(q0: Quad, pivot0: Point, i: usize, d: Point) -> (Quad, Point) {
    let mut q = q0;
    for k in [i, (i + 1) % 4] { q[k] = q0[k] + d; }
    let quad = keep_convex(q0, q);
    (quad, carry_pivot(q0, quad, pivot0))
}

/// Skew: the edge slides along itself (Alt: the opposite one the other
/// way).
fn skew(q0: Quad, pivot0: Point, i: usize, d: Point, m: Mods) -> (Quad, Point) {
    let (a, b) = (i, (i + 1) % 4);
    let e = q0[b] - q0[a];
    let l2 = e.dot(e);
    let mv = e * if l2 > 0.0 { d.dot(e) / l2 } else { 0.0 };
    let mut quad = q0;
    quad[a] = q0[a] + mv;
    quad[b] = q0[b] + mv;
    if m.alt { for k in [(i + 2) % 4, (i + 3) % 4] { quad[k] = q0[k] - mv; } }
    (quad, carry_pivot(q0, quad, pivot0))
}

/// Scale in the box's own frame (a turned box scales along its sides):
/// the opposite side stays, or the pivot with Alt. Corners keep the
/// proportions (Shift: free); an edge stretches its own axis (Shift:
/// both), as in Krita and CSP.
fn scale(q0: Quad, pivot0: Point, hit: Hit, p: Point, m: Mods, keep_aspect: bool) -> Option<(Quad, Point)> {
    let h = Homography::square_to_quad(q0)?;
    let inv = h.inverse()?;
    let (u, pu) = (inv.apply(p), inv.apply(pivot0));
    // Which side of each axis the handle moves: (at 0, at 1).
    let (mu, mv) = match hit {
        Hit::Corner(0) => ((true, false), (true, false)),
        Hit::Corner(1) => ((false, true), (true, false)),
        Hit::Corner(2) => ((false, true), (false, true)),
        Hit::Corner(3) => ((true, false), (false, true)),
        Hit::Edge(0) => ((false, false), (true, false)),
        Hit::Edge(1) => ((false, true), (false, false)),
        Hit::Edge(2) => ((false, false), (false, true)),
        _ => ((true, false), (false, false)),
    };
    // (anchor, factor) along one axis.
    let axis = |moving: (bool, bool), at: f64, piv: f64| -> (f64, f64) {
        if !moving.0 && !moving.1 { return (0.5, 1.0); }
        let handle = if moving.0 { 0.0 } else { 1.0 };
        let anchor = if m.alt { piv } else { 1.0 - handle };
        let d = handle - anchor;
        let k = if d.abs() > 1e-9 { (at - anchor) / d } else { 1.0 };
        (anchor, if k.is_finite() { k } else { 1.0 })
    };
    let (mut au, mut ku) = axis(mu, u.x, pu.x);
    let (mut av, mut kv) = axis(mv, u.y, pu.y);
    let corner = matches!(hit, Hit::Corner(_));
    let proportional = if corner { keep_aspect != m.shift } else { m.shift };
    if proportional && !corner {
        // An edge sets its axis; the other follows about the middle.
        if mu.0 || mu.1 { (av, kv) = (if m.alt { pu.y } else { 0.5 }, ku.abs()); } else { (au, ku) = (if m.alt { pu.x } else { 0.5 }, kv.abs()); }
    }
    if proportional && corner {
        let k = ku.abs().max(kv.abs());
        (ku, kv) = (k * ku.signum(), k * kv.signum());
    }
    // Never flat.
    let unflat = |k: f64| if k.abs() < FLAT { FLAT.copysign(if k == 0.0 { 1.0 } else { k }) } else { k };
    let (ku, kv) = (unflat(ku), unflat(kv));
    let r = [au - au * ku, av - av * kv, au + (1.0 - au) * ku, av + (1.0 - av) * kv];
    let map = |a: f64, b: f64| h.apply(Point::new(a, b));
    let quad = [map(r[0], r[1]), map(r[2], r[1]), map(r[2], r[3]), map(r[0], r[3])];
    Some((quad, if m.alt { pivot0 } else { carry_pivot(q0, quad, pivot0) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point { Point::new(x, y) }
    fn near(a: Point, b: Point) -> bool { a.distance(b) < 1e-6 }

    #[test]
    fn corners_scale_from_the_opposite_one_proportionally() {
        let mut x = Xform::new(Rect::new(0, 0, 100, 50));
        let hit = x.hit(pt(100.0, 50.0), 5.0, TransformMode::Free);
        assert_eq!(hit, Hit::Corner(2));
        x.press(hit, pt(100.0, 50.0));
        x.drag_to(pt(200.0, 60.0), Mods::default(), TransformMode::Free, true);
        assert!(near(x.warp.quad[0], pt(0.0, 0.0)));
        assert!(near(x.warp.quad[2], pt(200.0, 100.0)), "{:?}", x.warp.quad);
        // Shift: free.
        x.drag_to(pt(200.0, 60.0), Mods { shift: true, ..Default::default() }, TransformMode::Free, true);
        assert!(near(x.warp.quad[2], pt(200.0, 60.0)));
        assert!(x.release());
        assert!(x.step_back());
        assert_eq!(x.warp, Warp::identity(Rect::new(0, 0, 100, 50)));
        assert!(x.step_forward());
        // An edge with Alt scales about the pivot.
        let mut y = Xform::new(Rect::new(0, 0, 100, 100));
        y.press(Hit::Edge(1), pt(100.0, 50.0));
        y.drag_to(pt(150.0, 50.0), Mods { alt: true, ..Default::default() }, TransformMode::Free, true);
        assert!(near(y.warp.quad[0], pt(-50.0, 0.0)) && near(y.warp.quad[1], pt(150.0, 0.0)), "{:?}", y.warp.quad);
    }

    #[test]
    fn move_rotate_skew_distort_and_numbers() {
        let mut x = Xform::new(Rect::new(0, 0, 100, 100));
        let m = Mods::default();
        assert_eq!(x.hit(pt(50.0, 50.0), 5.0, TransformMode::Free), Hit::Pivot);
        assert_eq!(x.hit(pt(30.0, 30.0), 5.0, TransformMode::Free), Hit::Inside);
        x.press(Hit::Inside, pt(30.0, 30.0));
        x.drag_to(pt(40.0, 35.0), m, TransformMode::Free, true);
        x.release();
        assert!(near(x.warp.quad[0], pt(10.0, 5.0)));
        assert!(near(x.pivot, pt(60.0, 55.0)));
        // A quarter turn around the pivot.
        assert_eq!(x.hit(pt(200.0, 55.0), 5.0, TransformMode::Free), Hit::Outside);
        x.press(Hit::Outside, pt(160.0, 55.0));
        x.drag_to(pt(60.0, 155.0), m, TransformMode::Free, true);
        x.release();
        let n = x.numbers().unwrap();
        assert!((n[4] - 90.0).abs() < 1e-6 && (n[2] - 100.0).abs() < 1e-6, "{n:?}");
        x.set_numbers([60.0, 55.0, 50.0, 100.0, 0.0, 0.0]);
        assert!(near(x.warp.quad[0], pt(35.0, 5.0)) && near(x.warp.quad[2], pt(85.0, 105.0)), "{:?}", x.warp.quad);
        // Skew: the top edge slides.
        assert_eq!(x.hit(pt(45.0, 5.0), 5.0, TransformMode::Free), Hit::Side(0));
        x.press(Hit::Side(0), pt(45.0, 5.0));
        x.drag_to(pt(65.0, 9.0), m, TransformMode::Free, true);
        x.release();
        assert!(near(x.warp.quad[0], pt(55.0, 5.0)) && near(x.warp.quad[3], pt(35.0, 105.0)));
        assert!(x.numbers().unwrap()[5].abs() > 1.0);
        // Ctrl+corner: distort, never folding over.
        x.press(Hit::Corner(2), x.warp.quad[2]);
        x.drag_to(pt(0.0, -500.0), Mods { ctrl: true, ..Default::default() }, TransformMode::Free, true);
        assert!(aurastudio_transform::convex(&x.warp.quad));
        assert!(x.numbers().is_none());
        x.release();
        x.reset();
        assert_eq!(x.warp.quad, x.source_corners());
        // A press that never moves changes nothing.
        x.press(Hit::Inside, pt(10.0, 10.0));
        assert!(!x.drag_to(pt(10.0, 10.0), m, TransformMode::Free, true));
        assert!(!x.release());
    }

    #[test]
    fn flips_rotations_and_the_grid() {
        let mut x = Xform::new(Rect::new(0, 0, 100, 40));
        x.flip(true);
        assert!(near(x.warp.quad[0], pt(100.0, 0.0)));
        x.flip(true);
        x.rotate(90.0);
        assert!(near(x.warp.quad[0], pt(70.0, -30.0)), "{:?}", x.warp.quad);
        x.reset();
        x.set_grid(2);
        assert_eq!(x.mesh_points().len(), 9);
        let hit = x.hit(pt(50.0, 20.0), 3.0, TransformMode::Warp);
        assert_eq!(hit, Hit::Point(4));
        x.press(hit, pt(50.0, 20.0));
        x.drag_to(pt(50.0, 30.0), Mods::default(), TransformMode::Warp, true);
        let p = x.warp.map(x.src, pt(50.0, 20.0)).unwrap();
        assert!((p.y - 30.0).abs() < 1e-6);
        assert!(x.numbers().is_none());
        // The lines: the grid's 3 rows and 3 columns, one buffer.
        let mut lines = Polylines::default();
        x.lines(TransformMode::Warp, &mut lines);
        assert_eq!(lines.len(), 6);
        assert!(lines.iter().all(|l| l.len() == 33));
        let cap = lines.pts.capacity();
        x.lines(TransformMode::Free, &mut lines);
        assert_eq!((lines.len(), lines.pts.capacity()), (4, cap), "a bent box: its four sides, buffer kept");
    }
}

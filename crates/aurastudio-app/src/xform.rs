//! The transform tool's box: what a press grabs, how a drag changes the
//! quad (Photoshop's gestures in the box's own frame, PhotoCraft's
//! approach), the warp grid, the numbers in the options.

use aurastudio_core::Rect;
use aurastudio_transform::{keep_convex, Homography, Mesh, Warp};
use aurastudio_ui::{TransformMode, TransformNumbers};

pub type P = [f64; 2];

/// A transform under way, as the app sees it.
#[derive(Clone, Debug)]
pub struct Xform {
    /// What moves (canvas pixels).
    pub src:   Rect,
    pub warp:  Warp,
    /// Rotation / scaling center (canvas pixels).
    pub pivot: P,
    /// Earlier states, for Ctrl+Z inside the transform.
    pub undo:  Vec<(Warp, P)>,
    pub redo:  Vec<(Warp, P)>,
    pub drag:  Option<Drag>,
    /// The layers it was asked for.
    pub targets: Vec<u64>,
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

#[derive(Clone, Debug)]
pub struct Drag {
    pub hit:    Hit,
    pub start:  P,
    pub warp0:  Warp,
    pub pivot0: P,
    pub moved:  bool,
}

/// Keys held during a drag.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods { pub shift: bool, pub alt: bool, pub ctrl: bool }

fn sub(a: P, b: P) -> P { [a[0] - b[0], a[1] - b[1]] }
fn add(a: P, b: P) -> P { [a[0] + b[0], a[1] + b[1]] }
fn len(a: P) -> f64 { (a[0] * a[0] + a[1] * a[1]).sqrt() }
fn mid(a: P, b: P) -> P { [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0] }

/// Distance from p to segment a-b.
fn seg_dist(p: P, a: P, b: P) -> f64 {
    let d = sub(b, a);
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    len(sub(p, [a[0] + d[0] * t, a[1] + d[1] * t]))
}

fn inside(q: &[P; 4], p: P) -> bool {
    let mut c = false;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) { c = !c; }
    }
    c
}

/// Where the pivot lands when the box goes from `q0` to `q`: the same place
/// in it.
fn carry_pivot(q0: [P; 4], q: [P; 4], pivot0: P) -> P {
    let (Some(h0), Some(h1)) = (Homography::square_to_quad(q0), Homography::square_to_quad(q)) else { return pivot0 };
    let Some(inv) = h0.inverse() else { return pivot0 };
    let (u, v) = inv.apply(pivot0[0], pivot0[1]);
    let (x, y) = h1.apply(u, v);
    if x.is_finite() && y.is_finite() { [x, y] } else { pivot0 }
}

impl Xform {
    pub fn new(src: Rect, targets: Vec<u64>) -> Self {
        let pivot = [(src.0 + src.2) as f64 / 2.0, (src.1 + src.3) as f64 / 2.0];
        Self { src, warp: Warp::identity(src), pivot, undo: Vec::new(), redo: Vec::new(), drag: None, targets }
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

    /// The box's homography: unit square -> canvas.
    fn h(&self) -> Option<Homography> { Homography::square_to_quad(self.warp.quad) }

    /// Middle of each edge, on the box.
    pub fn edge_handles(&self) -> [P; 4] {
        let Some(h) = self.h() else { return std::array::from_fn(|i| mid(self.warp.quad[i], self.warp.quad[(i + 1) % 4])) };
        [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)].map(|(u, v)| { let (x, y) = h.apply(u, v); [x, y] })
    }

    /// Warp grid points on the canvas.
    pub fn mesh_points(&self) -> Vec<P> {
        let (Some(h), Some(m)) = (self.h(), &self.warp.mesh) else { return Vec::new() };
        m.pts.iter().map(|p| { let (x, y) = h.apply(p[0], p[1]); [x, y] }).collect()
    }

    /// Lines to draw: the box (bent by the grid), and the grid.
    pub fn lines(&self, mode: TransformMode) -> Vec<Vec<(f32, f32)>> {
        let s = self.src;
        let (sw, sh) = ((s.2 - s.0) as f64, (s.3 - s.1) as f64);
        let curve = |f: &dyn Fn(f64) -> (f64, f64)| -> Vec<(f32, f32)> {
            (0..=32).filter_map(|k| {
                let t = k as f64 / 32.0;
                let (u, v) = f(t);
                self.warp.map(s, s.0 as f64 + u * sw, s.1 as f64 + v * sh).map(|(x, y)| (x as f32, y as f32))
            }).collect()
        };
        match &self.warp.mesh {
            Some(m) if mode == TransformMode::Warp || !m.is_identity() => {
                let mut out = Vec::new();
                for j in 0..=m.rows {
                    let v = j as f64 / m.rows as f64;
                    if mode == TransformMode::Warp || j == 0 || j == m.rows { out.push(curve(&|t| (t, v))); }
                }
                for i in 0..=m.cols {
                    let u = i as f64 / m.cols as f64;
                    if mode == TransformMode::Warp || i == 0 || i == m.cols { out.push(curve(&|t| (u, t))); }
                }
                out
            }
            _ => {
                let q = self.warp.quad;
                vec![(0..=4).map(|i| (q[i % 4][0] as f32, q[i % 4][1] as f32)).collect()]
            }
        }
    }

    /// What a press at `p` grabs, `tol` canvas pixels around the handles.
    pub fn hit(&self, p: P, tol: f64, mode: TransformMode) -> Hit {
        let q = self.warp.quad;
        if mode == TransformMode::Warp {
            if let Some(i) = self.mesh_points().iter().position(|m| len(sub(*m, p)) <= tol) { return Hit::Point(i); }
            return if inside(&q, p) { Hit::Inside } else { Hit::Outside };
        }
        // The nearest corner in reach.
        let corner = (0..4).filter(|&i| len(sub(q[i], p)) <= tol).min_by(|&a, &b| len(sub(q[a], p)).total_cmp(&len(sub(q[b], p))));
        if let Some(i) = corner { return Hit::Corner(i); }
        // A small box: inside moves it (its pivot and edge handles would
        // cover it all).
        let small = (0..4).map(|i| len(sub(q[(i + 1) % 4], q[i]))).fold(f64::MAX, f64::min) < tol * 5.0;
        if small { return if inside(&q, p) { Hit::Inside } else { Hit::Outside }; }
        if mode == TransformMode::Free && len(sub(self.pivot, p)) <= tol * 0.8 { return Hit::Pivot; }
        if let Some(i) = (0..4).find(|&i| len(sub(self.edge_handles()[i], p)) <= tol) { return Hit::Edge(i); }
        if mode == TransformMode::Free {
            if let Some(i) = (0..4).find(|&i| seg_dist(p, q[i], q[(i + 1) % 4]) <= tol * 0.6) { return Hit::Side(i); }
        }
        if inside(&q, p) { Hit::Inside } else { Hit::Outside }
    }

    pub fn press(&mut self, hit: Hit, p: P) {
        self.drag = Some(Drag { hit, start: p, warp0: self.warp.clone(), pivot0: self.pivot, moved: false });
    }

    /// The pointer at `p` during a drag. False if nothing changed.
    pub fn drag_to(&mut self, p: P, m: Mods, mode: TransformMode, keep_aspect: bool) -> bool {
        let Some(g) = self.drag.clone() else { return false };
        let (dx, dy) = (p[0] - g.start[0], p[1] - g.start[1]);
        if !g.moved && dx.abs() < 1e-9 && dy.abs() < 1e-9 { return false; }
        if !g.moved { self.remember(); }
        if let Some(d) = &mut self.drag { d.moved = true; }
        let q0 = g.warp0.quad;
        let mut quad = q0;
        let mut pivot = g.pivot0;
        match g.hit {
            Hit::Inside => {
                let (dx, dy) = if m.shift {
                    // Along the closest of 8 directions.
                    let a = (dy.atan2(dx) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
                    let l = dx * a.cos() + dy * a.sin();
                    (l * a.cos(), l * a.sin())
                } else { (dx, dy) };
                quad = q0.map(|c| [c[0] + dx, c[1] + dy]);
                pivot = [pivot[0] + dx, pivot[1] + dy];
            }
            Hit::Pivot => pivot = p,
            Hit::Outside => {
                let c = g.pivot0;
                let a0 = (g.start[1] - c[1]).atan2(g.start[0] - c[0]);
                let a1 = (p[1] - c[1]).atan2(p[0] - c[0]);
                let mut da = a1 - a0;
                if m.shift {
                    let base = (q0[1][1] - q0[0][1]).atan2(q0[1][0] - q0[0][0]);
                    let step = 15f64.to_radians();
                    da = ((base + da) / step).round() * step - base;
                }
                let (sn, cs) = da.sin_cos();
                quad = q0.map(|q| { let (x, y) = (q[0] - c[0], q[1] - c[1]); [c[0] + x * cs - y * sn, c[1] + x * sn + y * cs] });
            }
            Hit::Corner(i) if mode == TransformMode::Perspective || m.ctrl => {
                let mut q = q0;
                q[i] = [q0[i][0] + dx, q0[i][1] + dy];
                quad = keep_convex(q0, q);
                pivot = carry_pivot(q0, quad, g.pivot0);
            }
            Hit::Edge(i) if mode == TransformMode::Perspective => {
                let (a, b) = (i, (i + 1) % 4);
                let mut q = q0;
                q[a] = [q0[a][0] + dx, q0[a][1] + dy];
                q[b] = [q0[b][0] + dx, q0[b][1] + dy];
                quad = keep_convex(q0, q);
                pivot = carry_pivot(q0, quad, g.pivot0);
            }
            Hit::Side(i) | Hit::Edge(i) if matches!(g.hit, Hit::Side(_)) || m.ctrl => {
                // Skew: the edge slides along itself (Alt: the opposite one
                // the other way).
                let (a, b) = (i, (i + 1) % 4);
                let e = sub(q0[b], q0[a]);
                let l2 = e[0] * e[0] + e[1] * e[1];
                let t = if l2 > 0.0 { (dx * e[0] + dy * e[1]) / l2 } else { 0.0 };
                let (mx, my) = (t * e[0], t * e[1]);
                quad[a] = [q0[a][0] + mx, q0[a][1] + my];
                quad[b] = [q0[b][0] + mx, q0[b][1] + my];
                if m.alt {
                    let (c, d) = ((i + 2) % 4, (i + 3) % 4);
                    quad[c] = [q0[c][0] - mx, q0[c][1] - my];
                    quad[d] = [q0[d][0] - mx, q0[d][1] - my];
                }
                pivot = carry_pivot(q0, quad, g.pivot0);
            }
            Hit::Corner(_) | Hit::Edge(_) | Hit::Side(_) => {
                // Scale in the box's own frame (a turned box scales along its
                // sides): the opposite side stays, or the pivot with Alt.
                let Some(h) = Homography::square_to_quad(q0) else { return false };
                let Some(inv) = h.inverse() else { return false };
                let (u, v) = inv.apply(p[0], p[1]);
                let (pu, pv) = inv.apply(g.pivot0[0], g.pivot0[1]);
                let (mu, mv) = match g.hit {
                    Hit::Corner(0) => ((true, false), (true, false)),
                    Hit::Corner(1) => ((false, true), (true, false)),
                    Hit::Corner(2) => ((false, true), (false, true)),
                    Hit::Corner(3) => ((true, false), (false, true)),
                    Hit::Edge(0) => ((false, false), (true, false)),
                    Hit::Edge(1) => ((false, true), (false, false)),
                    Hit::Edge(2) => ((false, false), (false, true)),
                    _ => ((true, false), (false, false)),
                };
                let axis = |moving: (bool, bool), at: f64, piv: f64| -> (f64, f64) {
                    if !moving.0 && !moving.1 { return (0.5, 1.0); }
                    let handle = if moving.0 { 0.0 } else { 1.0 };
                    let anchor = if m.alt { piv } else { 1.0 - handle };
                    let d = handle - anchor;
                    let k = if d.abs() > 1e-9 { (at - anchor) / d } else { 1.0 };
                    (anchor, if k.is_finite() { k } else { 1.0 })
                };
                let (mut au, mut ku) = axis(mu, u, pu);
                let (mut av, mut kv) = axis(mv, v, pv);
                // Corners keep the proportions (Shift: free); an edge
                // stretches its own axis (Shift: both), as in Krita and CSP.
                let corner = matches!(g.hit, Hit::Corner(_));
                let proportional = if corner { keep_aspect != m.shift } else { m.shift };
                if proportional && !corner {
                    // An edge sets its axis; the other follows about the middle.
                    if mu.0 || mu.1 { (av, kv) = (if m.alt { pv } else { 0.5 }, ku.abs()); } else { (au, ku) = (if m.alt { pu } else { 0.5 }, kv.abs()); }
                }
                if proportional && corner {
                    let k = ku.abs().max(kv.abs());
                    (ku, kv) = (k * ku.signum(), k * kv.signum());
                }
                // Never flat.
                if ku.abs() < 1e-4 { ku = 1e-4f64.copysign(if ku == 0.0 { 1.0 } else { ku }); }
                if kv.abs() < 1e-4 { kv = 1e-4f64.copysign(if kv == 0.0 { 1.0 } else { kv }); }
                let r = [au - au * ku, av - av * kv, au + (1.0 - au) * ku, av + (1.0 - av) * kv];
                let map = |a: f64, b: f64| { let (x, y) = h.apply(a, b); [x, y] };
                quad = [map(r[0], r[1]), map(r[2], r[1]), map(r[2], r[3]), map(r[0], r[3])];
                pivot = if m.alt { g.pivot0 } else { carry_pivot(q0, quad, g.pivot0) };
            }
            Hit::Point(i) => {
                let Some(h) = Homography::square_to_quad(q0) else { return false };
                let Some(inv) = h.inverse() else { return false };
                let (u, v) = inv.apply(p[0], p[1]);
                if let (Some(mesh), Some(m0)) = (&mut self.warp.mesh, &g.warp0.mesh) {
                    if u.is_finite() && v.is_finite() && i < mesh.pts.len() {
                        mesh.pts = m0.pts.clone();
                        mesh.pts[i] = [u, v];
                    }
                }
                return true;
            }
        }
        if quad.iter().all(|c| c[0].is_finite() && c[1].is_finite()) && Homography::square_to_quad(quad).is_some() {
            self.warp.quad = quad;
            self.pivot = pivot;
        }
        true
    }

    /// The drag ended. True if it changed something.
    pub fn release(&mut self) -> bool {
        self.drag.take().is_some_and(|d| d.moved)
    }

    /// Nudge by whole pixels (arrow keys).
    pub fn nudge(&mut self, dx: f64, dy: f64) {
        self.remember();
        self.warp.quad = self.warp.quad.map(|c| [c[0] + dx, c[1] + dy]);
        self.pivot = add(self.pivot, [dx, dy]);
    }

    pub fn flip(&mut self, horizontal: bool) {
        self.remember();
        let q = self.warp.quad;
        self.warp.quad = if horizontal { [q[1], q[0], q[3], q[2]] } else { [q[3], q[2], q[1], q[0]] };
        if let Some(m) = &mut self.warp.mesh {
            for p in &mut m.pts { if horizontal { p[0] = 1.0 - p[0]; } else { p[1] = 1.0 - p[1]; } }
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
        let (sn, cs) = degrees.to_radians().sin_cos();
        let c = self.pivot;
        self.warp.quad = self.warp.quad.map(|q| { let (x, y) = (q[0] - c[0], q[1] - c[1]); [c[0] + x * cs - y * sn, c[1] + x * sn + y * cs] });
    }

    pub fn reset(&mut self) {
        self.remember();
        let t = self.targets.clone();
        *self = Self { undo: std::mem::take(&mut self.undo), ..Self::new(self.src, t) };
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
        if self.warp.mesh.as_ref().is_some_and(|m| !m.is_identity()) { return None; }
        let q = self.warp.quad;
        if len(sub(add(q[0], q[2]), add(q[1], q[3]))) > 1e-6 * (1.0 + len(q[0])) { return None; }
        let (sw, sh) = ((self.src.2 - self.src.0) as f64, (self.src.3 - self.src.1) as f64);
        let c = mid(q[0], q[2]);
        let (e0, e1) = (sub(q[1], q[0]), sub(q[3], q[0]));
        let angle = e0[1].atan2(e0[0]);
        let (sn, cs) = (-angle).sin_cos();
        let (a, b) = (e1[0] * cs - e1[1] * sn, e1[0] * sn + e1[1] * cs);
        let skew = if b.abs() > 1e-9 { (a / b).atan().to_degrees() } else { 0.0 };
        let round = |v: f64| (v * 100.0).round() / 100.0;
        Some([round(c[0]), round(c[1]), round(len(e0) / sw * 100.0), round(b / sh * 100.0), round(angle.to_degrees()), round(skew)])
    }

    pub fn set_numbers(&mut self, n: TransformNumbers) {
        let (sw, sh) = ((self.src.2 - self.src.0) as f64, (self.src.3 - self.src.1) as f64);
        let (w, h) = (sw * n[2] / 100.0, sh * n[3] / 100.0);
        if w.abs() < 0.01 || h.abs() < 0.01 || n[5].abs() >= 89.0 { return; }
        self.remember();
        let (sn, cs) = n[4].to_radians().sin_cos();
        let rot = |v: P| [v[0] * cs - v[1] * sn, v[0] * sn + v[1] * cs];
        let ex = rot([w, 0.0]);
        let ey = rot([n[5].to_radians().tan() * h, h]);
        let c = [n[0], n[1]];
        let q0 = [c[0] - (ex[0] + ey[0]) / 2.0, c[1] - (ex[1] + ey[1]) / 2.0];
        let old = self.warp.quad;
        self.warp.quad = [q0, add(q0, ex), add(add(q0, ex), ey), add(q0, ey)];
        self.pivot = carry_pivot(old, self.warp.quad, self.pivot);
    }

    /// The source's corners (for tests).
    #[cfg(test)]
    pub fn source_corners(&self) -> [P; 4] { aurastudio_transform::corners(self.src) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: P, b: P) -> bool { len(sub(a, b)) < 1e-6 }

    #[test]
    fn corners_scale_from_the_opposite_one_proportionally() {
        let mut x = Xform::new((0, 0, 100, 50), vec![1]);
        let hit = x.hit([100.0, 50.0], 5.0, TransformMode::Free);
        assert_eq!(hit, Hit::Corner(2));
        x.press(hit, [100.0, 50.0]);
        x.drag_to([200.0, 60.0], Mods::default(), TransformMode::Free, true);
        assert!(near(x.warp.quad[0], [0.0, 0.0]));
        assert!(near(x.warp.quad[2], [200.0, 100.0]), "{:?}", x.warp.quad);
        // Shift: free.
        x.drag_to([200.0, 60.0], Mods { shift: true, ..Default::default() }, TransformMode::Free, true);
        assert!(near(x.warp.quad[2], [200.0, 60.0]));
        assert!(x.release());
        assert!(x.step_back());
        assert_eq!(x.warp, Warp::identity((0, 0, 100, 50)));
        assert!(x.step_forward());
    }

    #[test]
    fn move_rotate_skew_distort_and_numbers() {
        let mut x = Xform::new((0, 0, 100, 100), vec![1]);
        let m = Mods::default();
        assert_eq!(x.hit([50.0, 50.0], 5.0, TransformMode::Free), Hit::Pivot);
        assert_eq!(x.hit([30.0, 30.0], 5.0, TransformMode::Free), Hit::Inside);
        x.press(Hit::Inside, [30.0, 30.0]);
        x.drag_to([40.0, 35.0], m, TransformMode::Free, true);
        x.release();
        assert!(near(x.warp.quad[0], [10.0, 5.0]));
        assert!(near(x.pivot, [60.0, 55.0]));
        // A quarter turn around the pivot.
        assert_eq!(x.hit([200.0, 55.0], 5.0, TransformMode::Free), Hit::Outside);
        x.press(Hit::Outside, [160.0, 55.0]);
        x.drag_to([60.0, 155.0], m, TransformMode::Free, true);
        x.release();
        let n = x.numbers().unwrap();
        assert!((n[4] - 90.0).abs() < 1e-6 && (n[2] - 100.0).abs() < 1e-6, "{n:?}");
        x.set_numbers([60.0, 55.0, 50.0, 100.0, 0.0, 0.0]);
        assert!(near(x.warp.quad[0], [35.0, 5.0]) && near(x.warp.quad[2], [85.0, 105.0]), "{:?}", x.warp.quad);
        // Skew: the top edge slides.
        assert_eq!(x.hit([45.0, 5.0], 5.0, TransformMode::Free), Hit::Side(0));
        x.press(Hit::Side(0), [45.0, 5.0]);
        x.drag_to([65.0, 9.0], m, TransformMode::Free, true);
        x.release();
        assert!(near(x.warp.quad[0], [55.0, 5.0]) && near(x.warp.quad[3], [35.0, 105.0]));
        assert!(x.numbers().unwrap()[5].abs() > 1.0);
        // Ctrl+corner: distort, never folding over.
        x.press(Hit::Corner(2), x.warp.quad[2]);
        x.drag_to([0.0, -500.0], Mods { ctrl: true, ..Default::default() }, TransformMode::Free, true);
        assert!(aurastudio_transform::convex(&x.warp.quad));
        assert!(x.numbers().is_none());
        x.release();
        x.reset();
        assert_eq!(x.warp.quad, x.source_corners());
    }

    #[test]
    fn flips_rotations_and_the_grid() {
        let mut x = Xform::new((0, 0, 100, 40), vec![1]);
        x.flip(true);
        assert!(near(x.warp.quad[0], [100.0, 0.0]));
        x.flip(true);
        x.rotate(90.0);
        assert!(near(x.warp.quad[0], [70.0, -30.0]), "{:?}", x.warp.quad);
        x.reset();
        x.set_grid(2);
        assert_eq!(x.mesh_points().len(), 9);
        let hit = x.hit([50.0, 20.0], 3.0, TransformMode::Warp);
        assert_eq!(hit, Hit::Point(4));
        x.press(hit, [50.0, 20.0]);
        x.drag_to([50.0, 30.0], Mods::default(), TransformMode::Warp, true);
        let p = x.warp.map(x.src, 50.0, 20.0).unwrap();
        assert!((p.1 - 30.0).abs() < 1e-6);
        assert!(x.numbers().is_none());
    }
}

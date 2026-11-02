//! The projective map (3×3) and the quad a rectangle's corners go to.

use aurastudio_core::{Point, Rect};

/// Four corners, clockwise from top-left.
pub type Quad = [Point; 4];

/// 3×3 projective matrix, row-major: `[x', y', w'] = H · [x, y, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    #[inline]
    pub fn apply(&self, p: Point) -> Point {
        let m = &self.0;
        let w = m[6] * p.x + m[7] * p.y + m[8];
        Point::new((m[0] * p.x + m[1] * p.y + m[2]) / w, (m[3] * p.x + m[4] * p.y + m[5]) / w)
    }

    /// `self` after `o` (`o` applied first).
    pub fn mul(&self, o: &Homography) -> Homography {
        let (a, b) = (&self.0, &o.0);
        Homography(std::array::from_fn(|k| { let (i, j) = (k / 3, k % 3); (0..3).map(|n| a[i * 3 + n] * b[n * 3 + j]).sum() }))
    }

    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.0;
        let c = [
            m[4] * m[8] - m[5] * m[7], m[2] * m[7] - m[1] * m[8], m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8], m[0] * m[8] - m[2] * m[6], m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6], m[1] * m[6] - m[0] * m[7], m[0] * m[4] - m[1] * m[3],
        ];
        let det = m[0] * c[0] + m[1] * c[3] + m[2] * c[6];
        if det.abs() < 1e-12 || !det.is_finite() { return None; }
        Some(Homography(c.map(|v| v / det)))
    }

    /// Without perspective (move, scale, turn, skew): no division per
    /// pixel needed.
    pub fn is_affine(&self) -> bool { let m = &self.0; m[6].abs() < 1e-15 && m[7].abs() < 1e-15 && (m[8] - 1.0).abs() < 1e-12 }

    /// Unit square → quad (corners (0,0), (1,0), (1,1), (0,1)).
    pub fn square_to_quad(q: Quad) -> Option<Homography> {
        let [p0, p1, p2, p3] = q;
        let (sx, sy) = (p0.x - p1.x + p2.x - p3.x, p0.y - p1.y + p2.y - p3.y);
        if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            let h = Homography([p1.x - p0.x, p3.x - p0.x, p0.x, p1.y - p0.y, p3.y - p0.y, p0.y, 0.0, 0.0, 1.0]);
            return h.inverse().map(|_| h);
        }
        let (dx1, dx2, dy1, dy2) = (p1.x - p2.x, p3.x - p2.x, p1.y - p2.y, p3.y - p2.y);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 { return None; }
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        let m = Homography([p1.x - p0.x + g * p1.x, p3.x - p0.x + h * p3.x, p0.x, p1.y - p0.y + g * p1.y, p3.y - p0.y + h * p3.y, p0.y, g, h, 1.0]);
        m.inverse().map(|_| m)
    }

    /// Rectangle → quad.
    pub fn rect_to_quad(r: Rect, quad: Quad) -> Option<Homography> {
        let (w, h) = (r.w() as f64, r.h() as f64);
        if w <= 0.0 || h <= 0.0 { return None; }
        let to_unit = Homography([1.0 / w, 0.0, -r.x0 as f64 / w, 0.0, 1.0 / h, -r.y0 as f64 / h, 0.0, 0.0, 1.0]);
        Some(Self::square_to_quad(quad)?.mul(&to_unit))
    }
}

/// The corners of a rectangle, clockwise from top-left.
pub fn corners(r: Rect) -> Quad {
    let (x0, y0, x1, y1) = (r.x0 as f64, r.y0 as f64, r.x1 as f64, r.y1 as f64);
    [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]
}

/// Turn at each corner (all one sign: convex).
fn turns(q: &Quad) -> [f64; 4] {
    std::array::from_fn(|k| (q[(k + 1) % 4] - q[k]).cross(q[(k + 2) % 4] - q[(k + 1) % 4]))
}

pub fn convex(q: &Quad) -> bool {
    let t = turns(q);
    t.iter().all(|v| *v > 0.0) || t.iter().all(|v| *v < 0.0)
}

/// Moving corners from `from` toward `to`, as far as the quad stays convex
/// (a distort stops where it would fold over, as in Photoshop).
pub fn keep_convex(from: Quad, to: Quad) -> Quad {
    let t0 = turns(&from);
    let sign = t0[0].signum();
    let min_turn = t0.iter().map(|t| t.abs()).fold(f64::INFINITY, f64::min) * 1e-3;
    let ok = |q: &Quad| turns(q).iter().all(|t| t * sign > min_turn);
    if !(min_turn.is_finite() && min_turn > 0.0 && convex(&from)) || ok(&to) { return to; }
    let lerp = |t: f64| -> Quad { std::array::from_fn(|k| from[k].lerp(to[k], t)) };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if ok(&lerp(mid)) { lo = mid } else { hi = mid }
    }
    lerp(lo)
}

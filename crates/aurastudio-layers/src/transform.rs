//! Transforms: where the four corners of a source rectangle land (a quad,
//! a projective map: move, scale, rotate, skew, distort, perspective),
//! optionally bent by a smooth grid first (warp). Pixels are resampled
//! premultiplied (nearest, bilinear or Photoshop's bicubic) from a
//! pyramid, so big reductions don't alias, on all cores.

use std::sync::atomic::{AtomicUsize, Ordering};

use aurastudio_canvas::Rect;

/// 3×3 projective matrix, row-major: `[x', y', w'] = H · [x, y, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w)
    }

    pub fn mul(&self, o: &Homography) -> Homography {
        let (a, b) = (&self.0, &o.0);
        let mut r = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                r[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
            }
        }
        Homography(r)
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

    /// Unit square -> quad (corners (0,0), (1,0), (1,1), (0,1)).
    pub fn square_to_quad(q: [[f64; 2]; 4]) -> Option<Homography> {
        let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
        let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
        if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            let h = Homography([x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0, 1.0]);
            return h.inverse().map(|_| h);
        }
        let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 { return None; }
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        let m = Homography([x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]);
        m.inverse().map(|_| m)
    }

    /// Rectangle (x0, y0, x1, y1) -> quad (clockwise from top-left).
    pub fn rect_to_quad(r: [f64; 4], quad: [[f64; 2]; 4]) -> Option<Homography> {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if w <= 0.0 || h <= 0.0 { return None; }
        let to_unit = Homography([1.0 / w, 0.0, -r[0] / w, 0.0, 1.0 / h, -r[1] / h, 0.0, 0.0, 1.0]);
        Some(Self::square_to_quad(quad)?.mul(&to_unit))
    }
}

/// The corners of a rectangle, clockwise from top-left.
pub fn corners(r: Rect) -> [[f64; 2]; 4] {
    let (x0, y0, x1, y1) = (r.0 as f64, r.1 as f64, r.2 as f64, r.3 as f64);
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Turn at each corner (all one sign: convex).
fn turns(q: &[[f64; 2]; 4]) -> [f64; 4] {
    std::array::from_fn(|k| {
        let (a, b, c) = (q[k], q[(k + 1) % 4], q[(k + 2) % 4]);
        (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
    })
}

pub fn convex(q: &[[f64; 2]; 4]) -> bool {
    let t = turns(q);
    t.iter().all(|v| *v > 0.0) || t.iter().all(|v| *v < 0.0)
}

/// Moving corners from `from` toward `to`, as far as the quad stays convex
/// (a distort stops where it would fold over, as in Photoshop).
pub fn keep_convex(from: [[f64; 2]; 4], to: [[f64; 2]; 4]) -> [[f64; 2]; 4] {
    let t0 = turns(&from);
    let sign = t0[0].signum();
    let min_turn = t0.iter().map(|t| t.abs()).fold(f64::INFINITY, f64::min) * 1e-3;
    let ok = |q: &[[f64; 2]; 4]| turns(q).iter().all(|t| t * sign > min_turn);
    if !(min_turn.is_finite() && min_turn > 0.0 && convex(&from)) || ok(&to) { return to; }
    let lerp = |t: f64| -> [[f64; 2]; 4] { std::array::from_fn(|k| [from[k][0] + (to[k][0] - from[k][0]) * t, from[k][1] + (to[k][1] - from[k][1]) * t]) };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if ok(&lerp(mid)) { lo = mid } else { hi = mid }
    }
    lerp(lo)
}

/// How pixels are resampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interp {
    /// Hard pixels (pixel art).
    Nearest,
    Bilinear,
    /// Photoshop's (Keys a = -0.75): sharp, little ringing.
    #[default]
    Bicubic,
}

/// The four weights of Keys' cubic, a = -0.75, `t` past the second pixel.
fn bicubic(t: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let near = |x: f32| (A + 2.0) * x * x * x - (A + 3.0) * x * x + 1.0;
    let far = |x: f32| A * (x * x * x - 5.0 * x * x + 8.0 * x - 4.0);
    [far(1.0 + t), near(t), near(1.0 - t), far(2.0 - t)]
}

/// A grid bending the source before the quad (CSP's mesh): `cols` ×
/// `rows` cells, control points in the source's unit square. A point
/// moves only the four cells around it: the points around stay where they
/// are, and so does what is under them (each point's move fades out over
/// its cells, smoothly: 3t² - 2t³).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub cols: usize,
    pub rows: usize,
    /// (cols + 1) × (rows + 1), row by row.
    pub pts:  Vec<[f64; 2]>,
}

impl Mesh {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.clamp(1, 16), rows.clamp(1, 16));
        let pts = (0..=rows).flat_map(|j| (0..=cols).map(move |i| [i as f64 / cols as f64, j as f64 / rows as f64])).collect();
        Self { cols, rows, pts }
    }

    pub fn is_identity(&self) -> bool { *self == Mesh::new(self.cols, self.rows) }

    pub fn at(&self, i: usize, j: usize) -> [f64; 2] { self.pts[j * (self.cols + 1) + i] }

    /// How far point (i, j) moved from its place.
    fn moved(&self, i: usize, j: usize) -> [f64; 2] {
        let p = self.at(i, j);
        [p[0] - i as f64 / self.cols as f64, p[1] - j as f64 / self.rows as f64]
    }

    /// Where unit point (u, v) of the source goes (still unit space).
    /// Past the edges, as the nearest edge moved.
    pub fn eval(&self, u: f64, v: f64) -> [f64; 2] {
        let (x, y) = (u * self.cols as f64, v * self.rows as f64);
        let i = (x.floor() as isize).clamp(0, self.cols as isize - 1) as usize;
        let j = (y.floor() as isize).clamp(0, self.rows as isize - 1) as usize;
        let smooth = |t: f64| { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) };
        let (su, sv) = (smooth(x - i as f64), smooth(y - j as f64));
        let mut out = [u, v];
        for (di, dj, w) in [(0, 0, (1.0 - su) * (1.0 - sv)), (1, 0, su * (1.0 - sv)), (0, 1, (1.0 - su) * sv), (1, 1, su * sv)] {
            let d = self.moved(i + di, j + dj);
            out[0] += d[0] * w;
            out[1] += d[1] * w;
        }
        out
    }
}

/// A whole transform of the source rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    /// Where the source's corners go, clockwise from top-left.
    pub quad: [[f64; 2]; 4],
    pub mesh: Option<Mesh>,
}

impl Warp {
    pub fn identity(src: Rect) -> Self { Self { quad: corners(src), mesh: None } }

    /// Source canvas point -> destination, None if degenerate.
    pub fn map(&self, src: Rect, x: f64, y: f64) -> Option<(f64, f64)> {
        let h = Homography::square_to_quad(self.quad)?;
        let (sw, sh) = ((src.2 - src.0) as f64, (src.3 - src.1) as f64);
        let (mut u, mut v) = ((x - src.0 as f64) / sw, (y - src.1 as f64) / sh);
        if let Some(m) = &self.mesh { [u, v] = m.eval(u, v); }
        Some(h.apply(u, v))
    }

    /// Bounds of the result (a pixel more for the filter), unclipped.
    pub fn bounds(&self, src: Rect) -> Option<(f64, f64, f64, f64)> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let n = if self.mesh.is_some() { 32 } else { 1 };
        let (sw, sh) = ((src.2 - src.0) as f64, (src.3 - src.1) as f64);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            for (u, v) in [(t, 0.0), (t, 1.0), (0.0, t), (1.0, t)] {
                let (x, y) = self.map(src, src.0 as f64 + u * sw, src.1 as f64 + v * sh)?;
                x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y);
            }
        }
        // A bent grid: each cell's points stay in the cell's box grown by
        // its corners' moves (the quad maps a box to a convex quad).
        if let Some(m) = &self.mesh {
            let h = Homography::square_to_quad(self.quad)?;
            for j in 0..m.rows {
                for i in 0..m.cols {
                    let ds = [m.moved(i, j), m.moved(i + 1, j), m.moved(i, j + 1), m.moved(i + 1, j + 1)];
                    let lo = [ds.iter().map(|d| d[0]).fold(f64::MAX, f64::min), ds.iter().map(|d| d[1]).fold(f64::MAX, f64::min)];
                    let hi = [ds.iter().map(|d| d[0]).fold(f64::MIN, f64::max), ds.iter().map(|d| d[1]).fold(f64::MIN, f64::max)];
                    let (u0, v0) = (i as f64 / m.cols as f64 + lo[0], j as f64 / m.rows as f64 + lo[1]);
                    let (u1, v1) = ((i + 1) as f64 / m.cols as f64 + hi[0], (j + 1) as f64 / m.rows as f64 + hi[1]);
                    for (u, v) in [(u0, v0), (u1, v0), (u0, v1), (u1, v1)] {
                        let (x, y) = h.apply(u, v);
                        x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y);
                    }
                }
            }
        }
        let ok = [x0, y0, x1, y1].iter().all(|v| v.is_finite());
        ok.then_some((x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0))
    }

    /// Bounds clipped to a `w` × `h` canvas (whole pixels).
    pub fn dest_rect(&self, src: Rect, w: u32, h: u32) -> Option<Rect> {
        let (x0, y0, x1, y1) = self.bounds(src)?;
        let c = |v: f64, max: u32| (v.max(0.0).min(max as f64)) as u32;
        let r = (c(x0.floor(), w), c(y0.floor(), h), c(x1.ceil(), w), c(y1.ceil(), h));
        (r.0 < r.2 && r.1 < r.3).then_some(r)
    }
}

/// Premultiplied pixels, 16 bits a channel.
struct Level {
    w:  usize,
    h:  usize,
    px: Vec<[u16; 4]>,
}

/// What is transformed: its pixels at full size and halved down to a few
/// pixels.
pub struct Source {
    rect:   Rect,
    levels: Vec<Level>,
}

impl Source {
    /// From straight RGBA rows of `rect`.
    pub fn new(rect: Rect, rgba: &[u8]) -> Self {
        let (w, h) = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
        let px: Vec<[u16; 4]> = rgba.chunks_exact(4).map(|p| {
            let a = p[3] as u32;
            let pm = |c: u8| ((c as u32 * a * 65535 + 32512) / 65025) as u16;
            [pm(p[0]), pm(p[1]), pm(p[2]), (a * 257) as u16]
        }).collect();
        let mut levels = vec![Level { w, h, px }];
        while levels.len() < 14 {
            let l = levels.last().unwrap();
            if l.w <= 1 && l.h <= 1 { break; }
            let (nw, nh) = (l.w.div_ceil(2), l.h.div_ceil(2));
            let mut px = Vec::with_capacity(nw * nh);
            for y in 0..nh {
                for x in 0..nw {
                    let mut s = [0u32; 4];
                    let mut n = 0;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = (2 * x + dx, 2 * y + dy);
                        if sx < l.w && sy < l.h {
                            let p = l.px[sy * l.w + sx];
                            for c in 0..4 { s[c] += p[c] as u32; }
                            n += 1;
                        }
                    }
                    px.push(s.map(|v| ((v + n / 2) / n) as u16));
                }
            }
            levels.push(Level { w: nw, h: nh, px });
        }
        Self { rect, levels }
    }

    pub fn rect(&self) -> Rect { self.rect }

    /// Premultiplied sample at level `l`, (x, y) in its pixels (centers at
    /// k + 0.5), clamped to its edge.
    fn sample(&self, l: usize, x: f32, y: f32, interp: Interp) -> [f32; 4] {
        let lv = &self.levels[l];
        let (mx, my) = (lv.w as isize - 1, lv.h as isize - 1);
        let (u, v) = (x - 0.5, y - 0.5);
        // Away from the edges no tap needs clamping.
        let (fi, fj) = (u.floor() as isize, v.floor() as isize);
        let inner = fi >= 1 && fj >= 1 && fi + 2 <= mx && fj + 2 <= my;
        let px = |i: isize, j: isize| -> [f32; 4] {
            let k = if inner { j as usize * lv.w + i as usize } else { (j.clamp(0, my) as usize) * lv.w + i.clamp(0, mx) as usize };
            let p = lv.px[k];
            [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
        };
        let mut acc = [0f32; 4];
        match interp {
            Interp::Nearest => return px((u + 0.5).floor() as isize, (v + 0.5).floor() as isize).map(|c| c / 65535.0),
            Interp::Bilinear => {
                let (i, j) = (u.floor(), v.floor());
                let (fx, fy) = (u - i, v - j);
                let (i, j) = (i as isize, j as isize);
                for (dj, wy) in [(0, 1.0 - fy), (1, fy)] {
                    for (di, wx) in [(0, 1.0 - fx), (1, fx)] {
                        let p = px(i + di, j + dj);
                        for c in 0..4 { acc[c] += p[c] * wx * wy; }
                    }
                }
            }
            Interp::Bicubic => {
                let (i, j) = (u.floor(), v.floor());
                let (wx, wy) = (bicubic(u - i), bicubic(v - j));
                let (i, j) = (i as isize, j as isize);
                for (b, wyb) in wy.iter().enumerate() {
                    for (a, wxa) in wx.iter().enumerate() {
                        let k = wxa * wyb;
                        let p = px(i - 1 + a as isize, j - 1 + b as isize);
                        for c in 0..4 { acc[c] += p[c] * k; }
                    }
                }
            }
        }
        // Ringing kept in range: no color without alpha.
        let a = (acc[3] / 65535.0).clamp(0.0, 1.0);
        [(acc[0] / 65535.0).clamp(0.0, a), (acc[1] / 65535.0).clamp(0.0, a), (acc[2] / 65535.0).clamp(0.0, a), a]
    }

    /// The sample for source canvas point (x, y), `scale` source pixels per
    /// destination pixel, its footprint `(fu, fv)` (source pixels across
    /// and down): the level by the scale, the outline anti-aliased.
    /// The level to sample `scale` source pixels per destination pixel
    /// from, and its size factor.
    fn level(&self, scale: f64, interp: Interp) -> (usize, f64) {
        let l = if interp == Interp::Nearest || scale <= 1.0 || !scale.is_finite() { 0 } else { (scale.log2().floor() as usize).min(self.levels.len() - 1) };
        (l, 1.0 / (1u32 << l) as f64)
    }

    fn at(&self, x: f64, y: f64, (l, k): (usize, f64), foot: (f64, f64), interp: Interp) -> Option<[f32; 4]> {
        let r = self.rect;
        let (u, v) = (x - r.0 as f64, y - r.1 as f64);
        let (w, h) = ((r.2 - r.0) as f64, (r.3 - r.1) as f64);
        if u < -1.0 || v < -1.0 || u > w + 1.0 || v > h + 1.0 { return None; }
        let mut s = self.sample(l, (u * k) as f32, (v * k) as f32, interp);
        // Well inside: all covered.
        let inside = u > foot.0 && v > foot.1 && u < w - foot.0 && v < h - foot.1;
        if interp != Interp::Nearest && !inside {
            // How much of the destination pixel the source covers.
            let cu = (u.min(w - u) / foot.0.max(1e-9) + 0.5).clamp(0.0, 1.0);
            let cv = (v.min(h - v) / foot.1.max(1e-9) + 0.5).clamp(0.0, 1.0);
            let c = (cu * cv) as f32;
            for x in &mut s { *x *= c; }
        } else if u < 0.0 || v < 0.0 || u >= w || v >= h {
            return None;
        }
        (s[3] > 0.0).then_some(s)
    }
}

/// Premultiplied `s` over straight RGBA8 `d`.
fn over(d: &mut [u8], s: [f32; 4]) {
    // Opaque, or over nothing: the sample as it is.
    if s[3] >= 0.9995 || d[3] == 0 {
        let inv = if s[3] > 0.0 { 255.0 / s[3] } else { 0.0 };
        for c in 0..3 { d[c] = (s[c] * inv + 0.5).clamp(0.0, 255.0) as u8; }
        d[3] = (s[3] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        return;
    }
    let da = d[3] as f32 / 255.0;
    let a = s[3] + da * (1.0 - s[3]);
    if a <= 0.0 { return; }
    for c in 0..3 {
        let v = s[c] + d[c] as f32 / 255.0 * da * (1.0 - s[3]);
        d[c] = ((v / a) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    }
    d[3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
}

/// Threads to use (the brushes stay on one; a transform is one big job).
fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZero::get).clamp(1, 16)
}

/// Bands of `rows` rows of `dst`, `w` pixels wide, `run` on each (y0,
/// band) from all cores.
fn bands(dst: &mut [u8], w: usize, rows: usize, run: &(dyn Fn(usize, &mut [u8]) + Sync)) {
    let stride = w * 4 * rows;
    if stride == 0 { return; }
    let chunks: Vec<(usize, &mut [u8])> = dst.chunks_mut(stride).enumerate().map(|(i, c)| (i * rows, c)).collect();
    let n = chunks.len();
    let next = AtomicUsize::new(0);
    type Slot<'a> = std::sync::Mutex<Option<(usize, &'a mut [u8])>>;
    let slots: Vec<Slot> = chunks.into_iter().map(|c| std::sync::Mutex::new(Some(c))).collect();
    std::thread::scope(|sc| {
        for _ in 0..threads().min(n) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n { break; }
                let job = slots[i].lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                if let Some((y0, band)) = job { run(y0, band); }
            });
        }
    });
}

/// Paint `src` transformed by `warp` over `dst` (straight RGBA rows of
/// `rect`, canvas pixels).
pub fn render(src: &Source, warp: &Warp, interp: Interp, rect: Rect, dst: &mut [u8]) {
    let (rw, rh) = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
    if rw == 0 || rh == 0 || dst.len() != rw * rh * 4 { return; }
    let s = src.rect;
    if s.2 <= s.0 || s.3 <= s.1 { return; }
    match &warp.mesh {
        Some(m) if !m.is_identity() => render_mesh(src, warp, m, interp, rect, dst),
        _ => render_quad(src, warp, interp, rect, dst),
    }
}

fn render_quad(src: &Source, warp: &Warp, interp: Interp, rect: Rect, dst: &mut [u8]) {
    let s = src.rect;
    let Some(h) = Homography::rect_to_quad([s.0 as f64, s.1 as f64, s.2 as f64, s.3 as f64], warp.quad) else { return };
    let Some(inv) = h.inverse() else { return };
    let m = inv.0;
    // Move, scale, turn, skew: no division per pixel.
    let affine = m[6].abs() < 1e-15 && m[7].abs() < 1e-15 && (m[8] - 1.0).abs() < 1e-12;
    let rw = (rect.2 - rect.0) as usize;
    let (sx0, sy0, sx1, sy1) = (s.0 as f64 - 2.0, s.1 as f64 - 2.0, s.2 as f64 + 2.0, s.3 as f64 + 2.0);
    const B: usize = 16;
    bands(dst, rw, B, &|y0, band| {
        let rows = band.len() / (rw * 4);
        for bx in (0..rw).step_by(B) {
            let bw = B.min(rw - bx);
            let (x0, yb) = ((rect.0 as usize + bx) as f64, (rect.1 as usize + y0) as f64);
            // A block whose corners all map off the source on one side shows
            // nothing of it (the map keeps straight lines straight).
            let cs = [(x0, yb), (x0 + bw as f64, yb), (x0, yb + rows as f64), (x0 + bw as f64, yb + rows as f64)].map(|(x, y)| inv.apply(x, y));
            if cs.iter().all(|c| c.0 < sx0) || cs.iter().all(|c| c.0 > sx1) || cs.iter().all(|c| c.1 < sy0) || cs.iter().all(|c| c.1 > sy1) { continue; }
            // The scale and footprint of this block, at its center.
            let (cx, cy) = (x0 + bw as f64 / 2.0, yb + rows as f64 / 2.0);
            let (o, dx, dy) = (inv.apply(cx, cy), inv.apply(cx + 1.0, cy), inv.apply(cx, cy + 1.0));
            let (ju, jv) = ((dx.0 - o.0, dx.1 - o.1), (dy.0 - o.0, dy.1 - o.1));
            let scale = (ju.0 * jv.1 - ju.1 * jv.0).abs().sqrt();
            let foot = (ju.0.abs() + jv.0.abs(), ju.1.abs() + jv.1.abs());
            let level = src.level(scale, interp);
            for y in 0..rows {
                // Along the row the homogeneous point moves by fixed steps.
                let (px, py) = (x0 + 0.5, yb + y as f64 + 0.5);
                let (mut hx, mut hy, mut hw) = (m[0] * px + m[1] * py + m[2], m[3] * px + m[4] * py + m[5], m[6] * px + m[7] * py + m[8]);
                for x in bx..bx + bw {
                    let (u, v) = if affine { (hx, hy) } else { let r = 1.0 / hw; (hx * r, hy * r) };
                    hx += m[0];
                    hy += m[3];
                    hw += m[6];
                    if !(u.is_finite() && v.is_finite()) { continue; }
                    if let Some(c) = src.at(u, v, level, foot, interp) {
                        let o = (y * rw + x) * 4;
                        over(&mut band[o..o + 4], c);
                    }
                }
            }
        }
    });
}

fn render_mesh(src: &Source, warp: &Warp, m: &Mesh, interp: Interp, rect: Rect, dst: &mut [u8]) {
    let s = src.rect;
    let Some(h) = Homography::square_to_quad(warp.quad) else { return };
    let (sw, sh) = ((s.2 - s.0) as f64, (s.3 - s.1) as f64);
    // Fine triangles: about 10 destination pixels a side.
    let (Some(b), rw) = (warp.bounds(s), (rect.2 - rect.0) as usize) else { return };
    // (A part far past what is drawn needs no detail.)
    let shown = (rect.2 - rect.0).max(rect.3 - rect.1) as f64 * 4.0;
    let side = (b.2 - b.0).max(b.3 - b.1).min(shown);
    let n = ((side / 10.0).ceil() as usize).clamp(m.cols.max(m.rows) * 4, 512);
    let (nu, nv) = (n, (n as f64 * sh / sw).ceil().clamp(2.0, 768.0) as usize);
    let nu = nu.max(2);
    // Vertices: destination point, source canvas point.
    let mut verts = Vec::with_capacity((nu + 1) * (nv + 1));
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f64 / nu as f64, j as f64 / nv as f64);
            let p = m.eval(u, v);
            let d = h.apply(p[0], p[1]);
            verts.push(((d.0, d.1), (s.0 as f64 + u * sw, s.1 as f64 + v * sh)));
        }
    }
    let at = |i: usize, j: usize| verts[j * (nu + 1) + i];
    let mut tris = Vec::with_capacity(nu * nv * 2);
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    let rh = (rect.3 - rect.1) as usize;
    const B: usize = 32;
    // Triangles by the bands they cross.
    let nb = rh.div_ceil(B);
    let mut by_band: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (k, t) in tris.iter().enumerate() {
        let (y0, y1) = (t.iter().map(|v| v.0 .1).fold(f64::MAX, f64::min), t.iter().map(|v| v.0 .1).fold(f64::MIN, f64::max));
        let (x0, x1) = (t.iter().map(|v| v.0 .0).fold(f64::MAX, f64::min), t.iter().map(|v| v.0 .0).fold(f64::MIN, f64::max));
        if !(y0.is_finite() && y1.is_finite()) { continue; }
        if x1 < rect.0 as f64 || x0 > rect.2 as f64 || y1 < rect.1 as f64 || y0 > rect.3 as f64 { continue; }
        let a = ((y0 - rect.1 as f64).floor().max(0.0) as usize / B).min(nb);
        let z = ((y1 - rect.1 as f64).ceil().max(0.0) as usize / B + 1).min(nb);
        for v in &mut by_band[a..z] { v.push(k); }
    }
    bands(dst, rw, B, &|y0, band| {
        let rows = band.len() / (rw * 4);
        // Samples first (a fold covers a pixel twice: the later wins), then
        // over the pixels.
        let mut samples: Vec<Option<[f32; 4]>> = vec![None; rw * rows];
        for &k in &by_band[y0 / B] {
            let [(p0, s0), (p1, s1), (p2, s2)] = tris[k];
            let det = (p1.0 - p0.0) * (p2.1 - p0.1) - (p2.0 - p0.0) * (p1.1 - p0.1);
            if det.abs() < 1e-9 { continue; }
            // Destination -> source, affine on the triangle.
            let ix = [(p2.1 - p0.1) / det, -(p2.0 - p0.0) / det, -(p1.1 - p0.1) / det, (p1.0 - p0.0) / det];
            let du = ((s1.0 - s0.0) * ix[0] + (s2.0 - s0.0) * ix[2], (s1.0 - s0.0) * ix[1] + (s2.0 - s0.0) * ix[3]);
            let dv = ((s1.1 - s0.1) * ix[0] + (s2.1 - s0.1) * ix[2], (s1.1 - s0.1) * ix[1] + (s2.1 - s0.1) * ix[3]);
            let scale = (du.0 * dv.1 - du.1 * dv.0).abs().sqrt();
            let foot = (du.0.abs() + du.1.abs(), dv.0.abs() + dv.1.abs());
            let level = src.level(scale, interp);
            let (minx, maxx) = (p0.0.min(p1.0).min(p2.0), p0.0.max(p1.0).max(p2.0));
            let (miny, maxy) = (p0.1.min(p1.1).min(p2.1), p0.1.max(p1.1).max(p2.1));
            let by0 = (rect.1 as usize + y0) as f64;
            let ya = ((miny - by0 - 0.5).ceil().max(0.0) as usize).min(rows);
            let yb = ((maxy - by0 - 0.5).floor() as i64 + 1).clamp(0, rows as i64) as usize;
            let xa = ((minx - rect.0 as f64 - 0.5).ceil().max(0.0) as usize).min(rw);
            let xb = ((maxx - rect.0 as f64 - 0.5).floor() as i64 + 1).clamp(0, rw as i64) as usize;
            for y in ya..yb {
                let py = by0 + y as f64 + 0.5;
                for x in xa..xb {
                    let px = rect.0 as f64 + x as f64 + 0.5;
                    let (ex, ey) = (px - p0.0, py - p0.1);
                    let l1 = ix[0] * ex + ix[1] * ey;
                    let l2 = ix[2] * ex + ix[3] * ey;
                    if l1 < -1e-9 || l2 < -1e-9 || l1 + l2 > 1.0 + 1e-9 { continue; }
                    let (u, v) = (s0.0 + (s1.0 - s0.0) * l1 + (s2.0 - s0.0) * l2, s0.1 + (s1.1 - s0.1) * l1 + (s2.1 - s0.1) * l2);
                    if let Some(c) = src.at(u, v, level, foot, interp) { samples[y * rw + x] = Some(c); }
                }
            }
        }
        for (i, s) in samples.iter().enumerate() {
            if let Some(c) = s { over(&mut band[i * 4..i * 4 + 4], *c); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| { let (x, y) = (i % w, i / w); if (x / 4 + y / 4) % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] } }).collect()
    }

    #[test]
    fn homographies_map_corners_and_invert() {
        let q = [[10.0, 5.0], [90.0, 20.0], [80.0, 70.0], [5.0, 60.0]];
        let h = Homography::rect_to_quad([0.0, 0.0, 40.0, 30.0], q).unwrap();
        for (c, e) in corners((0, 0, 40, 30)).iter().zip(q) {
            let p = h.apply(c[0], c[1]);
            assert!((p.0 - e[0]).abs() < 1e-6 && (p.1 - e[1]).abs() < 1e-6);
        }
        let i = h.inverse().unwrap();
        let p = i.apply(50.0, 40.0);
        let back = h.apply(p.0, p.1);
        assert!((back.0 - 50.0).abs() < 1e-6 && (back.1 - 40.0).abs() < 1e-6);
        assert!(Homography::square_to_quad([[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]).is_none());
        assert!(convex(&q) && !convex(&[[0.0, 0.0], [10.0, 0.0], [2.0, 2.0], [0.0, 10.0]]));
        let k = keep_convex(q, [[60.0, 50.0], q[1], q[2], q[3]]);
        assert!(convex(&k));
    }

    #[test]
    fn identity_and_moves_keep_the_pixels() {
        let (w, h) = (32u32, 24u32);
        let px = checker(w, h);
        let src = Source::new((8, 8, 8 + w, 8 + h), &px);
        for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
            // Moved by whole pixels: exactly the same pixels.
            let warp = Warp { quad: corners((20, 10, 20 + w, 10 + h)), mesh: None };
            let mut out = vec![0u8; (w * h * 4) as usize];
            render(&src, &warp, interp, (20, 10, 20 + w, 10 + h), &mut out);
            assert_eq!(out, px, "{interp:?}");
        }
        // A grid left as it is changes nothing either.
        let warp = Warp { quad: corners((8, 8, 8 + w, 8 + h)), mesh: Some(Mesh::new(3, 3)) };
        let mut out = vec![0u8; (w * h * 4) as usize];
        render(&src, &warp, Interp::Bicubic, (8, 8, 8 + w, 8 + h), &mut out);
        assert_eq!(out, px);
    }

    #[test]
    fn halving_averages_and_rotating_keeps_alpha() {
        // A 1 px checker shrunk to half: an even mix, not aliasing.
        let fine: Vec<u8> = (0..64 * 64).flat_map(|i| if (i % 64 + i / 64) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let src = Source::new((0, 0, 64, 64), &fine);
        let warp = Warp { quad: corners((0, 0, 32, 32)), mesh: None };
        let mut out = vec![0u8; 32 * 32 * 4];
        render(&src, &warp, Interp::Bicubic, (0, 0, 32, 32), &mut out);
        for p in out[(10 * 32 + 10) * 4..(10 * 32 + 20) * 4].chunks(4) { assert!((110..=145).contains(&p[0]), "{p:?}"); }
        // A square turned 45°: its middle opaque, its corners outside empty,
        // soft edges.
        let sq = vec![200u8; 40 * 40 * 4].chunks(4).flat_map(|_| [200, 50, 10, 255]).collect::<Vec<u8>>();
        let src = Source::new((30, 30, 70, 70), &sq);
        let (c, r) = (50.0, 20.0 * 2f64.sqrt());
        let quad = [[c, c - r], [c + r, c], [c, c + r], [c - r, c]];
        let warp = Warp { quad, mesh: None };
        let dr = warp.dest_rect((30, 30, 70, 70), 100, 100).unwrap();
        let (dw, dh) = (dr.2 - dr.0, dr.3 - dr.1);
        let mut out = vec![0u8; (dw * dh * 4) as usize];
        render(&src, &warp, Interp::Bicubic, dr, &mut out);
        let px = |x: u32, y: u32| &out[(((y - dr.1) * dw + x - dr.0) * 4) as usize..][..4];
        assert_eq!(px(50, 50), &[200, 50, 10, 255]);
        assert_eq!(px(dr.0, dr.1)[3], 0);
        assert!(out.chunks(4).any(|p| p[3] > 0 && p[3] < 255));
        assert!(out.chunks(4).all(|p| p[3] == 0 || p[0] >= 190));
    }

    /// A point moves its four cells only: the points around and what is
    /// past them stay.
    #[test]
    fn a_warp_point_moves_its_cells_only() {
        let mut m = Mesh::new(5, 5);
        let k = 2 * 6 + 2;
        m.pts[k] = [m.pts[k][0] + 0.07, m.pts[k][1] - 0.05];
        for j in 0..=5 {
            for i in 0..=5 {
                let (u, v) = (i as f64 / 5.0, j as f64 / 5.0);
                let e = m.eval(u, v);
                if (i, j) == (2, 2) { assert!((e[0] - u - 0.07).abs() < 1e-12 && (e[1] - v + 0.05).abs() < 1e-12); }
                else { assert!((e[0] - u).abs() < 1e-12 && (e[1] - v).abs() < 1e-12, "point {i},{j} moved"); }
            }
        }
        // Inside the cells around: moved; past them: not at all.
        let e = m.eval(0.35, 0.45);
        assert!((e[0] - 0.35).abs() > 1e-3);
        for (u, v) in [(0.15, 0.4), (0.65, 0.4), (0.4, 0.1), (0.4, 0.7), (0.9, 0.9), (0.62, 0.62)] {
            assert_eq!(m.eval(u, v), [u, v], "{u},{v} moved");
        }
        // The whole grid moved: the whole image moves the same.
        let mut all = Mesh::new(3, 3);
        for p in &mut all.pts { p[0] += 0.1; }
        for (u, v) in [(0.0, 0.0), (0.33, 0.71), (1.0, 0.5)] {
            let e = all.eval(u, v);
            assert!((e[0] - u - 0.1).abs() < 1e-12 && (e[1] - v).abs() < 1e-12);
        }
    }

    #[test]
    fn a_bent_grid_moves_its_middle() {
        let px = vec![255u8; 40 * 40 * 4];
        let src = Source::new((0, 0, 40, 40), &px);
        let mut m = Mesh::new(2, 2);
        m.pts[4] = [0.5, 0.25];
        // The middle of the source now lands at a quarter.
        let warp = Warp { quad: corners((0, 0, 40, 40)), mesh: Some(m.clone()) };
        let p = warp.map((0, 0, 40, 40), 20.0, 20.0).unwrap();
        assert!((p.0 - 20.0).abs() < 1e-6 && (p.1 - 10.0).abs() < 1e-6, "{p:?}");
        assert_eq!(m.eval(0.0, 0.0), [0.0, 0.0]);
        assert_eq!(m.eval(1.0, 1.0), [1.0, 1.0]);
        let mut out = vec![0u8; 40 * 40 * 4];
        render(&src, &warp, Interp::Bilinear, (0, 0, 40, 40), &mut out);
        // Still covers its corners and middle.
        assert_eq!(out[(38 * 40 + 38) * 4 + 3], 255);
        assert_eq!(out[(20 * 40 + 20) * 4 + 3], 255);
        assert_eq!(out[(2 * 40 + 2) * 4 + 3], 255);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// cargo test --release -p aurastudio-layers transform_speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn transform_speed() {
        let (w, h) = (1610u32, 1247u32);
        let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 61) as u8, 255]).collect();
        let t = std::time::Instant::now();
        let src = Source::new((0, 0, w, h), &px);
        println!("source + pyramid: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        let (c, a) = ((w as f64 / 2.0, h as f64 / 2.0), std::env::var("ANGLE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.15f64));
        for (name, k) in [("rotated 85 %", 0.85), ("rotated 40 %", 0.4), ("rotated 160 %", 1.6)] {
            let quad = corners((0, 0, w, h)).map(|p| { let (x, y) = ((p[0] - c.0) * k, (p[1] - c.1) * k); [c.0 + x * a.cos() - y * a.sin(), c.1 + x * a.sin() + y * a.cos()] });
            let warp = Warp { quad, mesh: None };
            for interp in [Interp::Bilinear, Interp::Bicubic] {
                let mut out = vec![0u8; (w * h * 4) as usize];
                let t = std::time::Instant::now();
                for _ in 0..5 { render(&src, &warp, interp, (0, 0, w, h), &mut out); }
                println!("{name} {interp:?}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0 / 5.0);
            }
            let mut m = Mesh::new(4, 4);
            m.pts[6][0] += 0.05;
            let warp = Warp { quad, mesh: Some(m) };
            let mut out = vec![0u8; (w * h * 4) as usize];
            let t = std::time::Instant::now();
            for _ in 0..5 { render(&src, &warp, Interp::Bilinear, (0, 0, w, h), &mut out); }
            println!("{name} mesh bilinear: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0 / 5.0);
        }
    }
}

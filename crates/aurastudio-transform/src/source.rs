//! What is transformed: its premultiplied pixels and their halvings (built
//! when a reduction first needs them, rows on all cores), and sampling.

use std::sync::OnceLock;

use aurastudio_core::{Rect, Rgba8};
use rayon::prelude::*;

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
#[inline]
fn bicubic(t: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let near = |x: f32| (A + 2.0) * x * x * x - (A + 3.0) * x * x + 1.0;
    let far = |x: f32| A * (x * x * x - 5.0 * x * x + 8.0 * x - 4.0);
    [far(1.0 + t), near(t), near(1.0 - t), far(2.0 - t)]
}

/// Premultiplied pixels, 16 bits a channel (65535 = 1.0).
struct Level {
    w:  usize,
    h:  usize,
    px: Vec<[u16; 4]>,
}

impl Level {
    /// Half the size, each pixel the average of the up to four below.
    fn half(&self) -> Level {
        let (nw, nh) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut px = vec![[0u16; 4]; nw * nh];
        px.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let (mut s, mut n) = ([0u32; 4], 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (2 * x + dx, 2 * y + dy);
                    if sx < self.w && sy < self.h {
                        let p = self.px[sy * self.w + sx];
                        for c in 0..4 { s[c] += p[c] as u32; }
                        n += 1;
                    }
                }
                *o = s.map(|v| ((v + n / 2) / n) as u16);
            }
        });
        Level { w: nw, h: nh, px }
    }
}

/// A level to sample from, and its size against the source.
pub(crate) struct Lod<'a> {
    lv: &'a Level,
    k:  f64,
}

/// What is transformed: its pixels at full size, halvings on demand.
pub struct Source {
    rect:   Rect,
    levels: Vec<OnceLock<Level>>,
}

impl Source {
    /// From straight RGBA rows of `rect`.
    pub fn new(rect: Rect, rgba: &[Rgba8]) -> Self {
        let (w, h) = (rect.w() as usize, rect.h() as usize);
        let px: Vec<[u16; 4]> = if rgba.len() == w * h {
            rgba.par_iter().with_min_len(4096).map(|p| {
                let a = p[3] as u32;
                let pm = |c: u8| ((c as u32 * a * 65535 + 32512) / 65025) as u16;
                [pm(p[0]), pm(p[1]), pm(p[2]), (a * 257) as u16]
            }).collect()
        } else {
            vec![[0; 4]; w * h]
        };
        // Halvings down to one pixel (at most 13).
        let mut n = 1;
        let (mut lw, mut lh) = (w, h);
        while n < 14 && (lw > 1 || lh > 1) { lw = lw.div_ceil(2); lh = lh.div_ceil(2); n += 1; }
        let levels: Vec<OnceLock<Level>> = (0..n).map(|_| OnceLock::new()).collect();
        let _ = levels[0].set(Level { w, h, px });
        Self { rect, levels }
    }

    pub fn rect(&self) -> Rect { self.rect }

    /// Level `l`, made from the one above it the first time.
    fn lv(&self, l: usize) -> &Level {
        let l = l.min(self.levels.len() - 1);
        if let Some(v) = self.levels[l].get() { return v; }
        let above = self.lv(l - 1).half();
        self.levels[l].get_or_init(|| above)
    }

    /// Premultiplied sample at level `l`, (x, y) in its pixels (centers at
    /// k + 0.5), clamped to its edge.
    fn sample(lv: &Level, x: f32, y: f32, interp: Interp) -> [f32; 4] {
        let (mx, my) = (lv.w as isize - 1, lv.h as isize - 1);
        let (u, v) = (x - 0.5, y - 0.5);
        // Away from the edges no tap needs clamping.
        let (fi, fj) = (u.floor() as isize, v.floor() as isize);
        let inner = fi >= 1 && fj >= 1 && fi + 2 <= mx && fj + 2 <= my;
        let px = |i: isize, j: isize| -> [f32; 4] {
            let k = if inner { j as usize * lv.w + i as usize } else { (j.clamp(0, my) as usize) * lv.w + i.clamp(0, mx) as usize };
            lv.px[k].map(|c| c as f32)
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

    /// The level to sample `scale` source pixels per destination pixel
    /// from (made now if needed), and its size factor.
    pub(crate) fn level(&self, scale: f64, interp: Interp) -> Lod<'_> {
        let l = if interp == Interp::Nearest || scale <= 1.0 || !scale.is_finite() { 0 } else { (scale.log2().floor() as usize).min(self.levels.len() - 1) };
        Lod { lv: self.lv(l), k: 1.0 / (1u32 << l) as f64 }
    }

    /// The sample for source canvas point (x, y) at `level`, its footprint
    /// `foot` (source pixels across and down): the outline anti-aliased.
    #[inline]
    pub(crate) fn at(&self, x: f64, y: f64, lod: &Lod<'_>, foot: (f64, f64), interp: Interp) -> Option<[f32; 4]> {
        let r = self.rect;
        let (u, v) = (x - r.x0 as f64, y - r.y0 as f64);
        let (w, h) = (r.w() as f64, r.h() as f64);
        if u < -1.0 || v < -1.0 || u > w + 1.0 || v > h + 1.0 { return None; }
        let mut s = Self::sample(lod.lv, (u * lod.k) as f32, (v * lod.k) as f32, interp);
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

    /// Bytes held (the levels built so far).
    pub fn heap_bytes(&self) -> usize { self.levels.iter().filter_map(|l| l.get()).map(|l| l.px.len() * 8).sum() }
}

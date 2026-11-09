//! Changing a selection's edge: grow, shrink, antialias, feather, border.
//! Each works over the selection's box grown by its reach, on all cores; a
//! rectangle is worked out from its sides (no mask read, no distance map).

use aurastudio_core::{math, Rect};

use crate::Selection;

impl Selection {
    /// `out` (rows of `area`) as the new selection.
    fn redone(&self, area: Rect, out: &[u8]) -> Option<Self> { Self::from_mask(self.space(), area, out) }

    /// Grown by `r` pixels, round (Photoshop's Expand, Krita's Grow).
    pub fn grown(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let area = self.bbox.grown(r.ceil() as i32 + 1);
        let cover = |d: f32| math::to_u8((r + 1.0 - d).clamp(0.0, 1.0));
        if self.is_rect {
            // Distance to the rectangle, as is: no mask read.
            let b = self.bbox;
            return self.redone(area, &rows_of(area, |x, y| {
                let (dx, dy) = ((b.x0 - x).max(x - (b.x1 - 1)).max(0), (b.y0 - y).max(y - (b.y1 - 1)).max(0));
                if dx == 0 && dy == 0 { 255 } else { cover(((dx * dx + dy * dy) as f32).sqrt()) }
            }));
        }
        let m = self.mask(area);
        let (w, h) = (area.w() as usize, area.h() as usize);
        let mut out = m.clone();
        distance_rows(|i| m[i] >= 128, w, h, &mut out, |i, d, o| *o = m[i].max(cover(d)));
        self.redone(area, &out)
    }

    /// Shrunk by `r` pixels. A selection within the frame isn't shrunk
    /// from the frame's edge (select all, shrink: the edges stay).
    pub fn shrunk(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let frame = self.space().frame();
        let inside = frame.contains_rect(&self.bbox);
        let area = self.bbox.grown(1);
        let area = if inside { area.intersect(&frame) } else { area };
        let cut = |d: f32| 255 - math::to_u8((r + 1.0 - d).clamp(0.0, 1.0));
        if self.is_rect {
            // Sides on the frame's edge (within it) don't move.
            let b = self.bbox;
            let keep = |at_edge: bool| inside && at_edge;
            let (l, t, rt, bt) = (keep(b.x0 <= frame.x0), keep(b.y0 <= frame.y0), keep(b.x1 >= frame.x1), keep(b.y1 >= frame.y1));
            if r.fract() == 0.0 {
                let k = r as i32;
                let n = Rect::new(if l { b.x0 } else { b.x0 + k }, if t { b.y0 } else { b.y0 + k }, if rt { b.x1 } else { b.x1 - k }, if bt { b.y1 } else { b.y1 - k });
                return Self::rect(self.space(), n);
            }
            return self.redone(b, &rows_of(b, |x, y| {
                let far = i32::MAX;
                let d = [if l { far } else { x - b.x0 + 1 }, if t { far } else { y - b.y0 + 1 }, if rt { far } else { b.x1 - x }, if bt { far } else { b.y1 - y }];
                let d = d.into_iter().min().unwrap_or(far);
                if d == far { 255 } else { cut(d as f32) }
            }));
        }
        let (w, h) = (area.w() as usize, area.h() as usize);
        let m = self.mask(area);
        let mut out = m.clone();
        // Distance to the nearest unselected pixel.
        distance_rows(|i| m[i] < 128, w, h, &mut out, |i, d, o| *o = m[i].min(cut(d)));
        self.redone(area, &out)
    }

    /// Hard edges softened: a pixel on the edge gets the average of its
    /// 3 × 3 neighbors, kept on its side of one half (the magic wand's
    /// anti-aliasing).
    pub fn antialiased(&self) -> Option<Self> {
        let area = self.bbox.grown(1);
        let (w, h) = (area.w() as usize, area.h() as usize);
        let m = self.mask(area);
        let mut out = m.clone();
        for y in 0..h {
            for x in 0..w {
                let v = m[y * w + x];
                let (mut sum, mut edge) = (0u32, false);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        let n = if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 { v } else { m[ny as usize * w + nx as usize] };
                        edge |= n != v;
                        sum += n as u32;
                    }
                }
                if edge {
                    let avg = sum as f32 / (9.0 * 255.0);
                    out[y * w + x] = math::to_u8(if v > 127 { 0.5 + 0.5 * avg } else { 0.5 * avg });
                }
            }
        }
        self.redone(area, &out)
    }

    /// Soft edges: a Gaussian blur of the coverage, sigma `r` (Photoshop's
    /// Feather), through `filters`' one blur.
    pub fn feathered(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let mut m = self.mask.clone();
        aurastudio_filters::blur_mask(&mut m, r);
        Self::of(m)
    }

    /// A soft band `2r` wide along the outline (Photoshop's Border).
    pub fn bordered(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return None; }
        let area = self.bbox.grown(r.ceil() as i32 + 1);
        let (w, h) = (area.w() as usize, area.h() as usize);
        let m = self.mask(area);
        let band = |d: f32| math::to_u8((1.0 - (d - 1.0) / r).clamp(0.0, 1.0));
        let mut out = vec![0u8; w * h];
        // Outside: by the distance to the selection; inside: to the rest.
        distance_rows(|i| m[i] >= 128, w, h, &mut out, |i, d, o| if m[i] < 128 { *o = band(d); });
        distance_rows(|i| m[i] < 128, w, h, &mut out, |i, d, o| if m[i] >= 128 { *o = band(d); });
        self.redone(area, &out)
    }
}

/// Rows of `area` from `f(x, y)`, on all cores.
fn rows_of(area: Rect, f: impl Fn(i32, i32) -> u8 + Sync) -> Vec<u8> {
    use rayon::prelude::*;
    let w = area.w() as usize;
    let mut out = vec![0u8; w * area.h() as usize];
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(j, row)| {
        let y = area.y0 + j as i32;
        for (i, o) in row.iter_mut().enumerate() { *o = f(area.x0 + i as i32, y); }
    });
    out
}

/// Distance from each pixel's center to the nearest `set` one (Felzenszwalb
/// and Huttenlocher's exact transform, a pass per column then per row, on
/// all cores), handed row by row to `put(index, distance, out pixel)`: one
/// f32 per pixel held (the columns' pass), no other copy.
fn distance_rows(set: impl Fn(usize) -> bool + Sync, w: usize, h: usize, out: &mut [u8], put: impl Fn(usize, f32, &mut u8) + Sync) {
    use rayon::prelude::*;
    const INF: f32 = 1e20;
    if w == 0 || h == 0 || out.len() != w * h { return; }
    struct Pass { d: Vec<f32>, v: Vec<usize>, z: Vec<f32> }
    impl Pass {
        fn new(n: usize) -> Self { Self { d: vec![0.0; n], v: vec![0; n], z: vec![0.0; n + 1] } }
        /// The lower envelope of the parabolas over `f`: `d` gets it.
        fn run(&mut self, f: &[f32]) {
            let (d, v, z) = (&mut self.d, &mut self.v, &mut self.z);
            let len = f.len();
            let meet = |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
            let mut k = 0;
            v[0] = 0;
            z[0] = f32::NEG_INFINITY;
            z[1] = f32::INFINITY;
            for q in 1..len {
                let mut s = meet(q, v[k]);
                while s <= z[k] { k -= 1; s = meet(q, v[k]); }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
            }
            k = 0;
            for (q, dq) in d.iter_mut().enumerate().take(len) {
                while z[k + 1] < q as f32 { k += 1; }
                let p = v[k];
                *dq = (q as f32 - p as f32).powi(2) + f[p];
            }
        }
    }
    // Columns, each stored as a row of `cols` (column x at x * h).
    let mut cols = vec![0f32; w * h];
    cols.par_chunks_mut(h).enumerate().for_each_init(|| (Pass::new(h), vec![0f32; h]), |(pass, f), (x, col)| {
        for (y, v) in f.iter_mut().enumerate() { *v = if set(y * w + x) { 0.0 } else { INF }; }
        pass.run(f);
        col.copy_from_slice(&pass.d[..h]);
    });
    out.par_chunks_mut(w).enumerate().for_each_init(|| (Pass::new(w), vec![0f32; w]), |(pass, f), (y, row)| {
        for (x, v) in f.iter_mut().enumerate() { *v = cols[x * h + y]; }
        pass.run(f);
        for (x, o) in row.iter_mut().enumerate() { put(y * w + x, pass.d[x].min(INF).sqrt(), o); }
    });
}

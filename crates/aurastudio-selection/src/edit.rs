//! Changing a selection's edge: grow, shrink, antialias, feather, border.
//! Each works over the selection's box grown by its reach (a per-tile
//! version with a halo comes if the measures ask for it).

use aurastudio_core::{math, Rect};

use crate::Selection;

impl Selection {
    /// `out` (rows of `area`) as the new selection.
    fn redone(&self, area: Rect, out: &[u8]) -> Option<Self> { Self::from_mask(self.space(), area, out) }

    /// Grown by `r` pixels, round (Photoshop's Expand, Krita's Grow).
    pub fn grown(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let area = self.bbox.grown(r.ceil() as i32 + 1);
        let (w, h) = (area.w() as usize, area.h() as usize);
        let m = self.mask(area);
        let d = distance(&m.iter().map(|v| *v >= 128).collect::<Vec<_>>(), w, h);
        let out: Vec<u8> = m.iter().zip(d).map(|(v, d)| (*v).max(math::to_u8((r + 1.0 - d).clamp(0.0, 1.0)))).collect();
        self.redone(area, &out)
    }

    /// Shrunk by `r` pixels. A selection within the frame isn't shrunk
    /// from the frame's edge (select all, shrink: the edges stay).
    pub fn shrunk(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let frame = self.space().frame();
        let area = self.bbox.grown(1);
        let area = if frame.contains_rect(&self.bbox) { area.intersect(&frame) } else { area };
        let (w, h) = (area.w() as usize, area.h() as usize);
        let m = self.mask(area);
        // Distance to the nearest unselected pixel.
        let d = distance(&m.iter().map(|v| *v < 128).collect::<Vec<_>>(), w, h);
        let out: Vec<u8> = m.iter().zip(d).map(|(v, d)| (*v).min(255 - math::to_u8((r + 1.0 - d).clamp(0.0, 1.0)))).collect();
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
        let inside: Vec<bool> = m.iter().map(|v| *v >= 128).collect();
        let outside: Vec<bool> = inside.iter().map(|v| !v).collect();
        let (to_in, to_out) = (distance(&inside, w, h), distance(&outside, w, h));
        let out: Vec<u8> = inside.iter().enumerate().map(|(i, &sel)| {
            let d = if sel { to_out[i] } else { to_in[i] };
            math::to_u8((1.0 - (d - 1.0) / r).clamp(0.0, 1.0))
        }).collect();
        self.redone(area, &out)
    }
}

/// Distance from each pixel's center to the nearest `set` one (Felzenszwalb
/// and Huttenlocher's exact transform, a pass per column then per row).
fn distance(set: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut g: Vec<f32> = set.iter().map(|&s| if s { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0f32; n], vec![0f32; n], vec![0usize; n], vec![0f32; n + 1]);
    let mut pass = |f: &[f32], len: usize, d: &mut [f32]| {
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
    };
    for x in 0..w {
        for y in 0..h { f[y] = g[y * w + x]; }
        pass(&f, h, &mut d);
        for y in 0..h { g[y * w + x] = d[y]; }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        pass(&f, w, &mut d);
        g[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    g.iter().map(|v| v.min(INF).sqrt()).collect()
}

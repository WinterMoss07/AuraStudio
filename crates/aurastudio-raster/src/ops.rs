//! Whole-surface operations: resize, retile, copy between surfaces, the
//! fill tool's coverage fill, half-size levels (the pyramid built on
//! demand when zoomed out), and the mask operations selections use.

use std::sync::Arc;

use aurastudio_core::{Pixel, Rect};

use crate::slot::Slot;
use crate::surface::Surface;

/// How two masks combine (selection modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// The other mask instead.
    Replace,
    /// The higher of the two (add to the selection).
    Max,
    /// The lower of the two (intersect).
    Min,
    /// This minus the other, saturating (subtract).
    Subtract,
}

impl Combine {
    #[inline(always)]
    fn apply(self, a: u8, b: u8) -> u8 {
        match self { Combine::Replace => b, Combine::Max => a.max(b), Combine::Min => a.min(b), Combine::Subtract => a.saturating_sub(b) }
    }
}

impl<P: Pixel> Surface<P> {
    /// The same pixels on a surface of another size, the old one placed at
    /// (dx, dy) (canvas resize, content centered); outside: the fill.
    pub fn resized(&self, width: u32, height: u32, dx: i32, dy: i32) -> Self {
        let mut out = Self::new(width, height, self.tile, self.fill);
        out.copy_from(self, self.rect(), dx, dy);
        out
    }

    /// The same pixels with another tile size (an explicit conversion).
    pub fn retiled(&self, tile: u32) -> Self {
        let mut out = Self::new(self.width, self.height, tile, self.fill);
        if out.tile == self.tile { out.slots = self.slots.clone(); return out; }
        out.copy_from(self, self.rect(), 0, 0);
        out
    }

    /// Copy `src_rect` of `src` to this surface moved by (dx, dy) (move,
    /// paste, resize). Tile-aligned moves between surfaces of the same tile
    /// size share the tiles; the rest is copied by rows.
    pub fn copy_from(&mut self, src: &Surface<P>, src_rect: Rect, dx: i32, dy: i32) {
        let from = src_rect.intersect(&src.rect());
        let to = from.moved(dx, dy).intersect(&self.rect());
        if to.is_empty() { return; }
        let t = self.tile as i32;
        let aligned = src.tile == self.tile && dx % t == 0 && dy % t == 0 && src.fill == self.fill;
        if aligned {
            let Some((tx0, ty0, tx1, ty1)) = src.tiles_of(&from) else { return };
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    let sr = src.tile_rect(tx, ty);
                    let (ntx, nty) = (tx as i32 + dx / t, ty as i32 + dy / t);
                    if ntx < 0 || nty < 0 || ntx as u32 >= self.cols || nty as u32 >= self.rows { continue; }
                    let dr = self.tile_rect(ntx as u32, nty as u32);
                    // Whole tile inside both: shared as it is.
                    if from.contains_rect(&sr) && sr.moved(dx, dy) == dr {
                        self.set_slot(ntx as u32, nty as u32, src.slot(tx, ty).clone());
                    } else {
                        let part = sr.intersect(&from).moved(dx, dy).intersect(&dr);
                        if !part.is_empty() { self.write(part, &src.read(part.moved(-dx, -dy))); }
                    }
                }
            }
            self.drop_table_if_empty();
            return;
        }
        for c in self.chunks(to, 256) {
            let s = c.moved(-dx, -dy);
            if src.is_fill_in(s) && src.fill == self.fill { self.fill_rect(c, self.fill); continue; }
            self.write(c, &src.read(s));
        }
    }

    /// `color` over the pixels of `rect` with coverage `cov` (one byte per
    /// pixel of `rect`, rows packed): the fill tool. Fully covered tiles
    /// of an opaque color become that color (no pixels).
    pub fn fill_mask(&mut self, rect: Rect, cov: &[u8], color: P) {
        let rw = rect.w() as usize;
        if cov.len() != rw * rect.h() as usize { return; }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let tr = self.tile_rect(tx, ty);
                let part = tr.intersect(&rect);
                if part.is_empty() { continue; }
                let crow = |y: i32| { let s = (y - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize; &cov[s..s + part.w() as usize] };
                if (part.y0..part.y1).all(|y| crow(y).iter().all(|c| *c == 0)) { continue; }
                if part == tr && color.opaque() && (part.y0..part.y1).all(|y| crow(y).iter().all(|c| *c == 255)) {
                    self.set_slot(tx, ty, if color == self.fill { Slot::Fill } else { Slot::Uniform(color) });
                    continue;
                }
                let shift = self.shift;
                let rows: Vec<&[u8]> = (part.y0..part.y1).map(crow).collect();
                let px = self.full_mut(tx, ty);
                for (k, cr) in rows.into_iter().enumerate() {
                    let d = (((part.y0 + k as i32 - tr.y0) << shift) + (part.x0 - tr.x0)) as usize;
                    for (p, c) in px[d..d + cr.len()].iter_mut().zip(cr) {
                        if *c > 0 { *p = p.over(color, *c); }
                    }
                }
                self.settle(tx, ty);
            }
        }
        self.drop_table_if_empty();
    }

    /// Half the size (each pixel the average of 2 × 2, in premultiplied
    /// space): a pyramid level, built only when a zoomed-out view needs it.
    /// One-color tiles stay one color.
    pub fn half(&self) -> Self {
        let (w, h) = (self.width.div_ceil(2).max(1), self.height.div_ceil(2).max(1));
        let mut out = Self::new(w, h, self.tile, self.fill);
        for c in out.chunks(out.rect(), 256) {
            let src = Rect::new(c.x0 * 2, c.y0 * 2, c.x1 * 2, c.y1 * 2);
            if self.is_fill_in(src) { continue; }
            let sw = src.w() as usize;
            let px = self.read(src);
            let mut half = Vec::with_capacity(c.area() as usize);
            for y in 0..c.h() as usize {
                for x in 0..c.w() as usize {
                    let mut s = [0f32; 4];
                    let mut k = 0.0;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = (2 * x + dx, 2 * y + dy);
                        let (gx, gy) = (src.x0 as usize + sx, src.y0 as usize + sy);
                        if gx >= self.width as usize || gy >= self.height as usize { continue; }
                        let q = px[sy * sw + sx].to_premul();
                        for i in 0..4 { s[i] += q[i]; }
                        k += 1.0;
                    }
                    let k = 1.0 / f32::max(k, 1.0);
                    half.push(P::from_premul(s.map(|v| v * k)));
                }
            }
            out.write(c, &half);
        }
        out
    }
}

impl Surface<u8> {
    /// Combine another mask of the same size into this one, run by run;
    /// one-value tiles combine without touching pixels.
    pub fn combine(&mut self, other: &Surface<u8>, op: Combine) {
        if (other.width, other.height) != (self.width, self.height) { return; }
        if other.tile == self.tile {
            for ty in 0..self.rows {
                for tx in 0..self.cols {
                    let (a, b) = (self.slot(tx, ty).color(self.fill), other.slot(tx, ty).color(other.fill));
                    match (a, b) {
                        (Some(a), Some(b)) => {
                            let c = op.apply(a, b);
                            self.set_slot(tx, ty, if c == self.fill { Slot::Fill } else { Slot::Uniform(c) });
                        }
                        // Identity cases keep this tile as it is.
                        (_, Some(0)) if matches!(op, Combine::Max | Combine::Subtract) => {}
                        (_, Some(255)) if op == Combine::Min => {}
                        (_, Some(b)) if op == Combine::Replace => self.set_slot(tx, ty, if b == self.fill { Slot::Fill } else { Slot::Uniform(b) }),
                        (_, None) if op == Combine::Replace => self.set_slot(tx, ty, other.slot(tx, ty).clone()),
                        _ => self.combine_tile(other, tx, ty, op),
                    }
                }
            }
        } else {
            for c in self.chunks(self.rect(), 256) {
                let (a, b) = (self.read(c), other.read(c));
                let out: Vec<u8> = a.iter().zip(&b).map(|(a, b)| op.apply(*a, *b)).collect();
                self.write(c, &out);
            }
        }
        self.drop_table_if_empty();
    }

    fn combine_tile(&mut self, other: &Surface<u8>, tx: u32, ty: u32, op: Combine) {
        let r = self.tile_rect(tx, ty);
        let shift = self.shift;
        let mut b = Vec::with_capacity(r.area() as usize);
        other.rows(r, |_, _, run| for i in 0..run.len() { b.push(run.at(i)); });
        let px = self.full_mut(tx, ty);
        let w = r.w() as usize;
        for y in 0..r.h() as usize {
            for x in 0..w { let p = &mut px[(y << shift) + x]; *p = op.apply(*p, b[y * w + x]); }
        }
        self.settle(tx, ty);
    }

    /// 255 − v everywhere (select the inverse).
    pub fn invert(&mut self) {
        let n = self.npx();
        self.fill = 255 - self.fill;
        for s in &mut self.slots {
            *s = match std::mem::replace(s, Slot::Fill) {
                Slot::Fill => Slot::Fill,
                Slot::Uniform(c) => Slot::Uniform(255 - c),
                other => {
                    let px: Vec<u8> = other.expand(255 - self.fill, n).into_iter().map(|v| 255 - v).collect();
                    Slot::Full(Arc::new(crate::slot::FullTile { px, info: None }))
                }
            };
        }
        let fill = self.fill;
        for s in &mut self.slots { if matches!(s, Slot::Uniform(c) if *c == fill) { *s = Slot::Fill; } }
        let all: Vec<(u32, u32)> = (0..self.slots.len() as u32).map(|i| (i % self.cols, i / self.cols)).collect();
        for (tx, ty) in all { self.settle(tx, ty); }
        self.drop_table_if_empty();
    }

    /// The coverage of `rect`, rows packed (what a fill or a filter limits
    /// itself to).
    pub fn coverage(&self, rect: Rect) -> Vec<u8> { self.read(rect) }
}

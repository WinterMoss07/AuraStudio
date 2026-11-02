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
    /// The same pixels with a frame of another size, the old content moved
    /// by (dx, dy) (canvas resize, content centered). Nothing is cut: the
    /// canvas is unbounded.
    pub fn resized(&self, width: u32, height: u32, dx: i32, dy: i32) -> Self {
        let mut out = Self::new(width, height, self.tile, self.fill);
        if let Some(b) = self.stored_bounds() { out.copy_from(self, b, dx, dy); }
        out
    }

    /// The same pixels with another tile size (an explicit conversion).
    pub fn retiled(&self, tile: u32) -> Self {
        let mut out = Self::new(self.width, self.height, tile, self.fill);
        if out.tile == self.tile { out.grid = self.grid.clone(); return out; }
        if let Some(b) = self.stored_bounds() { out.copy_from(self, b, 0, 0); }
        out
    }

    /// Copy `src_rect` of `src` to this surface moved by (dx, dy) (move,
    /// paste, resize). Tile-aligned moves between surfaces of the same tile
    /// size and fill share the tiles; the rest is copied by chunks.
    pub fn copy_from(&mut self, src: &Surface<P>, src_rect: Rect, dx: i32, dy: i32) {
        if src_rect.is_empty() { return; }
        let t = self.tile as i32;
        let aligned = src.tile == self.tile && dx % t == 0 && dy % t == 0 && src.fill == self.fill;
        if aligned {
            let Some((tx0, ty0, tx1, ty1)) = src.tiles_of(&src_rect) else { return };
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    let sr = src.tile_rect(tx, ty);
                    let (ntx, nty) = (tx + dx / t, ty + dy / t);
                    if src_rect.contains_rect(&sr) {
                        self.set_slot(ntx, nty, src.slot(tx, ty).clone());
                    } else {
                        let part = sr.intersect(&src_rect);
                        if !part.is_empty() { self.write(part.moved(dx, dy), &src.read(part)); }
                    }
                }
            }
            self.drop_table_if_empty();
            return;
        }
        for c in self.chunks(src_rect.moved(dx, dy), 256) {
            let s = c.moved(-dx, -dy);
            if src.fill == self.fill && src.is_fill_in(s) { self.fill_rect(c, self.fill); continue; }
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
    /// One-color areas stay one color; the frame halves too.
    pub fn half(&self) -> Self {
        let mut out = Self::new(self.width.div_ceil(2).max(1), self.height.div_ceil(2).max(1), self.tile, self.fill);
        let Some(b) = self.stored_bounds() else { return out };
        let hb = Rect::new(b.x0.div_euclid(2), b.y0.div_euclid(2), (b.x1 + 1).div_euclid(2), (b.y1 + 1).div_euclid(2));
        for c in out.chunks(hb, 256) {
            let src = Rect::new(c.x0 * 2, c.y0 * 2, c.x1 * 2, c.y1 * 2);
            if self.is_fill_in(src) { continue; }
            let sw = src.w() as usize;
            let px = self.read(src);
            let mut half = Vec::with_capacity(c.area() as usize);
            for y in 0..c.h() as usize {
                for x in 0..c.w() as usize {
                    let mut s = [0f32; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let q = px[(2 * y + dy) * sw + 2 * x + dx].to_premul();
                        for i in 0..4 { s[i] += q[i]; }
                    }
                    half.push(P::from_premul(s.map(|v| v * 0.25)));
                }
            }
            out.write(c, &half);
        }
        out
    }
}

impl Surface<u8> {
    /// Combine another mask into this one, run by run; one-value tiles
    /// combine without touching pixels. Everywhere neither stores
    /// anything, the fills combine.
    pub fn combine(&mut self, other: &Surface<u8>, op: Combine) {
        if other.tile == self.tile {
            let mut cells: Vec<(i32, i32)> = self.grid.iter().map(|(x, y, _)| (x, y)).chain(other.grid.iter().map(|(x, y, _)| (x, y))).collect();
            cells.sort_unstable();
            cells.dedup();
            let new_fill = op.apply(self.fill, other.fill);
            // Every other tile is the combination of the two fills.
            let old_fill = self.fill;
            for (tx, ty) in cells {
                let (a, b) = (self.slot(tx, ty).color(old_fill), other.slot(tx, ty).color(other.fill));
                match (a, b, op) {
                    (Some(a), Some(b), _) => self.set_slot(tx, ty, Slot::Uniform(op.apply(a, b))),
                    (_, None, Combine::Replace) => self.set_slot(tx, ty, other.slot(tx, ty).clone()),
                    (None, Some(0), Combine::Max | Combine::Subtract) | (None, Some(255), Combine::Min) => {}
                    _ => self.combine_tile(other, tx, ty, op, old_fill),
                }
            }
            self.fill = new_fill;
            for (_, _, s) in self.grid.iter_mut() { if matches!(s, Slot::Uniform(c) if *c == new_fill) { *s = Slot::Fill; } }
        } else {
            let area = [self.stored_bounds(), other.stored_bounds()].into_iter().flatten().fold(Rect::EMPTY, |a, r| a.union(&r));
            let new_fill = op.apply(self.fill, other.fill);
            let mut result = Surface::new(self.width, self.height, self.tile, new_fill);
            for c in self.chunks(area, 256) {
                let (a, b) = (self.read(c), other.read(c));
                let out: Vec<u8> = a.iter().zip(&b).map(|(a, b)| op.apply(*a, *b)).collect();
                result.write(c, &out);
            }
            *self = result;
        }
        self.drop_table_if_empty();
    }

    fn combine_tile(&mut self, other: &Surface<u8>, tx: i32, ty: i32, op: Combine, old_fill: u8) {
        let r = self.tile_rect(tx, ty);
        let mut b = Vec::with_capacity(r.area() as usize);
        other.rows(r, |_, _, run| for i in 0..run.len() { b.push(run.at(i)); });
        // (A fill tile of this mask is its old fill, not the new one.)
        if self.slot(tx, ty).is_fill() { self.set_slot(tx, ty, Slot::Uniform(old_fill)); }
        let px = self.full_mut(tx, ty);
        for (p, v) in px.iter_mut().zip(&b) { *p = op.apply(*p, *v); }
        self.settle(tx, ty);
    }

    /// 255 − v everywhere (select the inverse).
    pub fn invert(&mut self) {
        let n = self.npx();
        let old = self.fill;
        self.fill = 255 - old;
        let new_fill = self.fill;
        let mut cells = Vec::new();
        for (tx, ty, s) in self.grid.iter_mut() {
            *s = match std::mem::replace(s, Slot::Fill) {
                Slot::Fill => Slot::Fill,
                Slot::Uniform(c) if 255 - c == new_fill => Slot::Fill,
                Slot::Uniform(c) => Slot::Uniform(255 - c),
                other => {
                    let px: Vec<u8> = other.expand(old, n).into_iter().map(|v| 255 - v).collect();
                    cells.push((tx, ty));
                    Slot::Full(Arc::new(crate::slot::FullTile { px, info: None }))
                }
            };
        }
        for (tx, ty) in cells { self.settle(tx, ty); }
        self.drop_table_if_empty();
    }

    /// The coverage of `rect`, rows packed (what a fill or a filter limits
    /// itself to).
    pub fn coverage(&self, rect: Rect) -> Vec<u8> { self.read(rect) }
}

//! A stroke in progress: what the brush engine paints through
//! (`core::PaintSurface`). The base is a snapshot taken at the start
//! (free: tiles are shared, a tile is only copied when first written);
//! the mask keeps each pixel's highest coverage; the limit (a selection's
//! coverage, any tile size) caps what the engine may cover, so a selection
//! clips the stroke at the source. The end settles the touched tiles (a
//! tile painted back to one color stores no pixels) and gives the undo
//! patch.

use std::collections::HashMap;
use std::sync::Arc;

use aurastudio_core::{PaintSurface, Pixel, Rect, Span};

use crate::patch::{Patch, Snapshot};
use crate::slot::Slot;
use crate::surface::{pixel_of, Surface};
use crate::DirtyTiles;

/// Shared base tiles of one color, by color.
type FlatBases<P> = HashMap<P, Arc<Vec<P>>>;
/// Base tiles opened from a coverage or cold form, by tile index.
type OpenBases<P> = HashMap<usize, Arc<Vec<P>>>;

struct Limits<'s> {
    surf: &'s Surface<u8>,
    /// One-value rows for uniform limit tiles, by value.
    flat: HashMap<u8, Box<[u8]>>,
    /// Limit tiles opened from a cold form, by tile index.
    open: HashMap<usize, Vec<u8>>,
}

pub struct Stroke<'s, P: Pixel> {
    surf:    &'s mut Surface<P>,
    before:  Snapshot<P>,
    flat:    FlatBases<P>,
    opened:  OpenBases<P>,
    /// Highest coverage per pixel, per touched tile (pooled buffers).
    mask:    Vec<Option<Box<[u8]>>>,
    limit:   Option<Limits<'s>>,
    touched: DirtyTiles,
}

impl<P: Pixel> Surface<P> {
    /// Start a stroke; `limit` (a one-channel surface of the same size,
    /// any tile size) caps the coverage the engine may give each pixel.
    pub fn begin_stroke<'s>(&'s mut self, limit: Option<&'s Surface<u8>>) -> Stroke<'s, P> {
        let before = self.snapshot();
        let n = (self.cols * self.rows) as usize;
        let limit = limit.filter(|l| (l.width, l.height) == (self.width, self.height)).map(|surf| Limits { surf, flat: HashMap::new(), open: HashMap::new() });
        let touched = self.dirty_set();
        Stroke { surf: self, before, flat: HashMap::new(), opened: HashMap::new(), mask: (0..n).map(|_| None).collect(), limit, touched }
    }
}

impl<P: Pixel> Stroke<'_, P> {
    /// The tiles the stroke went through so far.
    pub fn touched(&self) -> &DirtyTiles { &self.touched }

    /// The surface as it is now (for the merge while painting).
    pub fn surface(&self) -> &Surface<P> { self.surf }

    /// Coverage of the stroke at a pixel so far.
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        let s = self.surf.shift;
        if x < 0 || y < 0 || x as u32 >= self.surf.width || y as u32 >= self.surf.height { return 0; }
        let (x, y) = (x as u32, y as u32);
        let i = self.surf.index(x >> s, y >> s);
        self.mask[i].as_ref().map_or(0, |m| m[(((y & (self.surf.tile - 1)) << s) + (x & (self.surf.tile - 1))) as usize])
    }

    /// The end: touched tiles settled, masks back to the pool, and the
    /// patch that undoes the stroke (only the tiles that changed).
    pub fn end(self) -> Patch<P> {
        let Stroke { surf, before, mask, touched, .. } = self;
        for m in mask.into_iter().flatten() { crate::pool::give_mask(m); }
        surf.compact_set(&touched);
        surf.diff_set(&before, &touched)
    }

    /// Make tile (tx, ty) ready to paint: its pixels unshared (copied from
    /// the base once), its mask, its base readable as pixels.
    fn prepare_tile(&mut self, tx: u32, ty: u32) {
        let i = self.surf.index(tx, ty);
        if self.mask[i].is_some() { return; }
        let n = self.surf.npx();
        self.mask[i] = Some(crate::pool::take_mask(n));
        match self.before.slot(i) {
            Slot::Full(_) => {}
            Slot::Fill | Slot::Uniform(_) => {
                let c = self.before.slot(i).color(self.before.fill).unwrap_or(self.before.fill);
                self.flat.entry(c).or_insert_with(|| Arc::new(vec![c; n]));
            }
            s @ (Slot::Alpha(_) | Slot::Cold(_)) => { let px = s.expand(self.before.fill, n); self.opened.insert(i, Arc::new(px)); }
        }
        self.surf.full_mut(tx, ty);
        self.touched.insert(self.surf.tile_rect(tx, ty));
    }

    fn prepare_limit(&mut self, x: u32, y: u32) {
        let Some(l) = &mut self.limit else { return };
        let s = l.surf;
        let (lx, ly) = (x >> s.shift, y >> s.shift);
        let li = s.index(lx, ly);
        match s.slot(lx, ly) {
            Slot::Full(_) => {}
            Slot::Fill | Slot::Uniform(_) => {
                let c = s.slot(lx, ly).color(s.fill).unwrap_or(s.fill);
                l.flat.entry(c).or_insert_with(|| vec![c; s.tile as usize].into_boxed_slice());
            }
            other => { l.open.entry(li).or_insert_with(|| other.expand(s.fill, s.npx())); }
        }
    }

    /// Prepare the limit for every limit tile `r` touches.
    fn prepare_limits(&mut self, r: Rect) {
        let Some(l) = &self.limit else { return };
        let step = l.surf.tile as i32;
        let mut y = r.y0.div_euclid(step) * step;
        while y < r.y1 {
            let mut x = r.x0.div_euclid(step) * step;
            while x < r.x1 { self.prepare_limit(x.max(0) as u32, y.max(0) as u32); x += step; }
            y += step;
        }
    }
}

/// The base pixels of tile `i`.
fn base_of<'a, P: Pixel>(before: &'a Snapshot<P>, flat: &'a FlatBases<P>, opened: &'a OpenBases<P>, i: usize) -> &'a [P] {
    match before.slot(i) {
        Slot::Full(p) => &p.px,
        s @ (Slot::Fill | Slot::Uniform(_)) => {
            let c = s.color(before.fill).unwrap_or(before.fill);
            flat.get(&c).map_or(&[][..], |v| &v[..])
        }
        _ => opened.get(&i).map_or(&[][..], |v| &v[..]),
    }
}

/// The limit for `len` pixels from (x, y), `None` without a limit.
fn limit_of<'a>(l: Option<&'a Limits<'_>>, x: u32, y: u32, len: usize) -> Option<&'a [u8]> {
    let l = l?;
    let s = l.surf;
    let t = s.tile;
    let (lx, ly) = (x >> s.shift, y >> s.shift);
    let o = (((y & (t - 1)) << s.shift) + (x & (t - 1))) as usize;
    match s.slot(lx, ly) {
        Slot::Full(p) => p.px.get(o..o + len),
        c @ (Slot::Fill | Slot::Uniform(_)) => l.flat.get(&c.color(s.fill).unwrap_or(s.fill)).and_then(|v| v.get(..len)),
        _ => l.open.get(&s.index(lx, ly)).and_then(|v| v.get(o..o + len)),
    }
}

impl<P: Pixel> PaintSurface<P> for Stroke<'_, P> {
    fn width(&self) -> u32 { self.surf.width }
    fn height(&self) -> u32 { self.surf.height }

    fn prepare(&mut self, rect: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(&rect) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
        self.prepare_limits(rect.intersect(&self.surf.rect()));
    }

    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_, P> {
        let (t, shift) = (self.surf.tile, self.surf.shift);
        let (tx, ty) = (x >> shift, y >> shift);
        let mut len = max.min((((tx + 1) << shift) - x) as usize).min(self.surf.width.saturating_sub(x) as usize);
        if let Some(l) = &self.limit { len = len.min(((((x >> l.surf.shift) + 1) << l.surf.shift) - x) as usize); }
        if y >= self.surf.height || len == 0 {
            return Span { x, y, pixels: &mut [], base: &[], mask: &mut [], limit: None };
        }
        self.prepare_tile(tx, ty);
        self.prepare_limit(x, y);
        let i = self.surf.index(tx, ty);
        let o = (((y & (t - 1)) << shift) + (x & (t - 1))) as usize;
        let Stroke { surf, before, flat, opened, mask, limit, .. } = self;
        let base = base_of(before, flat, opened, i);
        let lim = limit_of(limit.as_ref(), x, y, len);
        let pixels = match surf.slots.get_mut(i) { Some(Slot::Full(p)) => Arc::make_mut(p).px.as_mut_slice(), _ => &mut [] };
        let m = mask[i].as_deref_mut().unwrap_or(&mut []);
        Span {
            x, y,
            pixels: pixels.get_mut(o..o + len).unwrap_or(&mut []),
            base: base.get(o..o + len).unwrap_or(&[]),
            mask: m.get_mut(o..o + len).unwrap_or(&mut []),
            limit: lim,
        }
    }

    fn spans_mut<'a>(&'a mut self, rect: Rect, out: &mut Vec<Span<'a, P>>) -> bool {
        let r = rect.intersect(&self.surf.rect());
        let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(&r) else { return true };
        // Prepare everything first, then hand out disjoint pieces.
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
        self.prepare_limits(r);
        let (t, shift, cols) = (self.surf.tile, self.surf.shift, self.surf.cols as usize);
        let Stroke { surf, before, flat, opened, mask, limit, .. } = self;
        let limit: Option<&'a Limits<'_>> = limit.as_ref();
        let (before, flat, opened): (&'a Snapshot<P>, &'a FlatBases<P>, &'a OpenBases<P>) = (before, flat, opened);
        // Only the tiles of the rect: rows of the table, then their part.
        let rows = surf.slots.chunks_mut(cols).zip(mask.chunks_mut(cols)).enumerate().skip(ty0 as usize).take((ty1 - ty0) as usize);
        for (ty, (srow, mrow)) in rows {
            for (k, (slot, m)) in srow[tx0 as usize..tx1 as usize].iter_mut().zip(mrow[tx0 as usize..tx1 as usize].iter_mut()).enumerate() {
                let (tx, i) = (tx0 + k as u32, ty * cols + tx0 as usize + k);
                let (Slot::Full(px), Some(m)) = (slot, m.as_deref_mut()) else { continue };
                let px = &mut Arc::make_mut(px).px[..];
                let base = base_of(before, flat, opened, i);
                let (bx, by) = (tx * t, ty as u32 * t);
                let part = Rect::from_xywh(bx as i32, by as i32, t, t).intersect(&r);
                if part.is_empty() { continue; }
                let (a, z) = ((part.x0 as u32 - bx) as usize, (part.x1 as u32 - bx) as usize);
                let (ly0, ly1) = ((part.y0 as u32 - by) as usize, (part.y1 as u32 - by) as usize);
                let tu = t as usize;
                for (ly, (prow, mrow)) in px.chunks_mut(tu).zip(m.chunks_mut(tu)).enumerate().take(ly1).skip(ly0) {
                    let y = by + ly as u32;
                    let row_base = base.get(ly << shift..).unwrap_or(&[]);
                    let (mut p_seg, mut m_seg) = (&mut prow[a..z], &mut mrow[a..z]);
                    let mut x = part.x0 as u32;
                    while !m_seg.is_empty() {
                        let len = match limit { Some(l) => ((((x >> l.surf.shift) + 1) << l.surf.shift) - x) as usize, None => usize::MAX }.min(m_seg.len());
                        let (p1, p2) = std::mem::take(&mut p_seg).split_at_mut(len);
                        let (m1, m2) = std::mem::take(&mut m_seg).split_at_mut(len);
                        (p_seg, m_seg) = (p2, m2);
                        let lim = limit_of(limit, x, y, len);
                        // A selection that covers nothing here: nothing to paint.
                        if !lim.is_some_and(|l| l.iter().all(|v| *v == 0)) {
                            let o = (x - bx) as usize;
                            out.push(Span { x, y, pixels: p1, base: row_base.get(o..o + len).unwrap_or(&[]), mask: m1, limit: lim });
                        }
                        x += len as u32;
                    }
                }
            }
        }
        true
    }

    fn base_pixel(&self, x: u32, y: u32) -> P {
        let s = &self.surf;
        if x >= s.width || y >= s.height { return self.before.fill; }
        let i = s.index(x >> s.shift, y >> s.shift);
        let o = (((y & (s.tile - 1)) << s.shift) + (x & (s.tile - 1))) as usize;
        pixel_of(self.before.slot(i), self.before.fill, o, s.npx())
    }
}

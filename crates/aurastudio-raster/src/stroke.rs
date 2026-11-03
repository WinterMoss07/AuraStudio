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
use crate::grid::{join, split, CH};
use crate::slot::Slot;
use crate::surface::{pixel_of, Surface};
use crate::DirtyTiles;

/// Shared base tiles of one color, by color.
type FlatBases<P> = HashMap<P, Arc<Vec<P>>>;
/// Base tiles opened from a coverage or cold form, by tile index.
type OpenBases<P> = HashMap<(i32, i32), Arc<Vec<P>>>;

struct Limits {
    /// (Its own copy: tiles are shared, so it costs the table only, and a
    /// stroke can be set aside between pen samples.)
    surf: Surface<u8>,
    /// One-value rows for uniform limit tiles, by value.
    flat: HashMap<u8, Box<[u8]>>,
    /// Limit tiles opened from a cold form, by tile.
    open: HashMap<(i32, i32), Vec<u8>>,
}

/// Tiles in a grid chunk.
const CN: usize = (CH * CH) as usize;

/// Masks of one chunk of tiles (same chunks as the surface's grid).
type MaskChunk = Vec<Option<Box<[u8]>>>;

pub struct Stroke<'s, P: Pixel> {
    surf:    &'s mut Surface<P>,
    before:  Snapshot<P>,
    flat:    FlatBases<P>,
    opened:  OpenBases<P>,
    /// Highest coverage per pixel, per touched tile (pooled buffers), by
    /// chunk of the grid.
    mask:    HashMap<(i32, i32), MaskChunk>,
    limit:   Option<Limits>,
    touched: DirtyTiles,
}

impl<P: Pixel> Surface<P> {
    /// Start a stroke; `limit` (a one-channel surface, any tile size)
    /// caps the coverage the engine may give each pixel (none outside the
    /// limit's stored area unless its fill allows it).
    pub fn begin_stroke(&mut self, limit: Option<&Surface<u8>>) -> Stroke<'_, P> { self.stroke_state(limit).on(self) }

    /// The same, as a state to set aside between pen samples (the
    /// document keeps it while its lock is let go): `on` takes it up
    /// again, on this surface only.
    pub fn stroke_state(&self, limit: Option<&Surface<u8>>) -> StrokeState<P> {
        let limit = limit.map(|surf| Limits { surf: surf.clone(), flat: HashMap::new(), open: HashMap::new() });
        StrokeState { before: self.snapshot(), flat: HashMap::new(), opened: HashMap::new(), mask: HashMap::new(), limit, touched: self.dirty_set() }
    }
}

/// A stroke set aside (see `Surface::stroke_state`).
pub struct StrokeState<P: Pixel> {
    before:  Snapshot<P>,
    flat:    FlatBases<P>,
    opened:  OpenBases<P>,
    mask:    HashMap<(i32, i32), MaskChunk>,
    limit:   Option<Limits>,
    touched: DirtyTiles,
}

impl<P: Pixel> StrokeState<P> {
    /// Go on painting on `surf` (the surface it was taken from).
    pub fn on(self, surf: &mut Surface<P>) -> Stroke<'_, P> {
        let StrokeState { before, flat, opened, mask, limit, touched } = self;
        Stroke { surf, before, flat, opened, mask, limit, touched }
    }

    /// Where it painted so far.
    pub fn touched(&self) -> &DirtyTiles { &self.touched }
}

impl<P: Pixel> Stroke<'_, P> {
    /// Set it aside (the surface is let go; `StrokeState::on` goes on).
    pub fn detach(self) -> StrokeState<P> {
        let Stroke { before, flat, opened, mask, limit, touched, .. } = self;
        StrokeState { before, flat, opened, mask, limit, touched }
    }

    /// The tiles the stroke went through so far.
    pub fn touched(&self) -> &DirtyTiles { &self.touched }

    /// The surface as it is now (for the merge while painting).
    pub fn surface(&self) -> &Surface<P> { self.surf }

    fn mask_of(&self, tx: i32, ty: i32) -> Option<&[u8]> {
        let (k, i) = split(tx, ty);
        self.mask.get(&k)?.get(i)?.as_deref()
    }

    /// Coverage of the stroke at a pixel so far.
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        let s = self.surf.shift;
        self.mask_of(x >> s, y >> s).map_or(0, |m| m[self.surf.offset(x, y)])
    }

    /// The end: touched tiles settled, masks back to the pool, and the
    /// patch that undoes the stroke (only the tiles that changed).
    pub fn end(self) -> Patch<P> {
        let Stroke { surf, before, mask, touched, .. } = self;
        for m in mask.into_values().flatten().flatten() { crate::pool::give_mask(m); }
        surf.compact_set(&touched);
        surf.diff_set(&before, &touched)
    }

    /// Make tile (tx, ty) ready to paint: its pixels unshared (copied from
    /// the base once), its mask, its base readable as pixels.
    fn prepare_tile(&mut self, tx: i32, ty: i32) {
        let (k, i) = split(tx, ty);
        let n = self.surf.npx();
        let chunk = self.mask.entry(k).or_insert_with(|| (0..CN).map(|_| None).collect());
        if chunk[i].is_some() { return; }
        chunk[i] = Some(crate::pool::take_mask(n));
        match self.before.slot(tx, ty) {
            Slot::Full(_) => {}
            s @ (Slot::Fill | Slot::Uniform(_)) => {
                let c = s.color(self.before.fill).unwrap_or(self.before.fill);
                self.flat.entry(c).or_insert_with(|| Arc::new(vec![c; n]));
            }
            s @ (Slot::Alpha(_) | Slot::Cold(_)) => { let px = s.expand(self.before.fill, n); self.opened.insert((tx, ty), Arc::new(px)); }
        }
        self.surf.full_mut(tx, ty);
        self.touched.insert(self.surf.tile_rect(tx, ty));
    }

    fn prepare_limit(&mut self, x: i32, y: i32) {
        let Some(l) = &mut self.limit else { return };
        let s = &l.surf;
        let (lx, ly) = (x >> s.shift, y >> s.shift);
        match s.slot(lx, ly) {
            Slot::Full(_) => {}
            c @ (Slot::Fill | Slot::Uniform(_)) => {
                let c = c.color(s.fill).unwrap_or(s.fill);
                l.flat.entry(c).or_insert_with(|| vec![c; s.tile as usize].into_boxed_slice());
            }
            other => { l.open.entry((lx, ly)).or_insert_with(|| other.expand(s.fill, s.npx())); }
        }
    }

    /// Prepare the limit for every limit tile `r` touches.
    fn prepare_limits(&mut self, r: Rect) {
        let Some(l) = &self.limit else { return };
        if r.is_empty() { return; }
        let step = l.surf.tile as i32;
        let mut y = r.y0.div_euclid(step) * step;
        while y < r.y1 {
            let mut x = r.x0.div_euclid(step) * step;
            while x < r.x1 { self.prepare_limit(x, y); x += step; }
            y += step;
        }
    }
}

/// The base pixels of tile (tx, ty).
fn base_of<'a, P: Pixel>(before: &'a Snapshot<P>, flat: &'a FlatBases<P>, opened: &'a OpenBases<P>, tx: i32, ty: i32) -> &'a [P] {
    match before.slot(tx, ty) {
        Slot::Full(p) => &p.px,
        s @ (Slot::Fill | Slot::Uniform(_)) => {
            let c = s.color(before.fill).unwrap_or(before.fill);
            flat.get(&c).map_or(&[][..], |v| &v[..])
        }
        _ => opened.get(&(tx, ty)).map_or(&[][..], |v| &v[..]),
    }
}

/// The limit for `len` pixels from (x, y), `None` without a limit.
fn limit_of(l: Option<&Limits>, x: i32, y: i32, len: usize) -> Option<&[u8]> {
    let l = l?;
    let s = &l.surf;
    let (lx, ly) = (x >> s.shift, y >> s.shift);
    let o = s.offset(x, y);
    match s.slot(lx, ly) {
        Slot::Full(p) => p.px.get(o..o + len),
        c @ (Slot::Fill | Slot::Uniform(_)) => l.flat.get(&c.color(s.fill).unwrap_or(s.fill)).and_then(|v| v.get(..len)),
        _ => l.open.get(&(lx, ly)).and_then(|v| v.get(o..o + len)),
    }
}

impl<P: Pixel> PaintSurface<P> for Stroke<'_, P> {
    fn frame(&self) -> Rect { self.surf.rect() }

    fn prepare(&mut self, rect: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(&rect) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
        self.prepare_limits(rect);
    }

    fn span_mut(&mut self, x: i32, y: i32, max: usize) -> Span<'_, P> {
        let (t, shift) = (self.surf.tile as i32, self.surf.shift);
        let (tx, ty) = (x >> shift, y >> shift);
        let mut len = max.min(((tx + 1) * t - x) as usize);
        if let Some(l) = &self.limit { len = len.min((((x >> l.surf.shift) + 1) * l.surf.tile as i32 - x) as usize); }
        if len == 0 { return Span { x, y, pixels: &mut [], base: &[], mask: &mut [], limit: None }; }
        self.prepare_tile(tx, ty);
        self.prepare_limit(x, y);
        let o = self.surf.offset(x, y);
        let (k, i) = split(tx, ty);
        let Stroke { surf, before, flat, opened, mask, limit, .. } = self;
        let base = base_of(before, flat, opened, tx, ty);
        let lim = limit_of(limit.as_ref(), x, y, len);
        let pixels = match surf.grid.get_mut(tx, ty) { Slot::Full(p) => Arc::make_mut(p).px.as_mut_slice(), _ => &mut [] };
        let m = mask.get_mut(&k).and_then(|c| c[i].as_deref_mut()).unwrap_or(&mut []);
        Span {
            x, y,
            pixels: pixels.get_mut(o..o + len).unwrap_or(&mut []),
            base: base.get(o..o + len).unwrap_or(&[]),
            mask: m.get_mut(o..o + len).unwrap_or(&mut []),
            limit: lim,
        }
    }

    fn spans_mut<'a>(&'a mut self, rect: Rect, out: &mut Vec<Span<'a, P>>) -> bool {
        self.spans_in(std::slice::from_ref(&rect), out)
    }

    fn spans_in<'a>(&'a mut self, rects: &[Rect], out: &mut Vec<Span<'a, P>>) -> bool {
        // Prepare everything first, then hand out disjoint pieces.
        let mut all = Rect::EMPTY;
        for r in rects {
            let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(r) else { continue };
            for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
            self.prepare_limits(*r);
            all = all.union(&Rect::new(tx0, ty0, tx1, ty1));
        }
        if all.is_empty() { return true; }
        let (tx0, ty0, tx1, ty1) = (all.x0, all.y0, all.x1, all.y1);
        let (t, shift) = (self.surf.tile as i32, self.surf.shift);
        let Stroke { surf, before, flat, opened, mask, limit, .. } = self;
        let limit: Option<&'a Limits> = limit.as_ref();
        let (before, flat, opened): (&'a Snapshot<P>, &'a FlatBases<P>, &'a OpenBases<P>) = (before, flat, opened);
        let ck = |v: i32| v.div_euclid(CH);
        for (key, chunk) in surf.grid.chunks.iter_mut() {
            if key.0 < ck(tx0) || key.0 > ck(tx1 - 1) || key.1 < ck(ty0) || key.1 > ck(ty1 - 1) { continue; }
            let Some(mc) = mask.get_mut(key) else { continue };
            let mc: *mut MaskChunk = mc;
            // SAFETY: every grid chunk key is visited once, so each mask
            // chunk is borrowed once; the mask map itself is not touched
            // while the spans live (`self` is borrowed for 'a).
            let mc: &'a mut MaskChunk = unsafe { &mut *mc };
            for (i, (slot, m)) in chunk.slots.iter_mut().zip(mc.iter_mut()).enumerate() {
                let (tx, ty) = join(*key, i);
                if tx < tx0 || tx >= tx1 || ty < ty0 || ty >= ty1 { continue; }
                let (Slot::Full(px), Some(m)) = (slot, m.as_deref_mut()) else { continue };
                let px = &mut Arc::make_mut(px).px[..];
                let base = base_of(before, flat, opened, tx, ty);
                let (bx, by) = (tx * t, ty * t);
                let tile_rect = Rect::new(bx, by, bx + t, by + t);
                let parts: Vec<Rect> = rects.iter().map(|r| tile_rect.intersect(r)).filter(|p| !p.is_empty()).collect();
                if parts.is_empty() { continue; }
                let tu = t as usize;
                let mut cuts: Vec<(usize, usize)> = Vec::with_capacity(parts.len());
                for (ly, (prow, mrow)) in px.chunks_mut(tu).zip(m.chunks_mut(tu)).enumerate() {
                    let y = by + ly as i32;
                    // This row's pieces, left to right (the rects are disjoint).
                    cuts.clear();
                    cuts.extend(parts.iter().filter(|p| p.y0 <= y && y < p.y1).map(|p| ((p.x0 - bx) as usize, (p.x1 - bx) as usize)));
                    if cuts.is_empty() { continue; }
                    cuts.sort_unstable();
                    let row_base = base.get(ly << shift..).unwrap_or(&[]);
                    let (mut prest, mut mrest, mut off) = (prow, mrow, 0usize);
                    for &(a, z) in &cuts {
                        let (_, p) = std::mem::take(&mut prest).split_at_mut(a - off);
                        let (_, mm) = std::mem::take(&mut mrest).split_at_mut(a - off);
                        let (mut p_seg, p_after) = p.split_at_mut(z - a);
                        let (mut m_seg, m_after) = mm.split_at_mut(z - a);
                        (prest, mrest, off) = (p_after, m_after, z);
                        let mut x = bx + a as i32;
                        while !m_seg.is_empty() {
                            let len = match limit { Some(l) => (((x >> l.surf.shift) + 1) * l.surf.tile as i32 - x) as usize, None => usize::MAX }.min(m_seg.len());
                            let (p1, p2) = std::mem::take(&mut p_seg).split_at_mut(len);
                            let (m1, m2) = std::mem::take(&mut m_seg).split_at_mut(len);
                            (p_seg, m_seg) = (p2, m2);
                            let lim = limit_of(limit, x, y, len);
                            // A selection that covers nothing here: nothing to paint.
                            if !lim.is_some_and(|l| l.iter().all(|v| *v == 0)) {
                                let o = (x - bx) as usize;
                                out.push(Span { x, y, pixels: p1, base: row_base.get(o..o + len).unwrap_or(&[]), mask: m1, limit: lim });
                            }
                            x += len as i32;
                        }
                    }
                }
            }
        }
        true
    }

    fn base_pixel(&self, x: i32, y: i32) -> P {
        let s = &self.surf;
        pixel_of(self.before.slot(x >> s.shift, y >> s.shift), self.before.fill, s.offset(x, y), s.npx())
    }
}

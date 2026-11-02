//! A stroke in progress on an RGBA8 surface: what the brush engine paints
//! through (`core::PaintSurface`). The base is a snapshot taken at the
//! start (free: tiles are shared, a tile is only copied when first
//! written); the mask keeps each pixel's highest coverage; the limit (a
//! selection's coverage, any tile size) caps what the engine may cover, so
//! a selection clips the stroke at the source. The end compacts the
//! touched tiles and gives the undo patch.

use std::collections::HashMap;
use std::sync::Arc;

use aurastudio_core::{PaintSurface, Rect, Rgba8, Span};

use crate::patch::{Patch, Snapshot};
use crate::surface::{Slot, Surface};

/// Shared base tiles of one color, by color.
type FlatBases = HashMap<Rgba8, Arc<Vec<Rgba8>>>;
/// Shared limit rows of one value, by value.
type FlatLimits = HashMap<u8, Box<[u8]>>;

pub struct Stroke<'s> {
    surf:    &'s mut Surface<Rgba8>,
    before:  Snapshot<Rgba8>,
    /// Base pixels of the base tiles that were one color, shared by every
    /// tile of that color.
    flat:    FlatBases,
    /// Highest coverage per pixel, per touched tile.
    mask:    Vec<Option<Box<[u8]>>>,
    limit:   Option<&'s Surface<u8>>,
    /// One-value rows for uniform limit tiles.
    flat_limit: FlatLimits,
    /// Area touched.
    touched: Rect,
}

impl Surface<Rgba8> {
    /// Start a stroke; `limit` (a one-channel surface of the same size)
    /// caps the coverage the engine may give each pixel.
    pub fn begin_stroke<'s>(&'s mut self, limit: Option<&'s Surface<u8>>) -> Stroke<'s> {
        let before = self.snapshot();
        let n = (self.cols * self.rows) as usize;
        let limit = limit.filter(|l| (l.width, l.height) == (self.width, self.height));
        Stroke { surf: self, before, flat: HashMap::new(), mask: (0..n).map(|_| None).collect(), limit, flat_limit: HashMap::new(), touched: Rect::EMPTY }
    }
}

impl Stroke<'_> {
    /// Where the stroke went so far (whole tiles).
    pub fn touched(&self) -> Rect { self.touched }

    /// The surface as it is now (for the merge while painting).
    pub fn surface(&self) -> &Surface<Rgba8> { self.surf }

    /// Coverage of the stroke at a pixel so far.
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        let s = self.surf.shift;
        if x < 0 || y < 0 || x as u32 >= self.surf.width || y as u32 >= self.surf.height { return 0; }
        let (x, y) = (x as u32, y as u32);
        let i = self.surf.index(x >> s, y >> s);
        self.mask[i].as_ref().map_or(0, |m| m[(((y & (self.surf.tile - 1)) << s) + (x & (self.surf.tile - 1))) as usize])
    }

    /// The end: touched tiles compacted (a tile painted back to one color
    /// stores no pixels), and the patch that undoes the stroke.
    pub fn end(self) -> Patch<Rgba8> {
        let Stroke { surf, before, touched, .. } = self;
        surf.compact(touched);
        surf.diff_in(&before, touched)
    }

    /// Make tile (tx, ty) ready to paint: its pixels unshared (copied from
    /// the base once), its mask, its flat base if it was one color.
    fn prepare_tile(&mut self, tx: u32, ty: u32) {
        let i = self.surf.index(tx, ty);
        if self.mask[i].is_some() { return; }
        let n = (self.surf.tile * self.surf.tile) as usize;
        self.mask[i] = Some(vec![0u8; n].into_boxed_slice());
        let base = match self.before.slots.get(i) { Some(s) => s.clone(), None => Slot::Fill };
        if !matches!(base, Slot::Full(_)) {
            let c = base.color(self.before.fill);
            self.flat.entry(c).or_insert_with(|| Arc::new(vec![c; n]));
        }
        self.surf.full_mut(tx, ty);
        self.touched = self.touched.union(&self.surf.tile_rect(tx, ty));
    }

    fn prepare_limit(&mut self, x: u32, y: u32) {
        let Some(l) = self.limit else { return };
        let s = l.slot(x >> l.shift, y >> l.shift);
        if !matches!(s, Slot::Full(_)) {
            let c = s.color(l.fill);
            let t = l.tile as usize;
            self.flat_limit.entry(c).or_insert_with(|| vec![c; t].into_boxed_slice());
        }
    }
}

/// The base pixels of tile `i` (from the snapshot, or the shared flat
/// tile of its color).
fn base_of<'a>(before: &'a Snapshot<Rgba8>, flat: &'a FlatBases, i: usize) -> &'a [Rgba8] {
    match before.slots.get(i) {
        Some(Slot::Full(p)) => p,
        s => {
            let c = s.map_or(before.fill, |s| s.color(before.fill));
            flat.get(&c).map_or(&[][..], |v| &v[..])
        }
    }
}

/// The limit for `len` pixels from (x, y), `None` without a limit.
fn limit_of<'a>(limit: Option<&'a Surface<u8>>, flat: &'a FlatLimits, x: u32, y: u32, len: usize) -> Option<&'a [u8]> {
    let l = limit?;
    let t = l.tile;
    match l.slot(x >> l.shift, y >> l.shift) {
        Slot::Full(p) => { let o = (((y & (t - 1)) << l.shift) + (x & (t - 1))) as usize; p.get(o..o + len) }
        s => flat.get(&s.color(l.fill)).and_then(|v| v.get(..len)),
    }
}

impl PaintSurface for Stroke<'_> {
    fn width(&self) -> u32 { self.surf.width }
    fn height(&self) -> u32 { self.surf.height }

    fn prepare(&mut self, rect: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(&rect) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
    }

    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_> {
        let (t, shift) = (self.surf.tile, self.surf.shift);
        let (tx, ty) = (x >> shift, y >> shift);
        let mut len = max.min((((tx + 1) << shift) - x) as usize).min(self.surf.width.saturating_sub(x) as usize);
        if let Some(l) = self.limit { len = len.min((((x >> l.shift) + 1) << l.shift) as usize - x as usize); }
        if y >= self.surf.height || len == 0 {
            return Span { x, y, pixels: &mut [], base: &[], mask: &mut [], limit: None };
        }
        self.prepare_tile(tx, ty);
        self.prepare_limit(x, y);
        let i = self.surf.index(tx, ty);
        let o = (((y & (t - 1)) << shift) + (x & (t - 1))) as usize;
        let Stroke { surf, before, flat, mask, limit, flat_limit, .. } = self;
        let base = base_of(before, flat, i);
        let limit = limit_of(*limit, flat_limit, x, y, len);
        let pixels = match surf.slots.get_mut(i) { Some(Slot::Full(p)) => Arc::make_mut(p).as_mut_slice(), _ => &mut [] };
        let m = mask[i].as_deref_mut().unwrap_or(&mut []);
        Span {
            x, y,
            pixels: pixels.get_mut(o..o + len).map_or(&mut [][..], |p| p.as_flattened_mut()),
            base: base.get(o..o + len).map_or(&[][..], |b| b.as_flattened()),
            mask: m.get_mut(o..o + len).unwrap_or(&mut []),
            limit,
        }
    }

    fn spans_mut<'a>(&'a mut self, rect: Rect, out: &mut Vec<Span<'a>>) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.surf.tiles_of(&rect) else { return true };
        let r = rect.intersect(&self.surf.rect());
        // Prepare everything first, then hand out disjoint pieces.
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.prepare_tile(tx, ty); } }
        if let Some(l) = self.limit {
            let step = l.tile.min(self.surf.tile) as i32;
            let mut y = r.y0;
            while y < r.y1 {
                let mut x = r.x0;
                while x < r.x1 { self.prepare_limit(x as u32, y as u32); x = (x / step + 1) * step; }
                y = (y / step + 1) * step;
            }
        }
        let (t, shift, cols) = (self.surf.tile, self.surf.shift, self.surf.cols);
        let Stroke { surf, before, flat, mask, limit, flat_limit, .. } = self;
        let limit: Option<&'a Surface<u8>> = *limit;
        let (before, flat, flat_limit): (&'a Snapshot<Rgba8>, &'a FlatBases, &'a FlatLimits) = (before, flat, flat_limit);
        // Only the tiles of the rect: rows of the table, then their part.
        let tile_rows = surf.slots.chunks_mut(cols as usize).zip(mask.chunks_mut(cols as usize)).enumerate().skip(ty0 as usize).take((ty1 - ty0) as usize);
        let cells = tile_rows.flat_map(|(ty, (srow, mrow))| {
            srow[tx0 as usize..tx1 as usize].iter_mut().zip(mrow[tx0 as usize..tx1 as usize].iter_mut()).enumerate()
                .map(move |(k, c)| (ty * cols as usize + tx0 as usize + k, c))
        });
        for (i, (slot, m)) in cells {
            let (tx, ty) = (i as u32 % cols, i as u32 / cols);
            let (Slot::Full(px), Some(m)) = (slot, m.as_deref_mut()) else { continue };
            let px = Arc::make_mut(px);
            let base = base_of(before, flat, i);
            let (bx, by) = (tx * t, ty * t);
            let part = Rect::from_xywh(bx as i32, by as i32, t, t).intersect(&r);
            if part.is_empty() { continue; }
            // Rows of this tile inside `part`, each cut at the limit's grid.
            let mut prow: &mut [Rgba8] = &mut px[..];
            let mut mrow: &mut [u8] = m;
            for ly in 0..t {
                let (pn, mn) = ((t as usize).min(prow.len()), (t as usize).min(mrow.len()));
                let (p_this, p_rest) = std::mem::take(&mut prow).split_at_mut(pn);
                let (m_this, m_rest) = std::mem::take(&mut mrow).split_at_mut(mn);
                prow = p_rest;
                mrow = m_rest;
                let y = by + ly;
                if (y as i32) < part.y0 || (y as i32) >= part.y1 { continue; }
                let (a, z) = ((part.x0 as u32 - bx) as usize, (part.x1 as u32 - bx) as usize);
                let mut p_seg = &mut p_this[a..z];
                let mut m_seg = &mut m_this[a..z];
                let mut x = part.x0 as u32;
                let row_base = &base[((ly << shift) as usize).min(base.len())..];
                while !m_seg.is_empty() {
                    let len = match limit { Some(l) => ((((x >> l.shift) + 1) << l.shift) - x) as usize, None => usize::MAX }.min(m_seg.len());
                    let (p1, p2) = std::mem::take(&mut p_seg).split_at_mut(len);
                    let (m1, m2) = std::mem::take(&mut m_seg).split_at_mut(len);
                    p_seg = p2;
                    m_seg = m2;
                    let lim = limit_of(limit, flat_limit, x, y, len);
                    // A selection that covers nothing here: nothing to paint.
                    if !lim.is_some_and(|l| l.iter().all(|v| *v == 0)) {
                        let o = (x - bx) as usize;
                        let b = row_base.get(o..o + len).map_or(&[][..], |b| b.as_flattened());
                        out.push(Span { x, y, pixels: p1.as_flattened_mut(), base: b, mask: m1, limit: lim });
                    }
                    x += len as u32;
                }
            }
        }
        true
    }
}

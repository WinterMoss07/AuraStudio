//! `Surface<P>`: the pixels of one layer or mask, unbounded (the canvas
//! has no edge: what is painted past the document's frame is kept, for
//! the camera and for layers moved partly out), in square tiles whose size
//! the surface chose at creation. A tile is the fill (nothing stored), one
//! color, one color with a coverage byte per pixel (alpha only: line art),
//! pixels, or cold (compressed). Forms change by themselves; pixel tiles
//! are shared (copy on write) between a surface, its copies, its snapshots
//! and the stroke base until one side writes.

use std::sync::Arc;

use aurastudio_core::{Fnv, HeapSize, Pixel, Rect};

use crate::grid::Grid;
use crate::slot::{decode, encode, info_of, info_of_color, AlphaTile, ColdTile, FullTile, Info, Slot, Summary};

/// Smallest and largest tile side.
pub const MIN_TILE: u32 = 8;
pub const MAX_TILE: u32 = 256;

/// A run of one row, as the storage holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run<'a, P: Pixel> {
    /// `n` pixels of one color.
    Uniform(P, usize),
    Pixels(&'a [P]),
    /// One color with a coverage per pixel (`P::join_alpha(color, a)`;
    /// `clear` where the coverage is 0). Merges use it as is, never
    /// widened to pixels.
    Coverage { color: P, clear: P, alpha: &'a [u8] },
}

impl<P: Pixel> Run<'_, P> {
    #[inline]
    pub fn len(&self) -> usize { match self { Run::Uniform(_, n) => *n, Run::Pixels(p) => p.len(), Run::Coverage { alpha, .. } => alpha.len() } }
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    /// Pixel `i` of the run.
    #[inline]
    pub fn at(&self, i: usize) -> P {
        match self { Run::Uniform(c, _) => *c, Run::Pixels(p) => p[i], Run::Coverage { color, clear, alpha } => if alpha[i] == 0 { *clear } else { P::join_alpha(*color, alpha[i]) } }
    }
}

/// Tiles by form (Memory report).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    pub uniform: usize,
    pub alpha:   usize,
    pub full:    usize,
    pub cold:    usize,
}

impl Stored {
    pub fn total(&self) -> usize { self.uniform + self.alpha + self.full + self.cold }
}

/// The coarse picture of a part of a surface: one premultiplied average
/// per tile (thumbnails, zoomed-out display, the first image when a file
/// opens). Cell (0, 0) is the tile at `origin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryGrid {
    /// Pixel position of cell (0, 0).
    pub origin: (i32, i32),
    pub cols:   u32,
    pub rows:   u32,
    /// Pixels each cell stands for, on a side.
    pub cell:   u32,
    pub avg:    Vec<[u8; 4]>,
}

#[derive(Clone, Debug)]
pub struct Surface<P: Pixel> {
    /// The document's frame: what it shows and exports by default.
    pub(crate) width:  u32,
    pub(crate) height: u32,
    pub(crate) tile:   u32,
    pub(crate) shift:  u32,
    pub(crate) grid:   Grid<P>,
    pub(crate) fill:   P,
}

impl<P: Pixel> Surface<P> {
    /// A surface whose frame is `width` × `height` (it holds pixels
    /// anywhere); `tile`: rounded to a power of two in `MIN_TILE..=MAX_TILE`.
    pub fn new(width: u32, height: u32, tile: u32, fill: P) -> Self {
        let tile = tile.clamp(MIN_TILE, MAX_TILE).next_power_of_two().min(MAX_TILE);
        Self { width, height, tile, shift: tile.trailing_zeros(), grid: Grid::default(), fill }
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    /// The frame, `(0, 0)` to `(width, height)`.
    pub fn rect(&self) -> Rect { Rect::of_size(self.width, self.height) }
    /// Change the frame (canvas size); the pixels stay where they are.
    pub fn set_frame(&mut self, width: u32, height: u32) { (self.width, self.height) = (width, height); }
    /// Side of this surface's tiles (for layout decisions only: callers
    /// work in rects, `chunks`, `DirtyTiles` and runs, never in tiles).
    pub fn tile_size(&self) -> u32 { self.tile }
    pub fn fill(&self) -> P { self.fill }
    /// An empty set of tiles shaped like this surface.
    pub fn dirty_set(&self) -> crate::DirtyTiles { crate::DirtyTiles::new(self.tile) }

    /// The same pixels as `o` over `rect`: tile by tile, a tile shared by
    /// both (or of one color in both) without reading its pixels.
    pub fn same_in(&self, o: &Self, rect: Rect) -> bool {
        if self.tile != o.tile { return self.read(rect) == o.read(rect); }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return true };
        (ty0..ty1).all(|ty| (tx0..tx1).all(|tx| {
            let (a, b) = (self.slot(tx, ty), o.slot(tx, ty));
            let shared = match (a, b) {
                (Slot::Full(x), Slot::Full(y)) => Arc::ptr_eq(x, y),
                (Slot::Alpha(x), Slot::Alpha(y)) => Arc::ptr_eq(x, y),
                (Slot::Cold(x), Slot::Cold(y)) => Arc::ptr_eq(x, y),
                _ => false,
            };
            if shared { return true; }
            if let (Some(x), Some(y)) = (a.color(self.fill), b.color(o.fill)) { return x == y; }
            let part = self.tile_rect(tx, ty).intersect(&rect);
            self.read(part) == o.read(part)
        }))
    }

    /// Rows `y0..` of the frame, full width (`rows`: whole rows), written
    /// as new tiles in their best form, on all cores (a picture opened, by
    /// bands). `y0` off the tile grid: an ordinary write.
    pub fn write_band(&mut self, y0: u32, rows: &[P]) {
        let w = self.width as usize;
        if w == 0 || rows.is_empty() || !rows.len().is_multiple_of(w) { return; }
        if !y0.is_multiple_of(self.tile) {
            let r = Rect::from_xywh(0, y0 as i32, self.width, (rows.len() / w) as u32);
            self.write(r, rows);
            self.compact(r);
            return;
        }
        for (tx, ty, s) in self.band_slots(y0, rows) { self.set_slot(tx, ty, s); }
        self.drop_table_if_empty();
    }

    /// The whole frame from `rows(y0, out)`, which fills `out` with the
    /// frame's rows from `y0` (full width): tile rows made on all cores.
    pub fn fill_frame(&mut self, rows: impl Fn(u32, &mut [P]) + Sync) {
        use rayon::prelude::*;
        let (w, h, t) = (self.width, self.height, self.tile);
        let this = &*self;
        let slots: Vec<Vec<(i32, i32, Slot<P>)>> = (0..h).step_by(t as usize).collect::<Vec<_>>().into_par_iter().map(|y0| {
            let mut buf = vec![this.fill; (w * t.min(h - y0)) as usize];
            rows(y0, &mut buf);
            this.band_slots(y0, &buf)
        }).collect();
        for (tx, ty, s) in slots.into_iter().flatten() { self.set_slot(tx, ty, s); }
        self.drop_table_if_empty();
    }

    /// The tiles of whole rows from `y0` (on the tile grid; any number of
    /// rows), each in its best form.
    fn band_slots(&self, y0: u32, rows: &[P]) -> Vec<(i32, i32, Slot<P>)> {
        use rayon::prelude::*;
        let (w, t) = (self.width as usize, self.tile as usize);
        let n = rows.len() / w.max(1);
        let (cols, bands) = (w.div_ceil(t), n.div_ceil(t));
        (0..cols * bands).into_par_iter().map(|k| {
            let (tx, band) = (k % cols, k / cols);
            let mut px = vec![self.fill; t * t];
            let (x0, ry) = (tx * t, band * t);
            let cw = t.min(w - x0);
            for y in 0..t.min(n - ry) { let o = (ry + y) * w + x0; px[y * t..y * t + cw].copy_from_slice(&rows[o..o + cw]); }
            let ty = (y0 >> self.shift) as i32 + band as i32;
            (tx as i32, ty, self.settled_slot(Slot::Full(Arc::new(FullTile { px, info: None }))))
        }).filter(|(_, _, s)| !s.is_fill()).collect()
    }

    /// Change the fill color (the Paper's color): every pixel not stored
    /// takes it; stored tiles keep theirs (a stored tile of the new color
    /// becomes the fill).
    pub fn set_fill(&mut self, fill: P) {
        self.fill = fill;
        for (_, _, s) in self.grid.iter_mut() { if matches!(s, Slot::Uniform(c) if *c == fill) { *s = Slot::Fill; } }
        self.grid.prune();
    }

    #[inline]
    pub(crate) fn npx(&self) -> usize { (self.tile * self.tile) as usize }

    #[inline]
    pub(crate) fn slot(&self, tx: i32, ty: i32) -> &Slot<P> { self.grid.get(tx, ty) }

    /// The canvas rect of tile (tx, ty).
    #[inline]
    pub(crate) fn tile_rect(&self, tx: i32, ty: i32) -> Rect {
        let t = self.tile as i32;
        Rect::from_xywh(tx * t, ty * t, self.tile, self.tile)
    }

    /// Tiles covering `r`: (tx0, ty0, tx1, ty1), end excluded.
    #[inline]
    pub(crate) fn tiles_of(&self, r: &Rect) -> Option<(i32, i32, i32, i32)> {
        if r.is_empty() { return None; }
        let s = self.shift;
        Some((r.x0 >> s, r.y0 >> s, ((r.x1 - 1) >> s) + 1, ((r.y1 - 1) >> s) + 1))
    }

    /// Offset of pixel (x, y) inside its tile.
    #[inline]
    pub(crate) fn offset(&self, x: i32, y: i32) -> usize {
        let m = self.tile as i32 - 1;
        (((y & m) << self.shift) + (x & m)) as usize
    }

    pub(crate) fn set_slot(&mut self, tx: i32, ty: i32, s: Slot<P>) { self.grid.set(tx, ty, s); }

    pub(crate) fn drop_table_if_empty(&mut self) { self.grid.prune(); }

    /// The pixels of tile (tx, ty) to write into: any other form becomes
    /// pixels; a shared tile is copied first.
    pub(crate) fn full_mut(&mut self, tx: i32, ty: i32) -> &mut Vec<P> {
        let (n, fill) = (self.npx(), self.fill);
        let slot = self.grid.get_mut(tx, ty);
        let shared = match slot { Slot::Full(t) => Arc::strong_count(t) > 1 || Arc::weak_count(t) > 0, _ => true };
        if shared {
            let px = match &*slot { Slot::Full(t) => t.px.clone(), other => other.expand(fill, n) };
            *slot = Slot::Full(Arc::new(FullTile { px, info: None }));
        }
        match slot {
            Slot::Full(t) => {
                // (Unique now: `make_mut` does not copy.)
                let t = Arc::make_mut(t);
                t.info = None;
                &mut t.px
            }
            // (Made Full just above.)
            _ => unreachable!(),
        }
    }

    /// Tiles stored, by form.
    pub fn stored(&self) -> Stored {
        let mut s = Stored::default();
        for (_, _, x) in self.grid.iter() {
            match x { Slot::Fill => {} Slot::Uniform(_) => s.uniform += 1, Slot::Alpha(_) => s.alpha += 1, Slot::Full(_) => s.full += 1, Slot::Cold(_) => s.cold += 1 }
        }
        s
    }

    /// One pixel, anywhere.
    pub fn pixel(&self, x: i32, y: i32) -> P {
        pixel_of(self.slot(x >> self.shift, y >> self.shift), self.fill, self.offset(x, y), self.npx())
    }

    /// Row `y` from `x0` to `x1`, as the storage holds it: `f(x, run)` from
    /// left to right.
    pub fn row(&self, y: i32, x0: i32, x1: i32, mut f: impl FnMut(i32, Run<'_, P>)) {
        let (t, n, shift) = (self.tile as i32, self.npx(), self.shift);
        let ty = y >> shift;
        let mut x = x0;
        while x < x1 {
            let tx = x >> shift;
            let end = ((tx + 1) * t).min(x1);
            let len = (end - x) as usize;
            let s = self.offset(x, y);
            match self.slot(tx, ty) {
                Slot::Cold(c) => match decode::<P>(&c.data, n) {
                    Some(open) => emit(&open, self.fill, x, s, len, &mut f),
                    None => f(x, Run::Uniform(self.fill, len)),
                },
                slot => emit(slot, self.fill, x, s, len, &mut f),
            }
            x = end;
        }
    }

    /// Every row of `rect`, runs as `row` gives them: `f(y, x, run)`.
    pub fn rows(&self, rect: Rect, mut f: impl FnMut(i32, i32, Run<'_, P>)) {
        if rect.is_empty() { return; }
        for y in rect.y0..rect.y1 { self.row(y, rect.x0, rect.x1, |x, run| f(y, x, run)); }
    }

    /// The pixels of `rect`, rows tightly packed.
    pub fn read(&self, rect: Rect) -> Vec<P> {
        let mut out = vec![self.fill; rect.area() as usize];
        self.read_into(rect, &mut out);
        out
    }

    /// `read` into the caller's buffer (`rect.w() × rect.h()` pixels).
    pub fn read_into(&self, rect: Rect, out: &mut [P]) {
        let w = rect.w() as usize;
        if out.len() != w * rect.h() as usize || rect.is_empty() { return; }
        for y in rect.y0..rect.y1 {
            let row = (y - rect.y0) as usize * w;
            self.row(y, rect.x0, rect.x1, |x, run| {
                let o = row + (x - rect.x0) as usize;
                match run {
                    Run::Pixels(p) => out[o..o + p.len()].copy_from_slice(p),
                    Run::Uniform(c, n) => out[o..o + n].fill(c),
                    r @ Run::Coverage { .. } => for (i, d) in out[o..o + r.len()].iter_mut().enumerate() { *d = r.at(i); },
                }
            });
        }
    }

    /// Write `data` (rows of `rect`, tightly packed) into the surface.
    /// Written tiles settle in their best form.
    pub fn write(&mut self, rect: Rect, data: &[P]) {
        let rw = rect.w() as usize;
        if data.len() != rw * rect.h() as usize { return; }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let tr = self.tile_rect(tx, ty);
                let part = tr.intersect(&rect);
                let src_row = |y: i32| { let s = (y - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize; &data[s..s + part.w() as usize] };
                // The whole tile in one color: no pixels needed.
                if part == tr {
                    let c = data[(part.y0 - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize];
                    if (part.y0..part.y1).all(|y| src_row(y).iter().all(|p| *p == c)) {
                        self.set_slot(tx, ty, if c == self.fill { Slot::Fill } else { Slot::Uniform(c) });
                        continue;
                    }
                }
                // Only the slot's own color written over it: nothing changes.
                if let Some(c) = self.slot(tx, ty).color(self.fill) {
                    if (part.y0..part.y1).all(|y| src_row(y).iter().all(|p| *p == c)) { continue; }
                }
                let shift = self.shift;
                let rows: Vec<&[P]> = (part.y0..part.y1).map(src_row).collect();
                let px = self.full_mut(tx, ty);
                for (k, src) in rows.into_iter().enumerate() {
                    let d = (((part.y0 + k as i32 - tr.y0) << shift) + (part.x0 - tr.x0)) as usize;
                    px[d..d + src.len()].copy_from_slice(src);
                }
                self.settle(tx, ty);
            }
        }
        self.drop_table_if_empty();
    }

    /// Every pixel of `rect` this color: covered tiles become that color
    /// (no pixels), edge tiles get pixels.
    pub fn fill_rect(&mut self, rect: Rect, c: P) {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let tr = self.tile_rect(tx, ty);
                let part = tr.intersect(&rect);
                if part == tr {
                    self.set_slot(tx, ty, if c == self.fill { Slot::Fill } else { Slot::Uniform(c) });
                    continue;
                }
                if self.slot(tx, ty).color(self.fill) == Some(c) { continue; }
                let shift = self.shift;
                let px = self.full_mut(tx, ty);
                for y in part.y0..part.y1 {
                    let d = (((y - tr.y0) << shift) + (part.x0 - tr.x0)) as usize;
                    px[d..d + part.w() as usize].fill(c);
                }
                self.settle(tx, ty);
            }
        }
        self.drop_table_if_empty();
    }

    /// Back to the fill everywhere (frees every tile and the table).
    pub fn clear(&mut self) {
        self.grid.clear();
    }

    /// Back to the fill outside `keep` (crop to the frame, or to a
    /// selection's box).
    pub fn clear_outside(&mut self, keep: Rect) {
        let cells: Vec<(i32, i32)> = self.grid.iter().map(|(tx, ty, _)| (tx, ty)).collect();
        for (tx, ty) in cells {
            let tr = self.tile_rect(tx, ty);
            if !tr.overlaps(&keep) { self.set_slot(tx, ty, Slot::Fill); continue; }
            if keep.contains_rect(&tr) { continue; }
            let fill = self.fill;
            for part in [Rect::new(tr.x0, tr.y0, tr.x1, keep.y0), Rect::new(tr.x0, keep.y1, tr.x1, tr.y1), Rect::new(tr.x0, tr.y0, keep.x0, tr.y1), Rect::new(keep.x1, tr.y0, tr.x1, tr.y1)] {
                let p = part.intersect(&tr);
                if !p.is_empty() { self.fill_rect(p, fill); }
            }
        }
        self.drop_table_if_empty();
    }

    /// Put tile (tx, ty) in its best form: one color (or the fill), one
    /// color with coverage, or pixels with their summary and fingerprint.
    pub(crate) fn settle(&mut self, tx: i32, ty: i32) {
        let (t, n, fill) = (self.tile as usize, self.npx(), self.fill);
        let form = match self.slot(tx, ty) {
            Slot::Full(f) if f.info.is_none() => analyze(&f.px, t, fill),
            s @ (Slot::Alpha(_) | Slot::Cold(_)) if !settled(s) => analyze(&s.expand(fill, n), t, fill),
            _ => return,
        };
        self.apply(tx, ty, form);
    }

    fn apply(&mut self, tx: i32, ty: i32, form: Form<P>) {
        let (fill, n) = (self.fill, self.npx());
        match form {
            Form::One(c) => self.set_slot(tx, ty, if c == fill { Slot::Fill } else { Slot::Uniform(c) }),
            Form::Alpha(a) => self.set_slot(tx, ty, Slot::Alpha(Arc::new(a))),
            Form::Pixels(info) => match self.grid.get_mut(tx, ty) {
                Slot::Full(f) => match Arc::get_mut(f) {
                    Some(f) => f.info = Some(info),
                    None => { let px = f.px.clone(); *f = Arc::new(FullTile { px, info: Some(info) }); }
                },
                s => { let px = s.expand(fill, n); *s = Slot::Full(Arc::new(FullTile { px, info: Some(info) })); }
            },
        }
    }

    /// A slot opened and put in its best form (loading, on any thread).
    pub(crate) fn settled_slot(&self, s: Slot<P>) -> Slot<P> {
        let (t, n, fill) = (self.tile as usize, self.npx(), self.fill);
        let form = match &s {
            Slot::Full(f) => analyze(&f.px, t, fill),
            Slot::Alpha(_) | Slot::Cold(_) => analyze(&s.expand(fill, n), t, fill),
            Slot::Uniform(c) if *c == fill => return Slot::Fill,
            _ => return s,
        };
        match form {
            Form::One(c) => if c == fill { Slot::Fill } else { Slot::Uniform(c) },
            Form::Alpha(a) => Slot::Alpha(Arc::new(a)),
            Form::Pixels(info) => match s {
                Slot::Full(f) => { let px = Arc::try_unwrap(f).map_or_else(|f| f.px.clone(), |f| f.px); Slot::Full(Arc::new(FullTile { px, info: Some(info) })) }
                other => Slot::Full(Arc::new(FullTile { px: other.expand(fill, n), info: Some(info) })),
            },
        }
    }

    /// Settle the tiles touching `rect` (after painting there).
    pub fn compact(&mut self, rect: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.settle(tx, ty); } }
        self.drop_table_if_empty();
    }

    /// Settle the tiles of a set (exact: a diagonal stroke's band only).
    pub fn compact_set(&mut self, set: &crate::DirtyTiles) {
        let cells: Vec<(i32, i32)> = set.tiles().collect();
        for (tx, ty) in cells { self.settle(tx, ty); }
        self.drop_table_if_empty();
    }

    /// The summary and fingerprint of tile (tx, ty) (computed on the spot
    /// for a tile being painted).
    pub(crate) fn info(&self, tx: i32, ty: i32) -> Info {
        let t = self.tile as usize;
        match self.slot(tx, ty) {
            Slot::Fill => info_of_color(self.fill, t, t),
            Slot::Uniform(c) => info_of_color(*c, t, t),
            Slot::Full(f) => f.info.unwrap_or_else(|| info_of(&f.px, t, t, t)),
            Slot::Alpha(a) if a.info != Info::default() => a.info,
            Slot::Cold(c) if c.info != Info::default() => c.info,
            s => info_of(&s.expand(self.fill, self.npx()), t, t, t),
        }
    }

    /// Independent pieces covering `rect` for work on several cores: whole
    /// tiles grouped to about `target` pixels a side, never smaller than a
    /// tile.
    pub fn chunks(&self, rect: Rect, target: u32) -> Vec<Rect> {
        if rect.is_empty() { return Vec::new(); }
        let step = target.max(self.tile).next_power_of_two() as i32;
        let g = rect.snapped_out(step as u32);
        let mut out = Vec::new();
        let mut y = g.y0;
        while y < g.y1 {
            let mut x = g.x0;
            while x < g.x1 {
                let c = Rect::new(x, y, x + step, y + step).intersect(&rect);
                if !c.is_empty() { out.push(c); }
                x += step;
            }
            y += step;
        }
        out
    }

    /// Stored tiles' area (painted once, maybe erased since), anywhere;
    /// `None` when nothing is stored.
    pub fn stored_bounds(&self) -> Option<Rect> {
        let b = self.grid.iter().fold(Rect::EMPTY, |b, (tx, ty, _)| b.union(&self.tile_rect(tx, ty)));
        (!b.is_empty()).then_some(b)
    }

    /// Area of the tiles holding a visible pixel; `None`: nothing shows.
    /// A visible fill covers the frame (and is everywhere past it).
    pub fn visible_bounds(&self) -> Option<Rect> {
        let mut b = if self.fill.visible() { self.rect() } else { Rect::EMPTY };
        for (tx, ty, _) in self.grid.iter() { if !self.info(tx, ty).sum.empty() { b = b.union(&self.tile_rect(tx, ty)); } }
        (!b.is_empty()).then_some(b)
    }

    /// Exact bounds of the visible pixels; `None`: nothing shows. A visible
    /// fill counts as the frame.
    pub fn pixel_bounds(&self) -> Option<Rect> {
        let mut b = if self.fill.visible() { self.rect() } else { Rect::EMPTY };
        for (tx, ty, _) in self.grid.iter() {
            let info = self.info(tx, ty);
            if info.sum.empty() { continue; }
            let tr = self.tile_rect(tx, ty);
            let [x0, y0, x1, y1] = info.sum.bbox.map(|v| v as i32);
            b = b.union(&Rect::new(tr.x0 + x0, tr.y0 + y0, tr.x0 + x1, tr.y0 + y1));
        }
        (!b.is_empty()).then_some(b)
    }

    /// A visible pixel inside `rect`.
    pub fn any_visible_in(&self, rect: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return false };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                if self.info(tx, ty).sum.empty() { continue; }
                let tr = self.tile_rect(tx, ty);
                if rect.contains_rect(&tr) { return true; }
                let part = tr.intersect(&rect);
                let mut found = false;
                for y in part.y0..part.y1 {
                    self.row(y, part.x0, part.x1, |_, run| found |= (0..run.len()).any(|i| run.at(i).visible()));
                    if found { return true; }
                }
            }
        }
        false
    }

    /// Every pixel of `rect` fully covering (the merge stops below such a
    /// tile).
    pub fn opaque_in(&self, rect: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return false };
        (ty0..ty1).all(|ty| (tx0..tx1).all(|tx| {
            if self.info(tx, ty).sum.opaque() { return true; }
            let part = self.tile_rect(tx, ty).intersect(&rect);
            let mut all = true;
            for y in part.y0..part.y1 { self.row(y, part.x0, part.x1, |_, run| all &= (0..run.len()).all(|i| run.at(i).opaque())); }
            all
        }))
    }

    /// Nothing but the fill inside `rect`.
    pub fn is_fill_in(&self, rect: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return true };
        if self.grid.is_empty() { return true; }
        let fill = self.fill;
        (ty0..ty1).all(|ty| (tx0..tx1).all(|tx| match self.slot(tx, ty) {
            Slot::Fill => true,
            Slot::Uniform(c) => *c == fill,
            _ => {
                let part = self.tile_rect(tx, ty).intersect(&rect);
                let mut all = true;
                for y in part.y0..part.y1 { self.row(y, part.x0, part.x1, |_, run| all &= (0..run.len()).all(|i| run.at(i) == fill)); }
                all
            }
        }))
    }

    /// The one color of every pixel of `rect`, when its tiles are all of
    /// that color (a tile holding pixels counts as mixed: filters skip
    /// what this finds without looking at pixels). `None` for an empty
    /// rect.
    pub fn uniform_in(&self, rect: Rect) -> Option<P> {
        let (tx0, ty0, tx1, ty1) = self.tiles_of(&rect)?;
        let mut c = None;
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let s = self.slot(tx, ty).color(self.fill)?;
                if *c.get_or_insert(s) != s { return None; }
            }
        }
        c
    }

    /// One premultiplied average per tile of `rect` (the frame by default).
    pub fn summary_grid(&self, rect: Option<Rect>) -> SummaryGrid {
        let r = rect.unwrap_or_else(|| self.rect());
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&r) else { return SummaryGrid { origin: (r.x0, r.y0), cols: 0, rows: 0, cell: self.tile, avg: Vec::new() } };
        let mut avg = Vec::with_capacity(((tx1 - tx0) * (ty1 - ty0)) as usize);
        for ty in ty0..ty1 { for tx in tx0..tx1 { avg.push(self.info(tx, ty).sum.avg); } }
        let t = self.tile as i32;
        SummaryGrid { origin: (tx0 * t, ty0 * t), cols: (tx1 - tx0) as u32, rows: (ty1 - ty0) as u32, cell: self.tile, avg }
    }

    /// A key of the content of `rect` (its tiles' fingerprints): equal
    /// keys, equal pixels (caches of merges, incremental saves).
    pub fn fingerprint(&self, rect: Rect) -> u64 {
        let mut h = Fnv::default();
        h.u32(self.tile);
        if let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) {
            for ty in ty0..ty1 { for tx in tx0..tx1 { h.u64(self.info(tx, ty).fp); } }
        }
        h.finish()
    }

    /// Compress the pixel and coverage tiles outside `keep` (layers not
    /// shown, sheets far from the current frame): read on demand, opened
    /// again when painted. Tiles that do not compress stay as they are.
    pub fn freeze(&mut self, keep: Rect) {
        let n = self.npx();
        let cells: Vec<(i32, i32)> = self.grid.iter().filter(|(_, _, s)| matches!(s, Slot::Full(_) | Slot::Alpha(_))).map(|(x, y, _)| (x, y)).collect();
        for (tx, ty) in cells {
            if self.tile_rect(tx, ty).overlaps(&keep) { continue; }
            let info = self.info(tx, ty);
            let s = self.slot(tx, ty);
            let data = encode(s);
            let raw = match s { Slot::Alpha(a) => a.a.len(), _ => n * P::BYTES };
            if data.len() >= raw { continue; }
            self.set_slot(tx, ty, Slot::Cold(Arc::new(ColdTile { data, info })));
        }
    }

    /// Open every cold tile again.
    pub fn thaw(&mut self) {
        let n = self.npx();
        let mut cells = Vec::new();
        for (tx, ty, s) in self.grid.iter_mut() {
            if let Slot::Cold(c) = s {
                if let Some(open) = decode::<P>(&c.data, n) { *s = open; cells.push((tx, ty)); }
            }
        }
        for (tx, ty) in cells { self.settle(tx, ty); }
    }

    /// A copy whose tiles are shared until one side writes (cheap).
    pub fn snapshot(&self) -> crate::Snapshot<P> {
        crate::Snapshot { grid: self.grid.clone(), tile: self.tile, fill: self.fill }
    }
}

/// The best form for a tile's pixels.
pub(crate) enum Form<P: Pixel> {
    One(P),
    Alpha(AlphaTile<P>),
    Pixels(Info),
}

/// Whether a coverage or cold tile already has its summary.
fn settled<P: Pixel>(s: &Slot<P>) -> bool {
    match s { Slot::Alpha(a) => a.info != Info::default(), Slot::Cold(c) => c.info != Info::default(), _ => true }
}

/// Choose the form of a tile's `t × t` pixels without copying them.
pub(crate) fn analyze<P: Pixel>(px: &[P], t: usize, fill: P) -> Form<P> {
    let c = px[0];
    if px.iter().all(|p| *p == c) { return Form::One(c); }
    let info = info_of(px, t, t, t);
    if P::ALPHA_FORM {
        // One color for every visible pixel, one value for the others.
        let (mut color, mut clear) = (None, None);
        let ok = px.iter().all(|p| {
            if p.visible() { let (c, _) = p.split_alpha(); *color.get_or_insert(c) == c } else { *clear.get_or_insert(*p) == *p }
        });
        if let (true, Some(color)) = (ok, color) {
            let a = px.iter().map(|p| if p.visible() { p.split_alpha().1 } else { 0 }).collect();
            return Form::Alpha(AlphaTile { color, clear: clear.unwrap_or(fill), a, info });
        }
    }
    Form::Pixels(info)
}

/// Pixel `o` of a tile.
pub(crate) fn pixel_of<P: Pixel>(s: &Slot<P>, fill: P, o: usize, n: usize) -> P {
    match s {
        Slot::Fill => fill,
        Slot::Uniform(c) => *c,
        Slot::Full(t) => t.px[o],
        Slot::Alpha(t) => if t.a[o] == 0 { t.clear } else { P::join_alpha(t.color, t.a[o]) },
        Slot::Cold(c) => decode::<P>(&c.data, n).map_or(fill, |s| pixel_of(&s, fill, o, n)),
    }
}

fn emit<P: Pixel>(s: &Slot<P>, fill: P, x: i32, start: usize, len: usize, f: &mut impl FnMut(i32, Run<'_, P>)) {
    match s {
        Slot::Full(p) => f(x, Run::Pixels(&p.px[start..start + len])),
        Slot::Alpha(t) => f(x, Run::Coverage { color: t.color, clear: t.clear, alpha: &t.a[start..start + len] }),
        Slot::Uniform(c) => f(x, Run::Uniform(*c, len)),
        Slot::Fill | Slot::Cold(_) => f(x, Run::Uniform(fill, len)),
    }
}

impl<P: Pixel> HeapSize for Surface<P> {
    /// The table and the tiles' data (a tile shared with a copy counts in
    /// both: the report shows the most this surface could free).
    fn heap_bytes(&self) -> usize {
        let n = self.npx();
        self.grid.table_bytes() + self.grid.iter().map(|(_, _, s)| match s {
            Slot::Fill | Slot::Uniform(_) => 0,
            Slot::Alpha(a) => a.a.capacity(),
            Slot::Full(_) => n * P::BYTES,
            Slot::Cold(c) => c.data.capacity(),
        }).sum::<usize>()
    }
}

/// Same pixels everywhere (inside the frame and past it), whatever is
/// stored or shared.
impl<P: Pixel> PartialEq for Surface<P> {
    fn eq(&self, o: &Self) -> bool {
        if (self.width, self.height) != (o.width, o.height) { return false; }
        let area = [self.stored_bounds(), o.stored_bounds()].into_iter().flatten().fold(self.rect(), |a, r| a.union(&r));
        self.fill == o.fill && self.read(area) == o.read(area)
    }
}

impl Summary {
    /// (For `SummaryGrid` users that want the tile's flags too.)
    pub fn is_uniform(&self) -> bool { self.flags & Summary::UNIFORM != 0 }
}

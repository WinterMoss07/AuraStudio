//! `Surface<P>`: the pixels of one layer or mask, in square tiles whose
//! size the surface chose at creation. A tile is the fill (nothing
//! stored), one color, one color with a coverage byte per pixel (alpha
//! only: line art), pixels, or cold (compressed). Forms change by
//! themselves; pixel tiles are shared (copy on write) between a surface,
//! its copies, its snapshots and the stroke base until one side writes.

use std::sync::Arc;

use aurastudio_core::{Fnv, HeapSize, Pixel, Rect};

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

/// The coarse picture of a surface: one premultiplied average per tile
/// (thumbnails, zoomed-out display, the first image when a file opens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryGrid {
    pub cols: u32,
    pub rows: u32,
    /// Pixels each cell stands for, on a side.
    pub cell: u32,
    pub avg:  Vec<[u8; 4]>,
}

#[derive(Clone, Debug)]
pub struct Surface<P: Pixel> {
    pub(crate) width:  u32,
    pub(crate) height: u32,
    pub(crate) tile:   u32,
    pub(crate) shift:  u32,
    pub(crate) cols:   u32,
    pub(crate) rows:   u32,
    /// Empty until the first slot that is not Fill (an empty layer has no
    /// table either), then `cols × rows`.
    pub(crate) slots:  Vec<Slot<P>>,
    pub(crate) fill:   P,
}

impl<P: Pixel> Surface<P> {
    /// `tile`: rounded to a power of two in `MIN_TILE..=MAX_TILE`.
    pub fn new(width: u32, height: u32, tile: u32, fill: P) -> Self {
        let tile = tile.clamp(MIN_TILE, MAX_TILE).next_power_of_two().min(MAX_TILE);
        let shift = tile.trailing_zeros();
        Self { width, height, tile, shift, cols: width.div_ceil(tile), rows: height.div_ceil(tile), slots: Vec::new(), fill }
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn rect(&self) -> Rect { Rect::of_size(self.width, self.height) }
    /// Side of this surface's tiles (for layout decisions only: callers
    /// work in rects, `chunks`, `DirtyTiles` and runs, never in tiles).
    pub fn tile_size(&self) -> u32 { self.tile }
    pub fn fill(&self) -> P { self.fill }
    /// An empty set of tiles shaped like this surface.
    pub fn dirty_set(&self) -> crate::DirtyTiles { crate::DirtyTiles::new(self.width, self.height, self.tile) }

    /// Change the fill color (the Paper's color): every pixel not stored
    /// takes it; stored tiles keep theirs (a stored tile of the new color
    /// becomes the fill).
    pub fn set_fill(&mut self, fill: P) {
        self.fill = fill;
        for s in &mut self.slots { if matches!(s, Slot::Uniform(c) if *c == fill) { *s = Slot::Fill; } }
        self.drop_table_if_empty();
    }

    #[inline]
    pub(crate) fn npx(&self) -> usize { (self.tile * self.tile) as usize }

    #[inline]
    pub(crate) fn index(&self, tx: u32, ty: u32) -> usize { (ty * self.cols + tx) as usize }

    #[inline]
    pub(crate) fn slot(&self, tx: u32, ty: u32) -> &Slot<P> {
        match self.slots.get(self.index(tx, ty)) { Some(s) => s, None => &Slot::Fill }
    }

    /// The canvas rect of tile (tx, ty), clipped to the surface.
    #[inline]
    pub(crate) fn tile_rect(&self, tx: u32, ty: u32) -> Rect {
        let t = self.tile as i32;
        let (x0, y0) = (tx as i32 * t, ty as i32 * t);
        Rect::new(x0, y0, (x0 + t).min(self.width as i32), (y0 + t).min(self.height as i32))
    }

    /// Tiles covering `r` (clipped): (tx0, ty0, tx1, ty1), end excluded.
    pub(crate) fn tiles_of(&self, r: &Rect) -> Option<(u32, u32, u32, u32)> {
        let (x0, y0, x1, y1) = r.clipped_u32(self.width, self.height)?;
        Some((x0 >> self.shift, y0 >> self.shift, ((x1 - 1) >> self.shift) + 1, ((y1 - 1) >> self.shift) + 1))
    }

    pub(crate) fn table(&mut self) -> &mut Vec<Slot<P>> {
        if self.slots.is_empty() { self.slots.resize((self.cols * self.rows) as usize, Slot::Fill); }
        &mut self.slots
    }

    pub(crate) fn set_slot(&mut self, tx: u32, ty: u32, s: Slot<P>) {
        if tx >= self.cols || ty >= self.rows { return; }
        if s.is_fill() && self.slots.is_empty() { return; }
        let i = self.index(tx, ty);
        let old = std::mem::replace(&mut self.table()[i], s);
        recycle(old);
    }

    pub(crate) fn drop_table_if_empty(&mut self) {
        if self.slots.iter().all(Slot::is_fill) { self.slots = Vec::new(); }
    }

    /// The pixels of tile (tx, ty) to write into: any other form becomes
    /// pixels; a shared tile is copied first (into a pooled buffer).
    pub(crate) fn full_mut(&mut self, tx: u32, ty: u32) -> &mut Vec<P> {
        let (n, fill) = (self.npx(), self.fill);
        let i = self.index(tx, ty);
        let slot = &mut self.table()[i];
        let shared = match slot { Slot::Full(t) => Arc::strong_count(t) > 1 || Arc::weak_count(t) > 0, _ => true };
        if shared {
            let mut px = crate::pool::take_pixels(n, fill);
            match &*slot {
                Slot::Full(t) => px.copy_from_slice(&t.px),
                other => { let e = other.expand(fill, n); px.copy_from_slice(&e); }
            }
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
        for x in &self.slots {
            match x { Slot::Fill => {} Slot::Uniform(_) => s.uniform += 1, Slot::Alpha(_) => s.alpha += 1, Slot::Full(_) => s.full += 1, Slot::Cold(_) => s.cold += 1 }
        }
        s
    }

    /// One pixel (outside the surface: the fill).
    pub fn pixel(&self, x: i32, y: i32) -> P {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height { return self.fill; }
        let (x, y) = (x as u32, y as u32);
        let o = (((y & (self.tile - 1)) << self.shift) + (x & (self.tile - 1))) as usize;
        pixel_of(self.slot(x >> self.shift, y >> self.shift), self.fill, o, self.npx())
    }

    /// Row `y` from `x0` to `x1` (clipped to the surface), as the storage
    /// holds it: `f(x, run)` from left to right.
    pub fn row(&self, y: i32, x0: i32, x1: i32, mut f: impl FnMut(i32, Run<'_, P>)) {
        if y < 0 || y as u32 >= self.height { return; }
        let (x0, x1) = (x0.max(0) as u32, (x1.max(0) as u32).min(self.width));
        let (y, t, n) = (y as u32, self.tile, self.npx());
        let (ty, ly) = (y >> self.shift, y & (t - 1));
        let mut x = x0;
        while x < x1 {
            let tx = x >> self.shift;
            let end = ((tx + 1) << self.shift).min(x1);
            let len = (end - x) as usize;
            let s = ((ly << self.shift) + (x & (t - 1))) as usize;
            match self.slot(tx, ty) {
                Slot::Cold(c) => match decode::<P>(&c.data, n) {
                    Some(open) => emit(&open, self.fill, x as i32, s, len, &mut f),
                    None => f(x as i32, Run::Uniform(self.fill, len)),
                },
                slot => emit(slot, self.fill, x as i32, s, len, &mut f),
            }
            x = end;
        }
    }

    /// Every row of `rect` (clipped), runs as `row` gives them: `f(y, x, run)`.
    pub fn rows(&self, rect: Rect, mut f: impl FnMut(i32, i32, Run<'_, P>)) {
        let r = rect.intersect(&self.rect());
        for y in r.y0..r.y1 { self.row(y, r.x0, r.x1, |x, run| f(y, x, run)); }
    }

    /// The pixels of `rect`, rows tightly packed; outside the surface: the
    /// fill.
    pub fn read(&self, rect: Rect) -> Vec<P> {
        let (w, h) = (rect.w() as usize, rect.h() as usize);
        let mut out = vec![self.fill; w * h];
        self.read_into(rect, &mut out);
        out
    }

    /// `read` into the caller's buffer (`rect.w() × rect.h()` pixels).
    pub fn read_into(&self, rect: Rect, out: &mut [P]) {
        let w = rect.w() as usize;
        if out.len() != w * rect.h() as usize { return; }
        out.fill(self.fill);
        let inside = rect.intersect(&self.rect());
        for y in inside.y0..inside.y1 {
            let row = (y - rect.y0) as usize * w;
            self.row(y, inside.x0, inside.x1, |x, run| {
                let o = row + (x - rect.x0) as usize;
                match run {
                    Run::Pixels(p) => out[o..o + p.len()].copy_from_slice(p),
                    Run::Uniform(c, n) => out[o..o + n].fill(c),
                    r @ Run::Coverage { .. } => for (i, d) in out[o..o + r.len()].iter_mut().enumerate() { *d = r.at(i); },
                }
            });
        }
    }

    /// Write `data` (rows of `rect`, tightly packed) into the surface; the
    /// part outside is ignored. Written tiles settle in their best form.
    pub fn write(&mut self, rect: Rect, data: &[P]) {
        let rw = rect.w() as usize;
        if data.len() != rw * rect.h() as usize { return; }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let tr = self.tile_rect(tx, ty);
                let part = tr.intersect(&rect);
                if part.is_empty() { continue; }
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
                if part.is_empty() { continue; }
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
        for s in std::mem::take(&mut self.slots) { recycle(s); }
    }

    /// Put tile (tx, ty) in its best form: one color (or the fill), one
    /// color with coverage, or pixels with their summary and fingerprint.
    pub(crate) fn settle(&mut self, tx: u32, ty: u32) {
        let r = self.tile_rect(tx, ty);
        let (w, h, t, n, fill) = (r.w() as usize, r.h() as usize, self.tile as usize, self.npx(), self.fill);
        let i = self.index(tx, ty);
        let form = match self.slots.get(i) {
            Some(Slot::Full(f)) if f.info.is_none() => analyze(&f.px, w, h, t, fill),
            Some(s @ (Slot::Alpha(_) | Slot::Cold(_))) if !settled(s) => analyze(&s.expand(fill, n), w, h, t, fill),
            _ => return,
        };
        self.apply(tx, ty, form);
    }

    fn apply(&mut self, tx: u32, ty: u32, form: Form<P>) {
        let (i, fill, n) = (self.index(tx, ty), self.fill, self.npx());
        match form {
            Form::One(c) => self.set_slot(tx, ty, if c == self.fill { Slot::Fill } else { Slot::Uniform(c) }),
            Form::Alpha(a) => self.set_slot(tx, ty, Slot::Alpha(Arc::new(a))),
            Form::Pixels(info) => match self.slots.get_mut(i) {
                Some(Slot::Full(f)) => match Arc::get_mut(f) {
                    Some(f) => f.info = Some(info),
                    None => { let px = f.px.clone(); *f = Arc::new(FullTile { px, info: Some(info) }); }
                },
                Some(s) => { let px = s.expand(fill, n); *s = Slot::Full(Arc::new(FullTile { px, info: Some(info) })); }
                None => {}
            },
        }
    }

    /// A slot opened and put in its best form (loading, on any thread).
    pub(crate) fn settled_slot(&self, s: Slot<P>, tx: u32, ty: u32) -> Slot<P> {
        let r = self.tile_rect(tx, ty);
        let (w, h, t, n, fill) = (r.w() as usize, r.h() as usize, self.tile as usize, self.npx(), self.fill);
        let form = match &s {
            Slot::Full(f) => analyze(&f.px, w, h, t, fill),
            Slot::Alpha(_) | Slot::Cold(_) => analyze(&s.expand(fill, n), w, h, t, fill),
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
        let cells: Vec<(u32, u32)> = set.tiles().collect();
        for (tx, ty) in cells { self.settle(tx, ty); }
        self.drop_table_if_empty();
    }

    /// The summary and fingerprint of tile (tx, ty) (computed on the spot
    /// for a tile being painted).
    pub(crate) fn info(&self, tx: u32, ty: u32) -> Info {
        let r = self.tile_rect(tx, ty);
        let (w, h) = (r.w() as usize, r.h() as usize);
        match self.slot(tx, ty) {
            Slot::Fill => info_of_color(self.fill, w, h),
            Slot::Uniform(c) => info_of_color(*c, w, h),
            Slot::Full(f) => f.info.unwrap_or_else(|| info_of(&f.px, w, h, self.tile as usize)),
            Slot::Alpha(a) if a.info != Info::default() => a.info,
            Slot::Cold(c) if c.info != Info::default() => c.info,
            s => info_of(&s.expand(self.fill, self.npx()), w, h, self.tile as usize),
        }
    }

    /// Independent pieces covering `rect` (clipped) for work on several
    /// cores: whole tiles grouped to about `target` pixels a side, never
    /// smaller than a tile.
    pub fn chunks(&self, rect: Rect, target: u32) -> Vec<Rect> {
        let r = rect.intersect(&self.rect());
        if r.is_empty() { return Vec::new(); }
        let step = target.max(self.tile).next_power_of_two() as i32;
        let g = r.snapped_out(step as u32);
        let mut out = Vec::new();
        let mut y = g.y0;
        while y < g.y1 {
            let mut x = g.x0;
            while x < g.x1 {
                let c = Rect::new(x, y, x + step, y + step).intersect(&r);
                if !c.is_empty() { out.push(c); }
                x += step;
            }
            y += step;
        }
        out
    }

    /// Stored tiles' area (painted once, maybe erased since); `None` when
    /// nothing is stored.
    pub fn stored_bounds(&self) -> Option<Rect> {
        let mut b = Rect::EMPTY;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.is_fill() { b = b.union(&self.tile_rect(i as u32 % self.cols, i as u32 / self.cols)); }
        }
        (!b.is_empty()).then_some(b)
    }

    /// Area of the tiles holding a visible pixel; `None`: nothing shows.
    /// A visible fill covers the whole surface.
    pub fn visible_bounds(&self) -> Option<Rect> {
        if self.fill.visible() { return Some(self.rect()); }
        let mut b = Rect::EMPTY;
        self.each_stored(|tx, ty, info| if !info.sum.empty() { b = b.union(&self.tile_rect(tx, ty)); });
        (!b.is_empty()).then_some(b)
    }

    /// Exact bounds of the visible pixels; `None`: nothing shows.
    pub fn pixel_bounds(&self) -> Option<Rect> {
        if self.fill.visible() { return Some(self.rect()); }
        let mut b = Rect::EMPTY;
        self.each_stored(|tx, ty, info| {
            if info.sum.empty() { return; }
            let tr = self.tile_rect(tx, ty);
            let [x0, y0, x1, y1] = info.sum.bbox.map(|v| v as i32);
            b = b.union(&Rect::new(tr.x0 + x0, tr.y0 + y0, tr.x0 + x1, tr.y0 + y1));
        });
        (!b.is_empty()).then_some(b)
    }

    fn each_stored(&self, mut f: impl FnMut(u32, u32, Info)) {
        for (i, s) in self.slots.iter().enumerate() {
            if s.is_fill() { continue; }
            let (tx, ty) = (i as u32 % self.cols, i as u32 / self.cols);
            f(tx, ty, self.info(tx, ty));
        }
    }

    /// A visible pixel inside `rect`.
    pub fn any_visible_in(&self, rect: Rect) -> bool {
        let r = rect.intersect(&self.rect());
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&r) else { return false };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let tr = self.tile_rect(tx, ty);
                let info = self.info(tx, ty);
                if info.sum.empty() { continue; }
                if r.contains_rect(&tr) { return true; }
                let part = tr.intersect(&r);
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
        let r = rect.intersect(&self.rect());
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&r) else { return false };
        (ty0..ty1).all(|ty| (tx0..tx1).all(|tx| {
            let info = self.info(tx, ty);
            if info.sum.opaque() { return true; }
            let part = self.tile_rect(tx, ty).intersect(&r);
            let mut all = true;
            for y in part.y0..part.y1 { self.row(y, part.x0, part.x1, |_, run| all &= (0..run.len()).all(|i| run.at(i).opaque())); }
            all
        }))
    }

    /// Nothing but the fill inside `rect`.
    pub fn is_fill_in(&self, rect: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return true };
        if self.slots.is_empty() { return true; }
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

    /// One premultiplied average per tile.
    pub fn summary_grid(&self) -> SummaryGrid {
        let mut avg = Vec::with_capacity((self.cols * self.rows) as usize);
        for ty in 0..self.rows { for tx in 0..self.cols { avg.push(self.info(tx, ty).sum.avg); } }
        SummaryGrid { cols: self.cols, rows: self.rows, cell: self.tile, avg }
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
    /// again when painted.
    pub fn freeze(&mut self, keep: Rect) {
        for i in 0..self.slots.len() {
            let (tx, ty) = (i as u32 % self.cols, i as u32 / self.cols);
            if self.tile_rect(tx, ty).overlaps(&keep) { continue; }
            if matches!(self.slots[i], Slot::Full(_) | Slot::Alpha(_)) {
                let info = self.info(tx, ty);
                let data = encode(&self.slots[i]);
                // (Noise does not compress: kept as it is.)
                let raw = match &self.slots[i] { Slot::Alpha(a) => a.a.len(), _ => self.npx() * P::BYTES };
                if data.len() >= raw { continue; }
                let old = std::mem::replace(&mut self.slots[i], Slot::Cold(Arc::new(ColdTile { data, info })));
                recycle(old);
            }
        }
    }

    /// Open every cold tile again.
    pub fn thaw(&mut self) {
        let n = self.npx();
        for s in &mut self.slots {
            if let Slot::Cold(c) = s {
                if let Some(open) = decode::<P>(&c.data, n) { *s = open; }
            }
        }
        let all: Vec<(u32, u32)> = (0..self.slots.len() as u32).map(|i| (i % self.cols, i / self.cols)).collect();
        for (tx, ty) in all { self.settle(tx, ty); }
    }

    /// A copy whose tiles are shared until one side writes (cheap).
    pub fn snapshot(&self) -> crate::Snapshot<P> {
        crate::Snapshot { slots: self.slots.clone(), cols: self.cols, rows: self.rows, tile: self.tile, fill: self.fill }
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

/// Choose the form of a tile's pixels (`w` × `h` on the surface, rows of
/// `t`) without copying them.
pub(crate) fn analyze<P: Pixel>(px: &[P], w: usize, h: usize, t: usize, fill: P) -> Form<P> {
    let on = |y: usize| &px[y * t..y * t + w];
    let c = px[0];
    if (0..h).all(|y| on(y).iter().all(|p| *p == c)) { return Form::One(c); }
    let info = info_of(px, w, h, t);
    if P::ALPHA_FORM {
        // One color for every visible pixel, one value for the others.
        let (mut color, mut clear) = (None, None);
        let ok = (0..h).all(|y| on(y).iter().all(|p| {
            if p.visible() { let (c, _) = p.split_alpha(); *color.get_or_insert(c) == c } else { *clear.get_or_insert(*p) == *p }
        }));
        if let (true, Some(color)) = (ok, color) {
            let mut a = vec![0u8; t * t];
            for y in 0..h { for (x, p) in on(y).iter().enumerate() { if p.visible() { a[y * t + x] = p.split_alpha().1; } } }
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

/// A slot let go: its pixel buffer back to the pool when nobody else
/// holds it.
pub(crate) fn recycle<P: Pixel>(s: Slot<P>) {
    if let Slot::Full(t) = s {
        if let Ok(t) = Arc::try_unwrap(t) { crate::pool::give_pixels(t.px); }
    }
}

impl<P: Pixel> HeapSize for Surface<P> {
    /// The table and the tiles' data (a tile shared with a copy counts in
    /// both: the report shows the most this surface could free).
    fn heap_bytes(&self) -> usize {
        let n = self.npx();
        self.slots.capacity() * std::mem::size_of::<Slot<P>>() + self.slots.iter().map(|s| match s {
            Slot::Fill | Slot::Uniform(_) => 0,
            Slot::Alpha(a) => a.a.capacity(),
            Slot::Full(_) => n * P::BYTES,
            Slot::Cold(c) => c.data.capacity(),
        }).sum::<usize>()
    }
}

/// Same pixels, whatever is stored or shared.
impl<P: Pixel> PartialEq for Surface<P> {
    fn eq(&self, o: &Self) -> bool {
        (self.width, self.height) == (o.width, o.height) && self.read(self.rect()) == o.read(o.rect())
    }
}

impl Summary {
    /// (For `SummaryGrid` users that want the tile's flags too.)
    pub fn is_uniform(&self) -> bool { self.flags & Summary::UNIFORM != 0 }
}

//! `Surface<P>`: the pixels of one layer or mask, in square tiles whose
//! size the surface chose at creation. Only tiles that differ from the
//! surface's fill exist; a tile of one color is that color, not pixels;
//! pixel tiles are shared (copy on write) between a surface, its copies,
//! its snapshots and the stroke base, until one of them writes.

use std::sync::Arc;

use aurastudio_core::{HeapSize, Pixel, Rect};

/// Smallest and largest tile side.
pub const MIN_TILE: u32 = 8;
pub const MAX_TILE: u32 = 256;

/// One square of the grid.
#[derive(Clone, Debug)]
pub(crate) enum Slot<P: Pixel> {
    /// The surface's fill color (nothing stored).
    Fill,
    /// Every pixel this color.
    Uniform(P),
    /// `tile × tile` pixels, row by row (edge tiles too: their part past
    /// the surface is never read).
    Full(Arc<Vec<P>>),
}

impl<P: Pixel> Slot<P> {
    /// The single color (`fill` for Fill); meaningless for Full.
    #[inline]
    pub(crate) fn color(&self, fill: P) -> P {
        match self { Slot::Fill => fill, Slot::Uniform(c) => *c, Slot::Full(t) => t.first().copied().unwrap_or(fill) }
    }
}

/// A run of one row, as the storage holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run<'a, P: Pixel> {
    /// `n` pixels of one color.
    Uniform(P, usize),
    Pixels(&'a [P]),
}

impl<P: Pixel> Run<'_, P> {
    #[inline]
    pub fn len(&self) -> usize { match self { Run::Uniform(_, n) => *n, Run::Pixels(p) => p.len() } }
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
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
    /// work in rects, `chunks` and runs, never in tiles).
    pub fn tile_size(&self) -> u32 { self.tile }
    pub fn fill(&self) -> P { self.fill }

    /// Change the fill color (the Paper's color): every pixel not stored
    /// takes it; stored tiles keep theirs (a stored tile of the new color
    /// becomes the fill).
    pub fn set_fill(&mut self, fill: P) {
        self.fill = fill;
        for s in &mut self.slots { if matches!(s, Slot::Uniform(c) if *c == fill) { *s = Slot::Fill; } }
        self.drop_table_if_empty();
    }

    #[inline]
    pub(crate) fn index(&self, tx: u32, ty: u32) -> usize { (ty * self.cols + tx) as usize }

    #[inline]
    pub(crate) fn slot(&self, tx: u32, ty: u32) -> &Slot<P> {
        static_fill(self.slots.get(self.index(tx, ty)))
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
        if matches!(s, Slot::Fill) && self.slots.is_empty() { return; }
        let i = self.index(tx, ty);
        self.table()[i] = s;
    }

    pub(crate) fn drop_table_if_empty(&mut self) {
        if self.slots.iter().all(|s| matches!(s, Slot::Fill)) { self.slots = Vec::new(); }
    }

    /// The pixels of tile (tx, ty) to write into: a uniform or fill tile
    /// becomes pixels of its color; a shared one is copied first.
    pub(crate) fn full_mut(&mut self, tx: u32, ty: u32) -> &mut Vec<P> {
        let n = (self.tile * self.tile) as usize;
        let fill = self.fill;
        let i = self.index(tx, ty);
        let slot = &mut self.table()[i];
        if !matches!(slot, Slot::Full(_)) {
            *slot = Slot::Full(Arc::new(vec![slot.color(fill); n]));
        }
        match slot {
            Slot::Full(t) => Arc::make_mut(t),
            // (Made Full just above.)
            _ => unreachable!(),
        }
    }

    /// Tiles stored as something other than the fill: (uniform, full).
    pub fn stored(&self) -> (usize, usize) {
        self.slots.iter().fold((0, 0), |(u, f), s| match s { Slot::Fill => (u, f), Slot::Uniform(_) => (u + 1, f), Slot::Full(_) => (u, f + 1) })
    }

    /// One pixel (outside the surface: the fill).
    pub fn pixel(&self, x: i32, y: i32) -> P {
        if x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height { return self.fill; }
        let (x, y) = (x as u32, y as u32);
        match self.slot(x >> self.shift, y >> self.shift) {
            Slot::Full(t) => t[(((y & (self.tile - 1)) << self.shift) + (x & (self.tile - 1))) as usize],
            s => s.color(self.fill),
        }
    }

    /// Row `y` from `x0` to `x1` (clipped to the surface), as the storage
    /// holds it: `f(x, run)` from left to right.
    pub fn row(&self, y: i32, x0: i32, x1: i32, mut f: impl FnMut(i32, Run<'_, P>)) {
        if y < 0 || y as u32 >= self.height { return; }
        let (x0, x1) = (x0.max(0) as u32, (x1.max(0) as u32).min(self.width));
        let (y, t) = (y as u32, self.tile);
        let (ty, ly) = (y >> self.shift, y & (t - 1));
        let mut x = x0;
        while x < x1 {
            let tx = x >> self.shift;
            let end = ((tx + 1) << self.shift).min(x1);
            let n = (end - x) as usize;
            match self.slot(tx, ty) {
                Slot::Full(p) => {
                    let s = ((ly << self.shift) + (x & (t - 1))) as usize;
                    f(x as i32, Run::Pixels(&p[s..s + n]));
                }
                s => f(x as i32, Run::Uniform(s.color(self.fill), n)),
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
        let inside = rect.intersect(&self.rect());
        for y in inside.y0..inside.y1 {
            let row = (y - rect.y0) as usize * w;
            self.row(y, inside.x0, inside.x1, |x, run| {
                let o = row + (x - rect.x0) as usize;
                match run {
                    Run::Pixels(p) => out[o..o + p.len()].copy_from_slice(p),
                    Run::Uniform(c, n) => out[o..o + n].fill(c),
                }
            });
        }
        out
    }

    /// Write `data` (rows of `rect`, tightly packed) into the surface; the
    /// part outside is ignored. A tile fully written with one color becomes
    /// that color; tiles left equal to the fill are freed.
    pub fn write(&mut self, rect: Rect, data: &[P]) {
        let rw = rect.w() as usize;
        if data.len() != rw * rect.h() as usize { return; }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let part = self.tile_rect(tx, ty).intersect(&rect);
                if part.is_empty() { continue; }
                let src_row = |y: i32| { let s = (y - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize; &data[s..s + part.w() as usize] };
                // The whole tile in one color: no pixels needed.
                if part == self.tile_rect(tx, ty) {
                    let c = data[(part.y0 - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize];
                    if (part.y0..part.y1).all(|y| src_row(y).iter().all(|p| *p == c)) {
                        let s = if c == self.fill { Slot::Fill } else { Slot::Uniform(c) };
                        self.set_slot(tx, ty, s);
                        continue;
                    }
                }
                // Only the slot's own color written over it: nothing changes.
                let cur = self.slot(tx, ty);
                if !matches!(cur, Slot::Full(_)) {
                    let c = cur.color(self.fill);
                    if (part.y0..part.y1).all(|y| src_row(y).iter().all(|p| *p == c)) { continue; }
                }
                let (t, shift) = (self.tile as i32, self.shift);
                let (bx, by) = (tx as i32 * t, ty as i32 * t);
                let rows: Vec<&[P]> = (part.y0..part.y1).map(src_row).collect();
                let px = self.full_mut(tx, ty);
                for (k, src) in rows.into_iter().enumerate() {
                    let d = (((part.y0 + k as i32 - by) << shift) + (part.x0 - bx)) as usize;
                    px[d..d + src.len()].copy_from_slice(src);
                }
                self.compact_tile(tx, ty);
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
                let cur = self.slot(tx, ty);
                if !matches!(cur, Slot::Full(_)) && cur.color(self.fill) == c { continue; }
                let (shift, bx, by) = (self.shift, tr.x0, tr.y0);
                let px = self.full_mut(tx, ty);
                for y in part.y0..part.y1 {
                    let d = (((y - by) << shift) + (part.x0 - bx)) as usize;
                    px[d..d + part.w() as usize].fill(c);
                }
                self.compact_tile(tx, ty);
            }
        }
        self.drop_table_if_empty();
    }

    /// Back to the fill everywhere (frees every tile and the table).
    pub fn clear(&mut self) { self.slots = Vec::new(); }

    /// If every pixel of tile (tx, ty) on the surface is one color, store
    /// it as that color (or as the fill).
    pub(crate) fn compact_tile(&mut self, tx: u32, ty: u32) {
        let Slot::Full(px) = self.slot(tx, ty) else { return };
        let r = self.tile_rect(tx, ty);
        let (w, shift) = (r.w() as usize, self.shift);
        let c = px[0];
        let one = (0..r.h()).all(|ly| { let s = (ly << shift) as usize; px[s..s + w].iter().all(|p| *p == c) });
        if one { self.set_slot(tx, ty, if c == self.fill { Slot::Fill } else { Slot::Uniform(c) }); }
    }

    /// Compact the tiles touching `rect` (after painting there).
    pub fn compact(&mut self, rect: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.compact_tile(tx, ty); } }
        self.drop_table_if_empty();
    }

    /// Independent pieces covering `rect` (clipped) for work on several
    /// cores: whole tiles grouped to about `target` pixels a side, never
    /// smaller than a tile. The caller does not need to know why they are
    /// cut there.
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
        self.bounds_of(|_| true)
    }

    /// Area of the tiles holding a visible pixel; `None`: nothing shows.
    /// A visible fill covers the whole surface.
    pub fn visible_bounds(&self) -> Option<Rect> {
        if self.fill.visible() { return Some(self.rect()); }
        self.bounds_of(|s| match s { Slot::Fill => false, Slot::Uniform(c) => c.visible(), Slot::Full(p) => p.iter().any(|p| p.visible()) })
    }

    fn bounds_of(&self, keep: impl Fn(&Slot<P>) -> bool) -> Option<Rect> {
        let mut b = Rect::EMPTY;
        for (i, s) in self.slots.iter().enumerate() {
            if matches!(s, Slot::Fill) || !keep(s) { continue; }
            b = b.union(&self.tile_rect(i as u32 % self.cols, i as u32 / self.cols));
        }
        (!b.is_empty()).then_some(b)
    }

    /// Exact bounds of the visible pixels; `None`: nothing shows.
    pub fn pixel_bounds(&self) -> Option<Rect> {
        if self.fill.visible() { return Some(self.rect()); }
        let mut b = Rect::EMPTY;
        for (i, s) in self.slots.iter().enumerate() {
            let (tx, ty) = (i as u32 % self.cols, i as u32 / self.cols);
            let tr = self.tile_rect(tx, ty);
            match s {
                Slot::Fill => {}
                Slot::Uniform(c) => if c.visible() { b = b.union(&tr); },
                Slot::Full(px) => {
                    // A tile that cannot widen the bounds is skipped.
                    if b.contains_rect(&tr) { continue; }
                    for ly in 0..tr.h() {
                        let row = &px[(ly << self.shift) as usize..((ly << self.shift) + tr.w()) as usize];
                        let Some(a) = row.iter().position(|p| p.visible()) else { continue };
                        let z = row.iter().rposition(|p| p.visible()).unwrap_or(a);
                        let y = tr.y0 + ly as i32;
                        b = b.union(&Rect::new(tr.x0 + a as i32, y, tr.x0 + z as i32 + 1, y + 1));
                    }
                }
            }
        }
        (!b.is_empty()).then_some(b)
    }

    /// A visible pixel inside `rect`.
    pub fn any_visible_in(&self, rect: Rect) -> bool {
        let mut found = false;
        let r = rect.intersect(&self.rect());
        for y in r.y0..r.y1 {
            self.row(y, r.x0, r.x1, |_, run| {
                if found { return; }
                found = match run { Run::Uniform(c, _) => c.visible(), Run::Pixels(p) => p.iter().any(|p| p.visible()) };
            });
            if found { return true; }
        }
        false
    }

    /// Nothing but the fill inside `rect`.
    pub fn is_fill_in(&self, rect: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&rect) else { return true };
        if self.slots.is_empty() { return true; }
        let fill = self.fill;
        (ty0..ty1).all(|ty| (tx0..tx1).all(|tx| match self.slot(tx, ty) {
            Slot::Fill => true,
            Slot::Uniform(c) => *c == fill,
            Slot::Full(_) => {
                let part = self.tile_rect(tx, ty).intersect(&rect);
                let mut all = true;
                for y in part.y0..part.y1 {
                    self.row(y, part.x0, part.x1, |_, run| all &= match run { Run::Uniform(c, _) => c == fill, Run::Pixels(p) => p.iter().all(|p| *p == fill) });
                }
                all
            }
        }))
    }

    /// The same pixels on a surface of another size, the old one placed at
    /// (dx, dy) (canvas resize, content centered); outside: the fill.
    pub fn resized(&self, width: u32, height: u32, dx: i32, dy: i32) -> Self {
        let mut out = Self::new(width, height, self.tile, self.fill);
        // Tile-aligned moves keep every tile as it is (shared).
        let t = self.tile as i32;
        if dx % t == 0 && dy % t == 0 {
            for (i, s) in self.slots.iter().enumerate() {
                if matches!(s, Slot::Fill) { continue; }
                let (tx, ty) = ((i as u32 % self.cols) as i32 + dx / t, (i as u32 / self.cols) as i32 + dy / t);
                if tx < 0 || ty < 0 { continue; }
                out.set_slot(tx as u32, ty as u32, s.clone());
            }
            // (Edge tiles of the old surface may now be inner tiles: their
            // part past the old edge must show the fill.)
            out.fix_old_edges(self.width as i32 + dx, self.height as i32 + dy);
            out.drop_table_if_empty();
            return out;
        }
        for (i, s) in self.slots.iter().enumerate() {
            if matches!(s, Slot::Fill) { continue; }
            let src = self.tile_rect(i as u32 % self.cols, i as u32 / self.cols);
            let dst = src.moved(dx, dy).intersect(&out.rect());
            if dst.is_empty() { continue; }
            out.write(dst, &self.read(dst.moved(-dx, -dy)));
        }
        out
    }

    /// Past the old right / bottom edge (now at `x_end`, `y_end`), the
    /// moved tiles hold leftovers: set them to the fill.
    fn fix_old_edges(&mut self, x_end: i32, y_end: i32) {
        let fill = self.fill;
        let right = Rect::new(x_end, 0, self.width as i32, y_end);
        let below = Rect::new(0, y_end, self.width as i32, self.height as i32);
        for r in [right, below] {
            let r = r.intersect(&self.rect());
            if r.is_empty() { continue; }
            let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&r) else { continue };
            for ty in ty0..ty1 {
                for tx in tx0..tx1 {
                    if let Slot::Full(_) = self.slot(tx, ty) {
                        let part = self.tile_rect(tx, ty).intersect(&r);
                        let (shift, bx, by) = (self.shift, tx as i32 * self.tile as i32, ty as i32 * self.tile as i32);
                        let px = self.full_mut(tx, ty);
                        for y in part.y0..part.y1 {
                            let d = (((y - by) << shift) + (part.x0 - bx)) as usize;
                            px[d..d + part.w() as usize].fill(fill);
                        }
                        self.compact_tile(tx, ty);
                    }
                }
            }
        }
    }

    /// The same pixels with another tile size (an explicit conversion).
    pub fn retiled(&self, tile: u32) -> Self {
        let mut out = Self::new(self.width, self.height, tile, self.fill);
        if out.tile == self.tile { out.slots = self.slots.clone(); return out; }
        for r in self.chunks(self.rect(), self.tile.max(out.tile)) {
            if self.is_fill_in(r) { continue; }
            out.write(r, &self.read(r));
        }
        out
    }

    /// A copy whose tiles are shared until one side writes (cheap).
    pub fn snapshot(&self) -> crate::Snapshot<P> {
        crate::Snapshot { slots: self.slots.clone(), cols: self.cols, rows: self.rows, tile: self.tile, fill: self.fill }
    }
}

impl<P: Pixel> HeapSize for Surface<P> {
    /// The table and the pixel tiles (a tile shared with a copy counts in
    /// both: the report shows the most this surface could free).
    fn heap_bytes(&self) -> usize {
        let px = (self.tile * self.tile) as usize * P::BYTES;
        self.slots.capacity() * std::mem::size_of::<Slot<P>>() + self.slots.iter().filter(|s| matches!(s, Slot::Full(_))).count() * px
    }
}

/// Same pixels, whatever is stored or shared.
impl<P: Pixel> PartialEq for Surface<P> {
    fn eq(&self, o: &Self) -> bool {
        (self.width, self.height) == (o.width, o.height) && self.read(self.rect()) == o.read(o.rect())
    }
}

fn static_fill<P: Pixel>(s: Option<&Slot<P>>) -> &Slot<P> {
    match s { Some(s) => s, None => &Slot::Fill }
}

//! A set of tiles, exact: what changed, what to upload, merge, mip or
//! save again. Unlike a union of rectangles, a diagonal stroke stays a
//! diagonal band of tiles. Callers see pixel rectangles only.

use aurastudio_core::Rect;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyTiles {
    tile:  u32,
    cols:  u32,
    rows:  u32,
    bits:  Vec<u64>,
    count: usize,
}

impl DirtyTiles {
    /// For a `width` × `height` surface of tiles of `tile`.
    pub fn new(width: u32, height: u32, tile: u32) -> Self {
        let tile = tile.max(1);
        let (cols, rows) = (width.div_ceil(tile), height.div_ceil(tile));
        Self { tile, cols, rows, bits: vec![0; ((cols * rows) as usize).div_ceil(64)], count: 0 }
    }

    pub fn is_empty(&self) -> bool { self.count == 0 }

    /// Tiles in the set.
    pub fn len(&self) -> usize { self.count }

    pub fn clear(&mut self) { self.bits.fill(0); self.count = 0; }

    #[inline]
    fn set(&mut self, i: usize) {
        let (w, b) = (i / 64, 1u64 << (i % 64));
        if self.bits[w] & b == 0 { self.bits[w] |= b; self.count += 1; }
    }

    #[inline]
    fn get(&self, i: usize) -> bool { self.bits[i / 64] & (1 << (i % 64)) != 0 }

    /// Add the tiles touching `r`.
    pub fn insert(&mut self, r: Rect) {
        let t = self.tile as i32;
        let full = Rect::new(0, 0, self.cols as i32 * t, self.rows as i32 * t);
        let Some(r) = r.intersection(&full) else { return };
        let (tx0, ty0, tx1, ty1) = (r.x0 / t, r.y0 / t, (r.x1 - 1) / t + 1, (r.y1 - 1) / t + 1);
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.set((ty as u32 * self.cols + tx as u32) as usize); } }
    }

    /// Add every tile of `o` (same shape; another shape is ignored).
    pub fn union(&mut self, o: &DirtyTiles) {
        if (o.tile, o.cols, o.rows) != (self.tile, self.cols, self.rows) { return; }
        for (a, b) in self.bits.iter_mut().zip(&o.bits) { *a |= b; }
        self.count = self.bits.iter().map(|w| w.count_ones() as usize).sum();
    }

    /// Whether some tile touching `r` is in the set.
    pub fn touches(&self, r: Rect) -> bool {
        let t = self.tile as i32;
        let full = Rect::new(0, 0, self.cols as i32 * t, self.rows as i32 * t);
        let Some(r) = r.intersection(&full) else { return false };
        let (tx0, ty0, tx1, ty1) = (r.x0 / t, r.y0 / t, (r.x1 - 1) / t + 1, (r.y1 - 1) / t + 1);
        (ty0..ty1).any(|ty| (tx0..tx1).any(|tx| self.get((ty as u32 * self.cols + tx as u32) as usize)))
    }

    /// The set as rectangles: runs of tiles side by side in each row,
    /// clipped to `width` × `height`.
    pub fn rects(&self, width: u32, height: u32) -> Vec<Rect> {
        let t = self.tile as i32;
        let full = Rect::of_size(width, height);
        let mut out = Vec::new();
        for ty in 0..self.rows {
            let mut tx = 0;
            while tx < self.cols {
                if !self.get((ty * self.cols + tx) as usize) { tx += 1; continue; }
                let start = tx;
                while tx < self.cols && self.get((ty * self.cols + tx) as usize) { tx += 1; }
                let r = Rect::new(start as i32 * t, ty as i32 * t, tx as i32 * t, (ty as i32 + 1) * t).intersect(&full);
                if !r.is_empty() { out.push(r); }
            }
        }
        out
    }

    /// The smallest rectangle around the set.
    pub fn bounds(&self, width: u32, height: u32) -> Rect { self.rects(width, height).iter().fold(Rect::EMPTY, |a, r| a.union(r)) }

    /// (tx, ty) of every tile in the set.
    pub(crate) fn tiles(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..self.cols * self.rows).filter(|i| self.get(*i as usize)).map(|i| (i % self.cols, i / self.cols))
    }
}

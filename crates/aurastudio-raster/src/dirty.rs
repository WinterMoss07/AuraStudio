//! A set of tiles, exact and unbounded: what changed, what to upload,
//! merge, mip or save again. Unlike a union of rectangles, a diagonal
//! stroke stays a diagonal band of tiles. Callers see pixel rectangles.

use std::collections::HashMap;

use aurastudio_core::Rect;

const CH: i32 = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyTiles {
    tile:   u32,
    /// Per chunk of 32 × 32 tiles, one bit per tile.
    chunks: HashMap<(i32, i32), [u64; 16]>,
    count:  usize,
}

impl DirtyTiles {
    /// For tiles of `tile` pixels.
    pub fn new(tile: u32) -> Self { Self { tile: tile.max(1), chunks: HashMap::new(), count: 0 } }

    pub fn tile_size(&self) -> u32 { self.tile }

    pub fn is_empty(&self) -> bool { self.count == 0 }

    /// Tiles in the set.
    pub fn len(&self) -> usize { self.count }

    pub fn clear(&mut self) { self.chunks.clear(); self.count = 0; }

    fn set(&mut self, tx: i32, ty: i32) {
        let k = (tx.div_euclid(CH), ty.div_euclid(CH));
        let i = (ty.rem_euclid(CH) * CH + tx.rem_euclid(CH)) as usize;
        let w = self.chunks.entry(k).or_insert([0; 16]);
        let b = 1u64 << (i % 64);
        if w[i / 64] & b == 0 { w[i / 64] |= b; self.count += 1; }
    }

    fn get(&self, tx: i32, ty: i32) -> bool {
        let k = (tx.div_euclid(CH), ty.div_euclid(CH));
        let i = (ty.rem_euclid(CH) * CH + tx.rem_euclid(CH)) as usize;
        self.chunks.get(&k).is_some_and(|w| w[i / 64] & (1 << (i % 64)) != 0)
    }

    fn span(&self, r: &Rect) -> Option<(i32, i32, i32, i32)> {
        if r.is_empty() { return None; }
        let t = self.tile as i32;
        Some((r.x0.div_euclid(t), r.y0.div_euclid(t), (r.x1 - 1).div_euclid(t) + 1, (r.y1 - 1).div_euclid(t) + 1))
    }

    /// Add the tiles touching `r`.
    pub fn insert(&mut self, r: Rect) {
        let Some((tx0, ty0, tx1, ty1)) = self.span(&r) else { return };
        for ty in ty0..ty1 { for tx in tx0..tx1 { self.set(tx, ty); } }
    }

    /// Add every tile of `o` (same tile size; another is ignored).
    pub fn union(&mut self, o: &DirtyTiles) {
        if o.tile != self.tile { return; }
        for (tx, ty) in o.tiles() { self.set(tx, ty); }
    }

    /// Whether some tile touching `r` is in the set.
    pub fn touches(&self, r: Rect) -> bool {
        let Some((tx0, ty0, tx1, ty1)) = self.span(&r) else { return false };
        if ((tx1 - tx0) as i64) * ((ty1 - ty0) as i64) > self.count as i64 {
            let t = self.tile as i32;
            return self.tiles().any(|(tx, ty)| Rect::from_xywh(tx * t, ty * t, self.tile, self.tile).overlaps(&r));
        }
        (ty0..ty1).any(|ty| (tx0..tx1).any(|tx| self.get(tx, ty)))
    }

    /// The set as rectangles: runs of tiles side by side in each row,
    /// top to bottom.
    pub fn rects(&self) -> Vec<Rect> {
        let mut cells: Vec<(i32, i32)> = self.tiles().map(|(x, y)| (y, x)).collect();
        cells.sort_unstable();
        let t = self.tile as i32;
        let mut out: Vec<Rect> = Vec::new();
        for (ty, tx) in cells {
            let r = Rect::from_xywh(tx * t, ty * t, self.tile, self.tile);
            match out.last_mut() {
                Some(l) if l.y0 == r.y0 && l.x1 == r.x0 => l.x1 = r.x1,
                _ => out.push(r),
            }
        }
        out
    }

    /// `rects` clipped to `clip`.
    pub fn rects_in(&self, clip: Rect) -> Vec<Rect> { self.rects().into_iter().map(|r| r.intersect(&clip)).filter(|r| !r.is_empty()).collect() }

    /// The smallest rectangle around the set.
    pub fn bounds(&self) -> Rect {
        let t = self.tile as i32;
        self.tiles().fold(Rect::EMPTY, |a, (tx, ty)| a.union(&Rect::from_xywh(tx * t, ty * t, self.tile, self.tile)))
    }

    /// (tx, ty) of every tile in the set.
    pub(crate) fn tiles(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.chunks.iter().flat_map(|(k, w)| (0..(CH * CH) as usize).filter(move |i| w[i / 64] & (1 << (i % 64)) != 0).map(move |i| (k.0 * CH + i as i32 % CH, k.1 * CH + i as i32 / CH)))
    }
}

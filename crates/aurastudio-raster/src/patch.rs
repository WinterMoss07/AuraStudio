//! Undo by tile: a snapshot of a surface (free: tiles are shared), the
//! patch of the tiles that changed since, and swapping a patch with the
//! surface (undo, then redo). A patch keeps its tiles as they are (shared)
//! until `pack` compresses them (older history steps), and saves as bytes.

use std::sync::Arc;

use aurastudio_core::{ByteReader, ByteWriter, HeapSize, Pixel, PixelKind, Rect};

use crate::slot::{encode, encoded_max, ColdTile, Info, Slot};
use crate::grid::Grid;
use crate::surface::{pixel_of, Surface};
use crate::DirtyTiles;

/// A surface's tiles at one moment (shared with it until either writes).
#[derive(Clone, Debug)]
pub struct Snapshot<P: Pixel> {
    pub(crate) grid: Grid<P>,
    pub(crate) tile: u32,
    pub(crate) fill: P,
}

impl<P: Pixel> Snapshot<P> {
    pub(crate) fn slot(&self, tx: i32, ty: i32) -> &Slot<P> { self.grid.get(tx, ty) }
}

/// Tiles of one surface to put back: (column, row, tile as it was).
#[derive(Clone, Debug)]
pub struct Patch<P: Pixel> {
    tile:  u32,
    tiles: Vec<(i32, i32, Slot<P>)>,
}

impl<P: Pixel> Default for Patch<P> {
    fn default() -> Self { Self { tile: 0, tiles: Vec::new() } }
}

/// Same pixels: `a` over fill `fa`, `b` over fill `fb` (`n` pixels a
/// tile), without copying them.
fn equal<P: Pixel>(a: &Slot<P>, fa: P, b: &Slot<P>, fb: P, n: usize) -> bool {
    match (a, b) {
        (Slot::Full(x), Slot::Full(y)) => Arc::ptr_eq(x, y) || x.px == y.px,
        (Slot::Alpha(x), Slot::Alpha(y)) if Arc::ptr_eq(x, y) => true,
        (Slot::Cold(x), Slot::Cold(y)) if Arc::ptr_eq(x, y) || x.data == y.data => true,
        (Slot::Cold(_), _) | (_, Slot::Cold(_)) => {
            let (wa, wb) = (a.warm(n), b.warm(n));
            match (wa, wb) { (Some(wa), Some(wb)) => equal(&wa, fa, &wb, fb, n), _ => false }
        }
        _ => match (a.color(fa), b.color(fb)) {
            (Some(x), Some(y)) => x == y,
            _ => (0..n).all(|i| pixel_of(a, fa, i, n) == pixel_of(b, fb, i, n)),
        },
    }
}

impl<P: Pixel> Surface<P> {
    /// The tiles that changed since `before`, as they were then: undoing
    /// the patch puts them back. Empty when nothing changed (or `before`
    /// is of another shape).
    pub fn diff(&self, before: &Snapshot<P>) -> Patch<P> {
        let mut all: Vec<(i32, i32)> = self.grid.iter().map(|(x, y, _)| (x, y)).chain(before.grid.iter().map(|(x, y, _)| (x, y))).collect();
        all.sort_unstable();
        all.dedup();
        self.diff_cells(before, all.into_iter())
    }

    /// `diff` for the tiles of `set` only (what a stroke touched).
    pub fn diff_set(&self, before: &Snapshot<P>, set: &DirtyTiles) -> Patch<P> { self.diff_cells(before, set.tiles()) }

    fn diff_cells(&self, before: &Snapshot<P>, cells: impl Iterator<Item = (i32, i32)>) -> Patch<P> {
        let mut p = Patch { tile: self.tile, tiles: Vec::new() };
        if before.tile != self.tile { return p; }
        let (n, refill) = (self.npx(), before.fill != self.fill);
        for (tx, ty) in cells {
            let (a, b) = (before.slot(tx, ty), self.slot(tx, ty));
            if !refill && a.is_fill() && b.is_fill() { continue; }
            if equal(a, before.fill, b, self.fill, n) { continue; }
            let kept = match a { Slot::Fill if refill => Slot::Uniform(before.fill), other => other.clone() };
            p.tiles.push((tx, ty, kept));
        }
        p
    }
}

impl<P: Pixel> Patch<P> {
    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }

    pub fn len(&self) -> usize { self.tiles.len() }

    /// The tiles as a set (what to merge and show again).
    pub fn tiles(&self) -> DirtyTiles {
        let mut set = DirtyTiles::new(self.tile.max(1));
        let t = self.tile as i32;
        for (tx, ty, _) in &self.tiles { set.insert(Rect::from_xywh(tx * t, ty * t, 1, 1)); }
        set
    }

    /// The area of the tiles, as few rectangles: runs of tiles side by side
    /// in a row.
    pub fn rects(&self) -> Vec<Rect> { self.tiles().rects() }

    /// The smallest rectangle around the tiles.
    /// Side of its tiles.
    pub fn tile_size(&self) -> u32 { self.tile }

    pub fn bounds(&self) -> Rect {
        let t = self.tile as i32;
        self.tiles.iter().fold(Rect::EMPTY, |a, (tx, ty, _)| a.union(&Rect::from_xywh(tx * t, ty * t, self.tile, self.tile)))
    }

    /// Compress the kept pixel and coverage tiles (an older history step:
    /// less memory, a little slower to undo).
    pub fn pack(&mut self) {
        for (_, _, s) in &mut self.tiles {
            if matches!(s, Slot::Full(_) | Slot::Alpha(_)) {
                let data = encode(s);
                *s = Slot::Cold(Arc::new(ColdTile { data, info: Info::default() }));
            }
        }
    }

    /// Put the kept tiles into `s` and keep its tiles instead: done again,
    /// it goes the other way (undo, redo). False, and `s` untouched, if a
    /// the tile size differs or data is damaged.
    pub fn swap(&mut self, s: &mut Surface<P>) -> bool {
        if self.tiles.is_empty() { return true; }
        if self.tile != s.tile { return false; }
        let n = s.npx();
        // Everything checked and opened first: all or nothing.
        let mut back = Vec::with_capacity(self.tiles.len());
        for (_, _, k) in &self.tiles {
            match k.warm(n) { Some(slot) => back.push(slot), None => return false }
        }
        for ((tx, ty, k), slot) in self.tiles.iter_mut().zip(back) {
            *k = s.slot(*tx, *ty).clone();
            let slot = match slot { Slot::Uniform(c) if c == s.fill => Slot::Fill, other => other };
            s.set_slot(*tx, *ty, slot);
            s.settle(*tx, *ty);
        }
        s.drop_table_if_empty();
        true
    }

    /// For a file: magic, version, pixel kind, tile size, then per tile its
    /// column, row and encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_header(&MAGIC, VERSION);
        w.u8(P::KIND.code()).u32(self.tile).varint(self.tiles.len() as u64);
        for (tx, ty, s) in &self.tiles { w.svarint(*tx as i64).svarint(*ty as i64).blob(&encode(s)); }
        w.finish()
    }

    /// Read back what `to_bytes` wrote; `None` if damaged or of another
    /// pixel kind (each tile's data is only opened when swapped).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut r = ByteReader::new(b);
        if r.header(&MAGIC)? != VERSION || PixelKind::from_code(r.u8()?)? != P::KIND { return None; }
        let tile = r.u32()?;
        if !tile.is_power_of_two() || !(crate::MIN_TILE..=crate::MAX_TILE).contains(&tile) { return None; }
        let max = encoded_max::<P>((tile * tile) as usize);
        let count = r.varint()? as usize;
        if count > r.remaining() { return None; }
        let mut tiles = Vec::with_capacity(count);
        for _ in 0..count {
            let (tx, ty) = (i32::try_from(r.svarint()?).ok()?, i32::try_from(r.svarint()?).ok()?);
            let data = r.blob(max)?;
            tiles.push((tx, ty, Slot::Cold(Arc::new(ColdTile { data: data.to_vec(), info: Info::default() }))));
        }
        r.at_end().then_some(Self { tile, tiles })
    }

    /// The bytes start like a patch of this format.
    pub fn is_patch(b: &[u8]) -> bool { b.starts_with(&MAGIC) }

    /// Open every kept tile (checks a patch read from a file at once).
    pub fn check(&self) -> bool { let n = (self.tile * self.tile) as usize; self.tiles.iter().all(|(_, _, s)| s.warm(n).is_some()) }
}

impl<P: Pixel> HeapSize for Patch<P> {
    /// Kept bytes (a kept tile shared with the surface counts fully).
    fn heap_bytes(&self) -> usize {
        let px = (self.tile * self.tile) as usize * P::BYTES;
        self.tiles.iter().map(|(_, _, s)| 16 + match s {
            Slot::Full(_) => px,
            Slot::Alpha(a) => a.a.capacity(),
            Slot::Cold(c) => c.data.capacity(),
            Slot::Fill | Slot::Uniform(_) => 0,
        }).sum()
    }
}

/// "AURP": an AuraStudio patch (0.19's started with four zero bytes and
/// "TILE": `crate::legacy` reads those).
const MAGIC: [u8; 4] = *b"AURP";
const VERSION: u16 = 1;

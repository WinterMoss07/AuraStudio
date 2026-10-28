//! Some tiles of a layer as they were, each compressed on its own: what
//! an undo step puts back. A stroke keeps only the tiles it touched (a
//! diagonal stroke's bounding box is mostly untouched pixels).

use aurastudio_core::{Rect, TILE};

use crate::{new_tile, Tile, Tiles, TILE_BYTES};

/// Tiles of one layer to put back: (column, row, LZ4 of the tile's bytes;
/// empty: no tile, the layer's fill color).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TilePatch {
    tiles: Vec<(u32, u32, Vec<u8>)>,
}

/// First bytes of a patch written to a file. An older version reads the
/// step as LZ4 pixels of a rectangle: the size these give (0) fits no
/// rectangle, so it drops the step instead of writing garbage.
const MAGIC: [u8; 8] = [0, 0, 0, 0, b'T', b'I', b'L', b'E'];

fn compress(tile: &[u8; TILE_BYTES], fill: [u8; 4]) -> Vec<u8> {
    if tile.chunks_exact(4).all(|p| p == fill) { return Vec::new(); }
    lz4_flex::compress(&tile[..])
}

fn decompress(data: &[u8], fill: [u8; 4]) -> Option<Tile> {
    if data.is_empty() { return Some(new_tile(fill)); }
    let px = lz4_flex::decompress(data, TILE_BYTES).ok()?;
    let tile: Tile = px.into_boxed_slice().try_into().ok()?;
    Some(tile)
}

impl TilePatch {
    /// Keep tile (tx, ty) as `tile` shows it.
    pub fn push(&mut self, tx: u32, ty: u32, tile: &[u8; TILE_BYTES], fill: [u8; 4]) {
        self.tiles.push((tx, ty, compress(tile, fill)));
    }

    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }

    pub fn len(&self) -> usize { self.tiles.len() }

    /// Compressed bytes held.
    pub fn bytes(&self) -> usize {
        self.tiles.iter().map(|t| t.2.len() + 12).sum()
    }

    /// The canvas area of the tiles, as few rectangles: runs of tiles side
    /// by side in a row (clipped to `width` × `height`).
    pub fn rects(&self, width: u32, height: u32) -> Vec<Rect> {
        let mut cells: Vec<(u32, u32)> = self.tiles.iter().map(|t| (t.1, t.0)).collect();
        cells.sort_unstable();
        cells.dedup();
        let mut out: Vec<Rect> = Vec::new();
        for (ty, tx) in cells {
            let r = (tx * TILE, ty * TILE, ((tx + 1) * TILE).min(width), ((ty + 1) * TILE).min(height));
            if r.0 >= r.2 || r.1 >= r.3 { continue; }
            match out.last_mut() {
                Some(l) if l.1 == r.1 && l.2 == r.0 => l.2 = r.2,
                _ => out.push(r),
            }
        }
        out
    }

    /// The smallest rectangle around the tiles.
    pub fn bbox(&self, width: u32, height: u32) -> Rect {
        self.rects(width, height).into_iter().reduce(aurastudio_core::union).unwrap_or((0, 0, 0, 0))
    }

    /// Put the kept tiles in `tiles` and keep theirs instead: done again,
    /// it goes the other way (undo, redo). False, and `tiles` untouched, if
    /// a tile is off the layer or its data is damaged.
    pub fn swap(&mut self, tiles: &mut Tiles) -> bool {
        let fill = tiles.fill;
        // Everything checked and unpacked first: all or nothing.
        let mut back = Vec::with_capacity(self.tiles.len());
        for (tx, ty, data) in &self.tiles {
            if *tx >= tiles.cols || *ty >= tiles.rows { return false; }
            let Some(tile) = decompress(data, fill) else { return false };
            back.push((data.is_empty(), tile));
        }
        for ((tx, ty, data), (absent, tile)) in self.tiles.iter_mut().zip(back) {
            *data = match tiles.tile(*tx, *ty) {
                Some(t) => compress(t, fill),
                None => Vec::new(),
            };
            tiles.set_tile(*tx, *ty, (!absent).then_some(tile));
        }
        true
    }

    /// For a file: the magic, then per tile its column, row and data
    /// length (u32 little endian) and the data.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAGIC.len() + self.bytes());
        out.extend_from_slice(&MAGIC);
        for (tx, ty, data) in &self.tiles {
            out.extend_from_slice(&tx.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out
    }

    /// Read back what to_bytes wrote. None if it is damaged (each tile's
    /// data is only checked when the step is undone).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut rest = b.strip_prefix(&MAGIC[..])?;
        let mut tiles = Vec::new();
        let u32_at = |s: &[u8], i: usize| -> Option<u32> { Some(u32::from_le_bytes(s.get(i..i + 4)?.try_into().ok()?)) };
        while !rest.is_empty() {
            let (tx, ty, n) = (u32_at(rest, 0)?, u32_at(rest, 4)?, u32_at(rest, 8)? as usize);
            // (A tile compresses to less than its size plus LZ4's margin.)
            if n > TILE_BYTES + TILE_BYTES / 255 + 16 { return None; }
            let data = rest.get(12..12 + n)?;
            tiles.push((tx, ty, data.to_vec()));
            rest = &rest[12 + n..];
        }
        Some(Self { tiles })
    }

    /// The bytes start like a patch (vs. an older pixel step).
    pub fn is_patch(b: &[u8]) -> bool { b.starts_with(&MAGIC) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles_with(cells: &[(u32, u32, u8)]) -> Tiles {
        let mut t = Tiles::new(200, 130, [0; 4]);
        for &(tx, ty, v) in cells {
            let mut tile = new_tile([0; 4]);
            for (i, b) in tile.iter_mut().enumerate() { *b = (i as u8).wrapping_mul(v); }
            t.set_tile(tx, ty, Some(tile));
        }
        t
    }

    fn same(a: &Tiles, b: &Tiles) -> bool {
        (0..a.rows).all(|ty| (0..a.cols).all(|tx| a.tile(tx, ty).map(|t| &t[..]) == b.tile(tx, ty).map(|t| &t[..])))
    }

    #[test]
    fn undo_and_redo_swap_the_tiles_back_and_forth() {
        let before = tiles_with(&[(0, 0, 3), (2, 1, 5)]);
        // Kept: tile (0, 0) as it was, (1, 1) absent (fill) before.
        let mut p = TilePatch::default();
        p.push(0, 0, before.tile(0, 0).unwrap(), [0; 4]);
        p.push(1, 1, &new_tile([0; 4]), [0; 4]);
        let after = tiles_with(&[(0, 0, 7), (1, 1, 9), (2, 1, 5)]);
        let mut t = after.clone();
        assert!(p.swap(&mut t), "undo");
        assert!(same(&t, &before), "the tiles as they were, (1, 1) gone again");
        assert!(t.tile(1, 1).is_none());
        assert!(p.swap(&mut t), "redo");
        assert!(same(&t, &after));
    }

    #[test]
    fn rects_join_tiles_side_by_side() {
        let mut p = TilePatch::default();
        for (tx, ty) in [(0, 0), (1, 0), (3, 0), (2, 2), (3, 2)] { p.push(tx, ty, &new_tile([0; 4]), [0; 4]); }
        assert_eq!(p.rects(200, 150), vec![(0, 0, 128, 64), (192, 0, 200, 64), (128, 128, 200, 150)]);
        assert_eq!(p.bbox(200, 150), (0, 0, 200, 150));
    }

    #[test]
    fn bytes_round_trip_and_damage_is_refused() {
        let t = tiles_with(&[(1, 0, 3)]);
        let mut p = TilePatch::default();
        p.push(1, 0, t.tile(1, 0).unwrap(), [0; 4]);
        p.push(0, 1, &new_tile([0; 4]), [0; 4]);
        let b = p.to_bytes();
        assert!(TilePatch::is_patch(&b));
        assert_eq!(TilePatch::from_bytes(&b), Some(p.clone()));
        assert_eq!(TilePatch::from_bytes(&b[..b.len() - 1]), None, "cut");
        assert_eq!(TilePatch::from_bytes(&b[1..]), None, "no magic");
        // Damaged tile data: refused when swapped, nothing changed.
        let mut bad = p.clone();
        bad.tiles[0].2[0] ^= 0xff;
        bad.tiles[0].2.truncate(3);
        let mut target = tiles_with(&[(1, 0, 9)]);
        let copy = target.clone();
        assert!(!bad.swap(&mut target));
        assert!(same(&target, &copy));
        // Off the layer.
        let mut off = TilePatch::default();
        off.push(9, 9, &new_tile([0; 4]), [0; 4]);
        assert!(!off.swap(&mut target));
    }

    /// An older version reads the step as an LZ4 rectangle: the size it
    /// finds is 0 (fits no rectangle).
    #[test]
    fn older_versions_read_no_pixels() {
        let b = TilePatch::default().to_bytes();
        assert_eq!(u32::from_le_bytes(b[..4].try_into().unwrap()), 0);
    }
}

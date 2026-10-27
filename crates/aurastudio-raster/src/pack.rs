//! A layer's stored tiles as bytes, for the .aurs file: their count
//! (u32), then for each its column and row (u32 each) and its pixels, LZ4
//! with their size first. The storage of a tile is this crate's business:
//! the file only keeps the bytes.

use crate::{Tiles, TILE, TILE_BYTES};

impl Tiles {
    /// The stored tiles as bytes (see the module).
    pub fn pack(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.stored() as u32).to_le_bytes());
        for (tx, ty, t) in self.iter() {
            out.extend_from_slice(&tx.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            let packed = lz4_flex::compress_prepend_size(&t[..]);
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&packed);
        }
        out
    }

    /// Most bytes `pack` can give for a `width` × `height` layer (to
    /// refuse a damaged file before reading it).
    pub fn packed_max(width: u32, height: u32) -> u64 {
        (width.div_ceil(TILE) * height.div_ceil(TILE)) as u64 * (TILE_BYTES as u64 + 64) + 4
    }

    /// Read `pack` data into these tiles (None if damaged).
    pub fn unpack_into(&mut self, blob: &[u8]) -> Option<()> {
        let mut at = 0usize;
        let u32_at = |at: &mut usize| -> Option<u32> {
            let v = u32::from_le_bytes(blob.get(*at..*at + 4)?.try_into().ok()?);
            *at += 4;
            Some(v)
        };
        let (cols, rows) = (self.width().div_ceil(TILE), self.height().div_ceil(TILE));
        let count = u32_at(&mut at)?;
        if count > cols * rows { return None; }
        for _ in 0..count {
            let (tx, ty, n) = (u32_at(&mut at)?, u32_at(&mut at)?, u32_at(&mut at)? as usize);
            if tx >= cols || ty >= rows { return None; }
            let data = lz4(blob.get(at..at.checked_add(n)?)?, TILE_BYTES)?;
            at += n;
            self.set_tile(tx, ty, Some(data.into_boxed_slice().try_into().ok()?));
        }
        (at == blob.len()).then_some(())
    }
}

/// LZ4 data with its size first, expected to unpack to exactly `len` bytes.
fn lz4(blob: &[u8], len: usize) -> Option<Vec<u8>> {
    let size = u32::from_le_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    if size != len { return None; }
    let mut out = vec![0u8; len];
    let n = lz4_flex::block::decompress_into(&blob[4..], &mut out).ok()?;
    (n == len).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_tiles_come_back_the_same_and_damage_is_refused() {
        let mut t = Tiles::new(200, 130, [0; 4]);
        t.write_rect((10, 20, 150, 90), &(0..140 * 70 * 4).map(|i| (i * 7 % 251) as u8).collect::<Vec<_>>());
        let blob = t.pack();
        let mut back = Tiles::new(200, 130, [0; 4]);
        back.unpack_into(&blob).unwrap();
        assert!(back == t);
        assert!(blob.len() as u64 <= Tiles::packed_max(200, 130));
        let mut bad = Tiles::new(200, 130, [0; 4]);
        assert!(bad.unpack_into(&blob[..blob.len() - 3]).is_none());
        let mut small = Tiles::new(64, 64, [0; 4]);
        assert!(small.unpack_into(&blob).is_none(), "tiles outside the layer");
    }
}

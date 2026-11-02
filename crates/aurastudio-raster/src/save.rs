//! A surface as bytes, in this crate's own versioned format: `file` keeps
//! the bytes and never reads inside them. Saving again reuses the bytes of
//! every tile whose content did not change (by fingerprint), and encodes
//! the others on all cores. Also the reader of the 0.19 layout (64 px
//! RGBA8 tiles) for older documents and undo steps.

use std::collections::HashMap;
use std::sync::Arc;

use rayon::prelude::*;

use aurastudio_core::{ByteReader, ByteWriter, Pixel, PixelKind, Rgba8};

use crate::slot::{decode, encode, encoded_max, FullTile, Slot};
use crate::surface::Surface;

/// "AURT" + version, pixel kind, tile size, width, height, fill, count,
/// then per stored tile: column, row, encoding.
const MAGIC: [u8; 4] = *b"AURT";
const VERSION: u16 = 1;

/// The encodings of the last save, by tile fingerprint: the next save
/// copies them instead of compressing again (an incremental save).
#[derive(Default)]
pub struct SaveCache {
    tiles: HashMap<u64, Arc<Vec<u8>>>,
}

impl SaveCache {
    pub fn len(&self) -> usize { self.tiles.len() }
    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }
}

/// What a save did (diagnostics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveStats {
    pub tiles:   usize,
    pub reused:  usize,
    pub encoded: usize,
}

impl<P: Pixel> Surface<P> {
    pub fn save(&self) -> Vec<u8> { self.save_with(&mut SaveCache::default()).0 }

    /// Save, reusing `cache`'s encodings of unchanged tiles; `cache` then
    /// holds this save's encodings only.
    pub fn save_with(&self, cache: &mut SaveCache) -> (Vec<u8>, SaveStats) {
        aurastudio_core::zone!("raster save");
        let stored: Vec<(u32, u32, &Slot<P>)> = self.slots.iter().enumerate().filter(|(_, s)| !s.is_fill()).map(|(i, s)| (i as u32 % self.cols, i as u32 / self.cols, s)).collect();
        let parts: Vec<(u64, Arc<Vec<u8>>, bool)> = stored.par_iter().map(|&(tx, ty, s)| {
            let fp = self.info(tx, ty).fp;
            match cache.tiles.get(&fp) {
                Some(b) => (fp, Arc::clone(b), true),
                None => (fp, Arc::new(encode(s)), false),
            }
        }).collect();
        let mut w = ByteWriter::with_header(&MAGIC, VERSION);
        w.u8(P::KIND.code()).u32(self.tile).u32(self.width).u32(self.height);
        let mut fill = Vec::new();
        self.fill.write_le(&mut fill);
        w.bytes(&fill).varint(stored.len() as u64);
        let mut stats = SaveStats { tiles: stored.len(), ..Default::default() };
        let mut next = HashMap::with_capacity(parts.len());
        for ((tx, ty, _), (fp, bytes, reused)) in stored.iter().zip(parts) {
            w.varint(*tx as u64).varint(*ty as u64).blob(&bytes);
            if reused { stats.reused += 1; } else { stats.encoded += 1; }
            next.insert(fp, bytes);
        }
        cache.tiles = next;
        aurastudio_core::count!("tiles saved again from cache", stats.reused);
        (w.finish(), stats)
    }

    /// Read what `save` wrote; `None` if damaged, of another pixel kind,
    /// or larger than `max_side` on a side. Tiles are opened on all cores.
    pub fn load(b: &[u8], max_side: u32) -> Option<Self> {
        aurastudio_core::zone!("raster load");
        let mut r = ByteReader::new(b);
        if r.header(&MAGIC)? != VERSION || PixelKind::from_code(r.u8()?)? != P::KIND { return None; }
        let (tile, width, height) = (r.u32()?, r.u32()?, r.u32()?);
        if !tile.is_power_of_two() || !(crate::MIN_TILE..=crate::MAX_TILE).contains(&tile) || width > max_side || height > max_side { return None; }
        let fill = P::read_le(r.bytes(P::BYTES)?)?;
        let mut s = Surface::new(width, height, tile, fill);
        let count = r.varint()? as usize;
        if count > (s.cols * s.rows) as usize { return None; }
        let (n, max) = (s.npx(), encoded_max::<P>(s.npx()));
        let mut raw = Vec::with_capacity(count);
        for _ in 0..count {
            let (tx, ty) = (u32::try_from(r.varint()?).ok()?, u32::try_from(r.varint()?).ok()?);
            if tx >= s.cols || ty >= s.rows { return None; }
            raw.push((tx, ty, r.blob(max)?));
        }
        if !r.at_end() { return None; }
        // Opened and settled on all cores, then put in place.
        let shape = &s;
        let opened: Option<Vec<(u32, u32, Slot<P>)>> = raw.into_par_iter().map(|(tx, ty, d)| Some((tx, ty, shape.settled_slot(decode::<P>(d, n)?, tx, ty)))).collect();
        for (tx, ty, slot) in opened? { s.set_slot(tx, ty, slot); }
        s.drop_table_if_empty();
        Some(s)
    }

    /// Most bytes `save` can give for this shape (to refuse a damaged file
    /// before reading it).
    pub fn saved_max(width: u32, height: u32, tile: u32) -> u64 {
        let t = tile.clamp(crate::MIN_TILE, crate::MAX_TILE) as u64;
        let tiles = (width as u64).div_ceil(t) * (height as u64).div_ceil(t);
        tiles * (encoded_max::<P>((t * t) as usize) as u64 + 24) + 64
    }
}

/// 0.19's layer layout: tile count, then per tile column, row, length and
/// LZ4 (size first) of 64 × 64 RGBA8 pixels.
pub mod legacy {
    use super::*;

    pub const TILE: u32 = 64;
    const BYTES: usize = (TILE * TILE * 4) as usize;

    fn u32_at(b: &[u8], i: usize) -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) }

    /// A 0.19 layer of `width` × `height` with fill `fill`, re-cut into
    /// tiles of `tile`. `None` if damaged.
    pub fn load_layer(blob: &[u8], width: u32, height: u32, fill: Rgba8, tile: u32) -> Option<Surface<Rgba8>> {
        let mut old = Surface::new(width, height, TILE, fill);
        let count = u32_at(blob, 0)?;
        if count > old.cols * old.rows { return None; }
        let mut at = 4usize;
        for _ in 0..count {
            let (tx, ty, n) = (u32_at(blob, at)?, u32_at(blob, at + 4)?, u32_at(blob, at + 8)? as usize);
            at += 12;
            if tx >= old.cols || ty >= old.rows { return None; }
            let data = blob.get(at..at.checked_add(n)?)?;
            at += n;
            if u32_at(data, 0)? as usize != BYTES { return None; }
            let mut raw = vec![0u8; BYTES];
            if lz4_flex::block::decompress_into(data.get(4..)?, &mut raw).ok()? != BYTES { return None; }
            let px: Vec<Rgba8> = raw.as_chunks::<4>().0.to_vec();
            old.set_slot(tx, ty, Slot::Full(Arc::new(FullTile { px, info: None })));
        }
        if at != blob.len() { return None; }
        old.compact(old.rect());
        Some(if tile == TILE { old } else { old.retiled(tile) })
    }

    /// A 0.19 stroke undo step: four zero bytes, "TILE", then per tile
    /// column, row, length and LZ4 of 64 × 64 RGBA8 pixels (empty: the
    /// fill).
    pub fn is_old_patch(b: &[u8]) -> bool { b.starts_with(&[0, 0, 0, 0, b'T', b'I', b'L', b'E']) }

    /// The tiles of an old undo step as (rect, pixels or `None` for the
    /// fill), canvas coordinates of 64 px tiles: the document replays them
    /// as pixel writes into a surface of any tile size.
    #[allow(clippy::type_complexity)]
    pub fn read_old_patch(b: &[u8]) -> Option<Vec<(aurastudio_core::Rect, Option<Vec<Rgba8>>)>> {
        let mut rest = b.get(8..)?;
        let mut out = Vec::new();
        while !rest.is_empty() {
            let (tx, ty, n) = (u32_at(rest, 0)?, u32_at(rest, 4)?, u32_at(rest, 8)? as usize);
            if n > BYTES + BYTES / 255 + 16 { return None; }
            let data = rest.get(12..12 + n)?;
            let r = aurastudio_core::Rect::from_xywh((tx * TILE) as i32, (ty * TILE) as i32, TILE, TILE);
            let px = if data.is_empty() { None } else {
                let raw = lz4_flex::decompress(data, BYTES).ok()?;
                if raw.len() != BYTES { return None; }
                Some(raw.as_chunks::<4>().0.to_vec())
            };
            out.push((r, px));
            rest = &rest[12 + n..];
        }
        Some(out)
    }
}

//! A surface as bytes, in this crate's own versioned format: `file` keeps
//! the bytes and never reads inside them. Also the reader of the 0.19
//! layout (64 px RGBA8 tiles) for older documents and undo steps.

use std::sync::Arc;

use aurastudio_core::{Pixel, PixelKind, Rgba8};

use crate::patch::{slot_bytes, unpack_slot};
use crate::surface::{Slot, Surface};

/// "AURS" + version, pixel kind, tile size, width, height, fill, count,
/// then per stored tile: column, row, length, data (as in a patch: one
/// pixel for a uniform tile, LZ4 of the pixels for a pixel tile).
const MAGIC: [u8; 4] = *b"AURT";
const VERSION: u8 = 1;

fn u32_at(b: &[u8], i: usize) -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) }

impl<P: Pixel> Surface<P> {
    pub fn save(&self) -> Vec<u8> {
        aurastudio_core::zone!("raster save");
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(P::KIND.code());
        for v in [self.tile, self.width, self.height] { out.extend_from_slice(&v.to_le_bytes()); }
        self.fill.write_le(&mut out);
        let stored: Vec<(usize, &Slot<P>)> = self.slots.iter().enumerate().filter(|(_, s)| !matches!(s, Slot::Fill)).collect();
        out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        for (i, s) in stored {
            let data = slot_bytes(s).unwrap_or_default();
            out.extend_from_slice(&(i as u32 % self.cols).to_le_bytes());
            out.extend_from_slice(&(i as u32 / self.cols).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
        out
    }

    /// Read what `save` wrote; `None` if damaged, of another pixel kind,
    /// or larger than `max_side` on a side.
    pub fn load(b: &[u8], max_side: u32) -> Option<Self> {
        aurastudio_core::zone!("raster load");
        let rest = b.strip_prefix(&MAGIC[..])?;
        if *rest.first()? != VERSION || PixelKind::from_code(*rest.get(1)?)? != P::KIND { return None; }
        let rest = &rest[2..];
        let (tile, width, height) = (u32_at(rest, 0)?, u32_at(rest, 4)?, u32_at(rest, 8)?);
        if !tile.is_power_of_two() || !(crate::MIN_TILE..=crate::MAX_TILE).contains(&tile) || width > max_side || height > max_side { return None; }
        let fill = P::read_le(rest.get(12..)?)?;
        let mut at = 12 + P::BYTES;
        let count = u32_at(rest, at)?;
        at += 4;
        let mut s = Surface::new(width, height, tile, fill);
        if count > s.cols * s.rows { return None; }
        let n = (tile * tile) as usize;
        for _ in 0..count {
            let (tx, ty, len) = (u32_at(rest, at)?, u32_at(rest, at + 4)?, u32_at(rest, at + 8)? as usize);
            at += 12;
            if tx >= s.cols || ty >= s.rows || len > n * P::BYTES + n * P::BYTES / 255 + 16 { return None; }
            let slot = unpack_slot::<P>(rest.get(at..at.checked_add(len)?)?, n)?;
            at += len;
            let slot = match slot { Slot::Uniform(c) if c == fill => Slot::Fill, other => other };
            s.set_slot(tx, ty, slot);
        }
        (at == rest.len()).then_some(s)
    }

    /// Most bytes `save` can give for this shape (to refuse a damaged file
    /// before reading it).
    pub fn saved_max(width: u32, height: u32, tile: u32) -> u64 {
        let t = tile.clamp(crate::MIN_TILE, crate::MAX_TILE) as u64;
        let tiles = (width as u64).div_ceil(t) * (height as u64).div_ceil(t);
        tiles * (t * t * P::BYTES as u64 * 257 / 255 + 32) + 64
    }
}

/// 0.19's layer layout: tile count, then per tile column, row, length and
/// LZ4 (size first) of 64 × 64 RGBA8 pixels.
pub mod legacy {
    use super::*;

    pub const TILE: u32 = 64;
    const BYTES: usize = (TILE * TILE * 4) as usize;

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
            let size = u32_at(data, 0)? as usize;
            if size != BYTES { return None; }
            let mut raw = vec![0u8; BYTES];
            if lz4_flex::block::decompress_into(data.get(4..)?, &mut raw).ok()? != BYTES { return None; }
            let px: Vec<Rgba8> = raw.as_chunks::<4>().0.to_vec();
            old.set_slot(tx, ty, Slot::Full(Arc::new(px)));
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

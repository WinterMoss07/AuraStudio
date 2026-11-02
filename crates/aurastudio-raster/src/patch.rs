//! Undo by tile: a snapshot of a surface (free: tiles are shared), the
//! patch of the tiles that changed since, and swapping a patch with the
//! surface (undo, then redo). A patch keeps its tiles as they are until
//! `pack` compresses them (old history steps), and is saved as bytes.

use std::sync::Arc;

use aurastudio_core::{HeapSize, Pixel, PixelKind, Rect};

use crate::surface::{Slot, Surface};

/// A surface's tiles at one moment (shared with it until either writes).
#[derive(Clone, Debug)]
pub struct Snapshot<P: Pixel> {
    pub(crate) slots: Vec<Slot<P>>,
    pub(crate) cols:  u32,
    pub(crate) rows:  u32,
    pub(crate) tile:  u32,
    pub(crate) fill:  P,
}

impl<P: Pixel> Snapshot<P> {
    fn slot(&self, i: usize) -> &Slot<P> {
        match self.slots.get(i) { Some(s) => s, None => &Slot::Fill }
    }
}

#[derive(Clone, Debug)]
enum Kept<P: Pixel> {
    /// The tile as it was (shared).
    Slot(Slot<P>),
    /// LZ4 of its pixels (`pack`), or empty: the fill.
    Packed(Vec<u8>),
}

/// Tiles of one surface to put back: (column, row, tile).
#[derive(Clone, Debug)]
pub struct Patch<P: Pixel> {
    tile:  u32,
    tiles: Vec<(u32, u32, Kept<P>)>,
}

impl<P: Pixel> Default for Patch<P> {
    fn default() -> Self { Self { tile: 0, tiles: Vec::new() } }
}

impl<P: Pixel> Surface<P> {
    /// The tiles that changed since `before`, as they were then: undoing
    /// the patch puts them back. Empty when nothing changed (or `before`
    /// is of another shape).
    pub fn diff(&self, before: &Snapshot<P>) -> Patch<P> { self.diff_in(before, self.rect()) }

    /// `diff` for the tiles touching `area` only (what a stroke touched:
    /// the rest of the surface is not visited).
    pub fn diff_in(&self, before: &Snapshot<P>, area: Rect) -> Patch<P> {
        let mut p = Patch { tile: self.tile, tiles: Vec::new() };
        if (before.cols, before.rows, before.tile) != (self.cols, self.rows, self.tile) { return p; }
        if self.slots.is_empty() && before.slots.is_empty() { return p; }
        let Some((tx0, ty0, tx1, ty1)) = self.tiles_of(&area) else { return p };
        let now = |i: usize| match self.slots.get(i) { Some(s) => s, None => &Slot::Fill };
        let refill = before.fill != self.fill;
        for i in (ty0..ty1).flat_map(|ty| (tx0..tx1).map(move |tx| (ty * self.cols + tx) as usize)) {
            let (a, b) = (before.slot(i), now(i));
            if !refill && matches!((a, b), (Slot::Fill, Slot::Fill)) { continue; }
            if equal(a, before.fill, b, self.fill) { continue; }
            let kept = match a { Slot::Fill if refill => Slot::Uniform(before.fill), other => other.clone() };
            p.tiles.push((i as u32 % self.cols, i as u32 / self.cols, Kept::Slot(kept)));
        }
        p
    }
}

impl<P: Pixel> Patch<P> {
    pub fn is_empty(&self) -> bool { self.tiles.is_empty() }

    pub fn len(&self) -> usize { self.tiles.len() }

    /// The area of the tiles, as few rectangles: runs of tiles side by side
    /// in a row, clipped to `width` × `height`.
    pub fn rects(&self, width: u32, height: u32) -> Vec<Rect> {
        let t = self.tile as i32;
        let mut cells: Vec<(u32, u32)> = self.tiles.iter().map(|k| (k.1, k.0)).collect();
        cells.sort_unstable();
        cells.dedup();
        let full = Rect::of_size(width, height);
        let mut out: Vec<Rect> = Vec::new();
        for (ty, tx) in cells {
            let r = Rect::from_xywh(tx as i32 * t, ty as i32 * t, self.tile, self.tile).intersect(&full);
            if r.is_empty() { continue; }
            match out.last_mut() {
                Some(l) if l.y0 == r.y0 && l.x1 == r.x0 => l.x1 = r.x1,
                _ => out.push(r),
            }
        }
        out
    }

    /// The smallest rectangle around the tiles.
    pub fn bounds(&self, width: u32, height: u32) -> Rect {
        self.rects(width, height).iter().fold(Rect::EMPTY, |a, r| a.union(r))
    }

    /// Compress the kept pixel tiles (an older history step: less memory,
    /// a little slower to undo).
    pub fn pack(&mut self) {
        for (_, _, k) in &mut self.tiles {
            if let Kept::Slot(s) = k {
                if let Some(bytes) = slot_bytes(s) { *k = Kept::Packed(bytes); }
            }
        }
    }

    /// Put the kept tiles into `s` and keep its tiles instead: done again,
    /// it goes the other way (undo, redo). False, and `s` untouched, if a
    /// tile is off the surface, the tile size differs or data is damaged.
    pub fn swap(&mut self, s: &mut Surface<P>) -> bool {
        if self.tiles.is_empty() { return true; }
        if self.tile != s.tile { return false; }
        // Everything checked and unpacked first: all or nothing.
        let n = (s.tile * s.tile) as usize;
        let mut back = Vec::with_capacity(self.tiles.len());
        for (tx, ty, k) in &self.tiles {
            if *tx >= s.cols || *ty >= s.rows { return false; }
            let slot = match k {
                Kept::Slot(sl) => sl.clone(),
                Kept::Packed(b) => match unpack_slot::<P>(b, n) { Some(sl) => sl, None => return false },
            };
            back.push(slot);
        }
        for ((tx, ty, k), slot) in self.tiles.iter_mut().zip(back) {
            *k = Kept::Slot(s.slot(*tx, *ty).clone());
            let slot = match slot { Slot::Uniform(c) if c == s.fill => Slot::Fill, other => other };
            s.set_slot(*tx, *ty, slot);
        }
        s.drop_table_if_empty();
        true
    }

    /// For a file: magic, version, pixel kind, tile size, then per tile its
    /// column, row, data length (u32 LE) and data (packed form: LZ4 of the
    /// pixels, one pixel for a uniform tile, nothing for the fill).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(P::KIND.code());
        out.extend_from_slice(&self.tile.to_le_bytes());
        for (tx, ty, k) in &self.tiles {
            let data = match k { Kept::Slot(s) => slot_bytes(s).unwrap_or_default(), Kept::Packed(b) => b.clone() };
            out.extend_from_slice(&tx.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&data);
        }
        out
    }

    /// Read back what `to_bytes` wrote; `None` if damaged or of another
    /// pixel kind (each tile's data is only checked when swapped).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let rest = b.strip_prefix(&MAGIC[..])?;
        let (&version, rest) = rest.split_first()?;
        let (&kind, rest) = rest.split_first()?;
        if version != VERSION || PixelKind::from_code(kind)? != P::KIND { return None; }
        let tile = u32_at(rest, 0)?;
        if !tile.is_power_of_two() || !(crate::MIN_TILE..=crate::MAX_TILE).contains(&tile) { return None; }
        let max = (tile * tile) as usize * P::BYTES;
        let mut rest = &rest[4..];
        let mut tiles = Vec::new();
        while !rest.is_empty() {
            let (tx, ty, n) = (u32_at(rest, 0)?, u32_at(rest, 4)?, u32_at(rest, 8)? as usize);
            // (A tile compresses to less than its size plus LZ4's margin.)
            if n > max + max / 255 + 16 { return None; }
            tiles.push((tx, ty, Kept::Packed(rest.get(12..12 + n)?.to_vec())));
            rest = &rest[12 + n..];
        }
        Some(Self { tile, tiles })
    }

    /// The bytes start like a patch of this format.
    pub fn is_patch(b: &[u8]) -> bool { b.starts_with(&MAGIC) }
}

impl<P: Pixel> HeapSize for Patch<P> {
    /// Kept bytes (a kept tile shared with the surface counts fully).
    fn heap_bytes(&self) -> usize {
        let px = (self.tile * self.tile) as usize * P::BYTES;
        self.tiles.iter().map(|(_, _, k)| 12 + match k { Kept::Slot(Slot::Full(_)) => px, Kept::Slot(_) => 0, Kept::Packed(b) => b.len() }).sum()
    }
}

/// Same pixels: `a` over fill `fa`, `b` over fill `fb`.
fn equal<P: Pixel>(a: &Slot<P>, fa: P, b: &Slot<P>, fb: P) -> bool {
    match (a, b) {
        (Slot::Full(x), Slot::Full(y)) => Arc::ptr_eq(x, y) || x == y,
        (Slot::Full(x), o) => { let c = o.color(fb); x.iter().all(|p| *p == c) }
        (o, Slot::Full(y)) => { let c = o.color(fa); y.iter().all(|p| *p == c) }
        _ => a.color(fa) == b.color(fb),
    }
}

/// "AURP": an AuraStudio patch (0.19's started with four zero bytes and
/// "TILE": `crate::legacy` reads those).
const MAGIC: [u8; 4] = *b"AURP";
const VERSION: u8 = 1;

fn u32_at(s: &[u8], i: usize) -> Option<u32> { Some(u32::from_le_bytes(s.get(i..i + 4)?.try_into().ok()?)) }

/// The packed form of a slot: empty = fill, one pixel = uniform, LZ4 of
/// the pixels otherwise. `None` for the fill (nothing to keep but itself).
pub(crate) fn slot_bytes<P: Pixel>(s: &Slot<P>) -> Option<Vec<u8>> {
    match s {
        Slot::Fill => Some(Vec::new()),
        Slot::Uniform(c) => { let mut v = Vec::with_capacity(P::BYTES); c.write_le(&mut v); Some(v) }
        Slot::Full(px) => {
            let mut raw = Vec::with_capacity(px.len() * P::BYTES);
            for p in px.iter() { p.write_le(&mut raw); }
            Some(lz4_flex::compress(&raw))
        }
    }
}

pub(crate) fn unpack_slot<P: Pixel>(b: &[u8], n: usize) -> Option<Slot<P>> {
    if b.is_empty() { return Some(Slot::Fill); }
    if b.len() == P::BYTES { return P::read_le(b).map(Slot::Uniform); }
    let raw = lz4_flex::decompress(b, n * P::BYTES).ok()?;
    if raw.len() != n * P::BYTES { return None; }
    let px: Option<Vec<P>> = raw.chunks_exact(P::BYTES).map(P::read_le).collect();
    Some(Slot::Full(Arc::new(px?)))
}

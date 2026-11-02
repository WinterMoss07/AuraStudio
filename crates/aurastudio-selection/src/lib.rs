//! What is selected: 8-bit coverage on an unbounded mask (`raster`'s
//! `Surface<u8>`: one-value tiles cost nothing), the ways to make it
//! (shapes, a predicate, a layer's alpha), to change it (combine, invert,
//! move, grow, shrink, feather, border, antialias) and its outline.
//!
//! Not here: applying it (a stroke's limit is `raster`'s
//! `begin_stroke(limit)` with `as_mask()`, a filter takes it as is), where
//! it lives (`document`), drawing the marching ants (`ui`).

mod edit;
mod outline;
mod shapes;

use std::sync::atomic::{AtomicU64, Ordering};

use aurastudio_core::{ByteReader, ByteWriter, HeapSize, Point, Rect};
use aurastudio_raster::{Combine, Mask, Surface};

pub use outline::Segment;

pub use self::Mode as SelectMode;

/// How a new selection meets the one there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// The document's frame (what "all" and "invert" mean) and tile size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub width:  u32,
    pub height: u32,
    pub tile:   u32,
}

impl Space {
    pub fn of<P: aurastudio_core::Pixel>(s: &Surface<P>) -> Space { Space { width: s.width(), height: s.height(), tile: s.tile_size() } }
    pub fn frame(&self) -> Rect { Rect::of_size(self.width, self.height) }
    fn mask(&self) -> Mask { Mask::new(self.width, self.height, self.tile, 0) }
}

#[derive(Clone, Debug)]
pub struct Selection {
    mask:    Mask,
    /// Smallest rectangle holding every selected pixel.
    bbox:    Rect,
    /// Exactly its bounding box, fully selected.
    is_rect: bool,
    /// New for every new content (the outline cache's key).
    stamp:   u64,
}

impl PartialEq for Selection {
    fn eq(&self, o: &Self) -> bool {
        self.space() == o.space() && self.bbox == o.bbox && self.mask.read(self.bbox) == o.mask.read(o.bbox)
    }
}

fn next_stamp() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

const MAGIC: [u8; 4] = *b"ASEL";
const VERSION: u16 = 1;

impl Selection {
    /// A mask made or changed: `None` if nothing is selected.
    pub fn of(mask: Mask) -> Option<Self> { Self::finish(mask, false) }

    fn finish(mask: Mask, is_rect: bool) -> Option<Self> {
        // Unselected everywhere past what is stored (an inversion stays
        // within the frame).
        if mask.fill() != 0 { return None; }
        let bbox = mask.pixel_bounds()?;
        Some(Self { mask, bbox, is_rect, stamp: next_stamp() })
    }

    pub fn space(&self) -> Space { Space::of(&self.mask) }

    /// The whole frame.
    pub fn all(space: Space) -> Self {
        let mut m = space.mask();
        let f = space.frame().union(&Rect::new(0, 0, 1, 1));
        m.fill_rect(f, 255);
        Self { mask: m, bbox: f, is_rect: true, stamp: next_stamp() }
    }

    pub fn coverage(&self, x: i32, y: i32) -> u8 { if self.bbox.contains(x, y) { self.mask.pixel(x, y) } else { 0 } }

    /// Selected at all (soft edges included).
    pub fn contains(&self, x: i32, y: i32) -> bool { self.coverage(x, y) > 0 }

    /// Inside the outline (at least half selected).
    pub fn inside(&self, x: i32, y: i32) -> bool { self.coverage(x, y) >= 128 }

    /// Coverage of `r`, one byte per pixel, rows of `r`.
    pub fn mask(&self, r: Rect) -> Vec<u8> { self.mask.read(r) }

    /// The coverage as a surface (a stroke's limit, a filter's selection).
    pub fn as_mask(&self) -> &Mask { &self.mask }

    pub fn into_mask(self) -> Mask { self.mask }

    pub fn bbox(&self) -> Rect { self.bbox }

    pub fn is_rect(&self) -> bool { self.is_rect }

    /// Changes with every new content (cache key).
    pub fn stamp(&self) -> u64 { self.stamp }

    /// `new` met with `old` (`None`: nothing selected).
    pub fn combine(old: Option<&Selection>, new: Option<Selection>, mode: Mode) -> Option<Selection> {
        match (mode, old, new) {
            (Mode::Replace, _, n) => n,
            (Mode::Add, None, n) => n,
            (Mode::Add | Mode::Subtract, Some(o), None) => Some(o.clone()),
            (Mode::Subtract | Mode::Intersect, None, _) | (Mode::Intersect, _, None) => None,
            (m, Some(o), Some(n)) => {
                let op = match m { Mode::Add => Combine::Max, Mode::Subtract => Combine::Subtract, _ => Combine::Min };
                let mut mask = o.mask.clone();
                mask.combine(&n.mask, op);
                // Two rectangles intersected are still one.
                let rect = m == Mode::Intersect && o.is_rect && n.is_rect;
                Self::finish(mask, rect)
            }
        }
    }

    /// Everything else in the frame (`None`: all of it was selected).
    /// Past the frame an inversion selects nothing.
    pub fn inverted(&self) -> Option<Self> {
        let s = self.space();
        let mut m = s.mask();
        m.fill_rect(s.frame(), 255);
        m.combine(&self.mask, Combine::Subtract);
        Self::finish(m, false)
    }

    /// Moved by whole pixels (nothing is lost: the canvas is unbounded).
    pub fn translated(&self, dx: i32, dy: i32) -> Option<Self> {
        let mut m = self.space().mask();
        m.copy_from(&self.mask, self.bbox, dx, dy);
        Self::finish(m, self.is_rect)
    }

    /// Bytes for a file (.aurs) or the clipboard.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_header(&MAGIC, VERSION);
        w.u8(self.is_rect as u8).blob(&self.mask.save());
        w.finish()
    }

    /// Back from `to_bytes`; `None` if damaged or nothing selected.
    pub fn from_bytes(b: &[u8], max_side: u32) -> Option<Self> {
        let mut r = ByteReader::new(b);
        if r.header(&MAGIC)? != VERSION { return None; }
        let rect = r.u8()? == 1;
        let mask = Mask::load(r.blob(b.len())?, max_side)?;
        if !r.at_end() { return None; }
        Self::finish(mask, rect)
    }
}

impl HeapSize for Selection {
    fn heap_bytes(&self) -> usize { self.mask.heap_bytes() }
}

/// Points of a shape, checked: finite.
fn finite(points: &[Point]) -> bool { points.iter().all(|p| p.x.is_finite() && p.y.is_finite()) }

#[cfg(test)]
mod tests;

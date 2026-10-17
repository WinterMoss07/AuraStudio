//! The projection: all layers merged into one image, as shown on screen.
//!
//! Like Krita, the merge happens off the UI thread (here, on the paint
//! thread right after painting); the UI thread only copies the changed
//! rectangles to the GPU. The projection has its own small lock, separate
//! from the document's, so neither thread waits long for the other.

use std::sync::{Arc, Mutex};

use aurastudio_canvas::Rect;

/// Above this many pending rectangles, they are merged into one: cheaper
/// than issuing hundreds of tiny uploads.
const MAX_RECTS: usize = 32;

pub struct Projection {
    /// RGBA, straight alpha, width * height * 4.
    pub pixels: Vec<u8>,
    pub width:  u32,
    pub height: u32,
    /// Areas changed since the UI last uploaded them.
    dirty:      Vec<Rect>,
}

pub type SharedProjection = Arc<Mutex<Projection>>;

impl Projection {
    pub fn new(width: u32, height: u32) -> Self {
        Self { pixels: vec![255; (width * height * 4) as usize], width, height, dirty: Vec::new() }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        *self = Self::new(width, height);
    }

    /// Record a changed area. Kept as a list (Krita does the same): one
    /// bounding box of a long diagonal stroke covers mostly untouched
    /// pixels, and uploading / merging them was most of the old cost.
    /// A rect that mostly overlaps an existing one is merged with it.
    pub fn add_dirty(&mut self, r: Rect) {
        add_rect(&mut self.dirty, r);
    }

    pub fn take_dirty(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.dirty)
    }
}

/// Add `r` to a list of changed areas, merging it into an existing one
/// when that costs no more area than keeping both.
pub fn add_rect(list: &mut Vec<Rect>, r: Rect) {
    if r.0 >= r.2 || r.1 >= r.3 { return; }
    for d in list.iter_mut() {
        let u = union(*d, r);
        if area(u) <= area(*d) + area(r) {
            *d = u;
            return;
        }
    }
    list.push(r);
    if list.len() > MAX_RECTS {
        let all = list.iter().copied().reduce(union).unwrap_or(r);
        list.clear();
        list.push(all);
    }
}

pub fn union(a: Rect, b: Rect) -> Rect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

fn area(r: Rect) -> u64 {
    (r.2 - r.0) as u64 * (r.3 - r.1) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_rects_merge_far_ones_stay_apart() {
        let mut p = Projection::new(1000, 1000);
        p.add_dirty((0, 0, 100, 100));
        p.add_dirty((10, 10, 110, 110)); // mostly overlapping: merged
        p.add_dirty((800, 800, 900, 900)); // far away: kept apart
        let d = p.take_dirty();
        assert_eq!(d, vec![(0, 0, 110, 110), (800, 800, 900, 900)]);
        assert!(p.take_dirty().is_empty());
    }

    #[test]
    fn too_many_rects_collapse_into_one() {
        let mut p = Projection::new(4000, 10);
        for i in 0..(MAX_RECTS as u32 + 1) {
            p.add_dirty((i * 100, 0, i * 100 + 10, 10));
        }
        assert_eq!(p.take_dirty().len(), 1);
    }
}

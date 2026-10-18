//! What the paint thread hands to the screen: pieces of the merged image.
//!
//! Like Krita, the merge happens off the UI thread (here, on the paint
//! thread right after painting). Only the changed rectangles are merged,
//! each into a small buffer of its own, and queued here; the UI thread
//! copies them to the GPU texture and drops them. The whole merged image
//! only exists on the GPU: no second copy in RAM (33 MB at A4 300 dpi,
//! 200 MB at B4 600 dpi). It has its own small lock, separate from the
//! document's, so neither thread waits long for the other.

use std::sync::{Arc, Mutex};

use aurastudio_canvas::Rect;

/// Above this many pending rectangles, they are merged into one: cheaper
/// than issuing hundreds of tiny uploads.
const MAX_RECTS: usize = 32;

pub struct Projection {
    pub width:  u32,
    pub height: u32,
    /// Merged pieces waiting for the GPU, oldest first: (area, RGBA rows
    /// of it, tightly packed).
    patches:    Vec<(Rect, Vec<u8>)>,
}

pub type SharedProjection = Arc<Mutex<Projection>>;

impl Projection {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, patches: Vec::new() }
    }

    /// Queue a merged piece (newer pieces are uploaded after, over older
    /// ones).
    pub fn push(&mut self, rect: Rect, rgba: Vec<u8>) {
        if rect.0 < rect.2 && rect.1 < rect.3 { self.patches.push((rect, rgba)); }
    }

    pub fn take_patches(&mut self) -> Vec<(Rect, Vec<u8>)> {
        std::mem::take(&mut self.patches)
    }

    /// Drop what is queued: the whole image is about to be sent again.
    pub fn clear(&mut self) {
        self.patches.clear();
    }
}

/// Add `r` to a list of changed areas, merging it into an existing one
/// when that costs no more area than keeping both. Kept as a list (Krita
/// does the same): one bounding box of a long diagonal stroke covers mostly
/// untouched pixels, and merging / uploading them was most of the old cost.
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
        let mut list = Vec::new();
        add_rect(&mut list, (0, 0, 100, 100));
        add_rect(&mut list, (10, 10, 110, 110)); // mostly overlapping: merged
        add_rect(&mut list, (800, 800, 900, 900)); // far away: kept apart
        assert_eq!(list, vec![(0, 0, 110, 110), (800, 800, 900, 900)]);
    }

    #[test]
    fn too_many_rects_collapse_into_one() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECTS as u32 + 1) {
            add_rect(&mut list, (i * 100, 0, i * 100 + 10, 10));
        }
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn patches_come_out_in_order_once() {
        let mut p = Projection::new(100, 100);
        p.push((0, 0, 1, 1), vec![1; 4]);
        p.push((0, 0, 0, 1), vec![]);
        p.push((5, 5, 6, 6), vec![2; 4]);
        let got = p.take_patches();
        assert_eq!(got.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![(0, 0, 1, 1), (5, 5, 6, 6)]);
        assert!(p.take_patches().is_empty());
    }
}

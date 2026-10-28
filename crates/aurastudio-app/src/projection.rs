//! The changed pieces of the merged image, from the paint thread to the
//! GPU (the whole image only exists there), behind their own lock.

use std::sync::{Arc, Mutex};

use aurastudio_core::Rect;

/// Above this many pending rectangles, they are merged into one: cheaper
/// than issuing hundreds of tiny uploads. (An undone diagonal stroke comes
/// as one rectangle per row of tiles: ~50 for 3000 px.)
const MAX_RECTS: usize = 128;

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

    /// Bytes of the merged pieces waiting.
    pub fn pending_bytes(&self) -> u64 { self.patches.iter().map(|p| p.1.len() as u64).sum() }

    pub fn take_patches(&mut self) -> Vec<(Rect, Vec<u8>)> {
        std::mem::take(&mut self.patches)
    }

    /// Drop what is queued: the whole image is about to be sent again.
    pub fn clear(&mut self) {
        self.patches.clear();
    }
}

/// Add `r` to the changed areas, merged with one when that costs no more
/// area (a list, not one box: a diagonal stroke's box is mostly empty).
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

pub use aurastudio_core::union;

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

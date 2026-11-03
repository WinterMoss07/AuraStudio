//! The changed pieces of the merged image, from the paint thread to the
//! GPU (the whole image only exists there), behind their own lock.

use std::sync::{Arc, Mutex};

use aurastudio_core::{Rect, Rgba8};

/// Above this many pending rectangles, they are merged into one: cheaper
/// than issuing hundreds of tiny uploads.
const MAX_RECTS: usize = 128;

pub struct Projection {
    pub width:  u32,
    pub height: u32,
    /// Merged pieces waiting for the GPU, oldest first: (area, its rows).
    patches:    Vec<(Rect, Vec<Rgba8>)>,
}

pub type SharedProjection = Arc<Mutex<Projection>>;

impl Projection {
    pub fn new(width: u32, height: u32) -> Self { Self { width, height, patches: Vec::new() } }

    /// Queue a merged piece (newer pieces are uploaded after, over older
    /// ones).
    pub fn push(&mut self, rect: Rect, px: Vec<Rgba8>) {
        if !rect.is_empty() && px.len() as u64 == rect.area() { self.patches.push((rect, px)); }
    }

    /// Bytes of the merged pieces waiting.
    pub fn pending_bytes(&self) -> u64 { self.patches.iter().map(|p| p.1.len() as u64 * 4).sum() }

    pub fn take_patches(&mut self) -> Vec<(Rect, Vec<Rgba8>)> { std::mem::take(&mut self.patches) }

    /// Drop what is queued: the whole image is about to be sent again.
    pub fn clear(&mut self) { self.patches.clear(); }
}

/// Add `r` to the changed areas, merged with one when that costs no more
/// area (a list, not one box: a diagonal stroke's box is mostly empty).
pub fn add_rect(list: &mut Vec<Rect>, r: Rect) {
    if r.is_empty() { return; }
    for d in list.iter_mut() {
        let u = d.union(&r);
        if u.area() <= d.area() + r.area() {
            *d = u;
            return;
        }
    }
    list.push(r);
    if list.len() > MAX_RECTS {
        let all = list.iter().fold(r, |a, b| a.union(b));
        list.clear();
        list.push(all);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_rects_merge_far_ones_stay_apart() {
        let mut list = Vec::new();
        add_rect(&mut list, Rect::new(0, 0, 100, 100));
        add_rect(&mut list, Rect::new(10, 10, 110, 110));
        add_rect(&mut list, Rect::new(800, 800, 900, 900));
        assert_eq!(list, vec![Rect::new(0, 0, 110, 110), Rect::new(800, 800, 900, 900)]);
    }

    #[test]
    fn too_many_rects_collapse_into_one() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECTS as i32 + 1) { add_rect(&mut list, Rect::new(i * 100, 0, i * 100 + 10, 10)); }
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn patches_come_out_in_order_once() {
        let mut p = Projection::new(100, 100);
        p.push(Rect::new(0, 0, 1, 1), vec![[1; 4]]);
        p.push(Rect::new(0, 0, 0, 1), vec![]);
        p.push(Rect::new(5, 5, 6, 6), vec![[2; 4]]);
        let got = p.take_patches();
        assert_eq!(got.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![Rect::new(0, 0, 1, 1), Rect::new(5, 5, 6, 6)]);
        assert!(p.take_patches().is_empty());
    }
}

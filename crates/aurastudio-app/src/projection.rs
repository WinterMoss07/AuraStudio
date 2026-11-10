//! The changed pieces of the merged image, from the paint thread to the
//! GPU (the whole image only exists there), behind their own lock.

use std::sync::{Arc, Mutex};

use aurastudio_core::{Rect, Rgba8};

/// Above this many pending rectangles, they are joined per GPU page (one
/// box per page touched): cheaper than hundreds of tiny merges and
/// uploads, still not the box of a whole diagonal stroke.
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
    /// ones). Older pieces it covers whole are dropped: they'd be uploaded
    /// for nothing (a UI that waits piles them up).
    pub fn push(&mut self, rect: Rect, px: Vec<Rgba8>) {
        if rect.is_empty() || px.len() as u64 != rect.area() { return; }
        self.patches.retain(|(r, _)| !rect.contains_rect(r));
        self.patches.push((rect, px));
    }

    /// Bytes of the merged pieces waiting.
    pub fn pending_bytes(&self) -> u64 { self.patches.iter().map(|p| p.1.len() as u64 * 4).sum() }

    pub fn take_patches(&mut self) -> Vec<(Rect, Vec<Rgba8>)> { std::mem::take(&mut self.patches) }
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
    if list.len() > MAX_RECTS { *list = by_page(list); }
}

/// The rects joined per GPU page: one box in each page they touch.
fn by_page(list: &[Rect]) -> Vec<Rect> {
    let p = aurastudio_gpu::PAGE as i32;
    let mut pages: std::collections::BTreeMap<(i32, i32), Rect> = std::collections::BTreeMap::new();
    for r in list {
        for py in r.y0.div_euclid(p)..=(r.y1 - 1).div_euclid(p) {
            for px in r.x0.div_euclid(p)..=(r.x1 - 1).div_euclid(p) {
                let part = r.intersect(&Rect::new(px * p, py * p, px * p + p, py * p + p));
                if part.is_empty() { continue; }
                pages.entry((py, px)).and_modify(|b| *b = b.union(&part)).or_insert(part);
            }
        }
    }
    pages.into_values().collect()
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
    fn too_many_rects_join_per_page() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECTS as i32 + 1) { add_rect(&mut list, Rect::new(i * 100, 0, i * 100 + 10, 10)); }
        let p = aurastudio_gpu::PAGE as i32;
        assert_eq!(list.len() as i32, (MAX_RECTS as i32 * 100 + 10 + p - 1) / p);
        // A diagonal: one box per page it crosses, not its whole box.
        let mut list = Vec::new();
        for i in 0..(MAX_RECTS as i32 + 1) { add_rect(&mut list, Rect::new(i * 20, i * 20, i * 20 + 8, i * 20 + 8)); }
        let area: u64 = list.iter().map(|r| r.area()).sum();
        let whole = Rect::new(0, 0, MAX_RECTS as i32 * 20 + 8, MAX_RECTS as i32 * 20 + 8).area();
        assert!(area * 4 < whole, "{area} of {whole}");
    }

    #[test]
    fn a_covered_patch_is_dropped() {
        let mut p = Projection::new(100, 100);
        p.push(Rect::new(2, 2, 4, 4), vec![[1; 4]; 4]);
        p.push(Rect::new(3, 3, 6, 6), vec![[2; 4]; 9]);
        p.push(Rect::new(0, 0, 5, 5), vec![[3; 4]; 25]);
        let rects: Vec<Rect> = p.take_patches().into_iter().map(|x| x.0).collect();
        assert_eq!(rects, [Rect::new(3, 3, 6, 6), Rect::new(0, 0, 5, 5)]);
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

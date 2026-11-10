//! The changed pieces of the merged image, from the paint thread to the
//! GPU (the whole image only exists there), behind their own lock: cut
//! along the GPU pages and sorted before they come here (the interface
//! thread only writes them).

use std::sync::{Arc, Mutex};

use aurastudio_core::Rect;
use aurastudio_gpu::Piece;

/// Above this many pending rectangles, they are joined per GPU page (one
/// box per page touched): cheaper than hundreds of tiny merges and
/// uploads, still not the box of a whole diagonal stroke.
const MAX_RECTS: usize = 128;

pub struct Projection {
    pub width:  u32,
    pub height: u32,
    /// Pieces waiting for the GPU, oldest first.
    pieces:     Vec<Piece>,
}

pub type SharedProjection = Arc<Mutex<Projection>>;

impl Projection {
    pub fn new(width: u32, height: u32) -> Self { Self { width, height, pieces: Vec::new() } }

    /// Queue pieces of this canvas size (newer pieces are written after,
    /// over older ones). Older pieces they cover whole are dropped: they'd
    /// be written for nothing (a UI that waits piles them up).
    pub fn push(&mut self, pieces: Vec<Piece>) {
        for p in pieces {
            let r = p.rect();
            if r.is_empty() { continue; }
            self.pieces.retain(|q| !r.contains_rect(&q.rect()));
            self.pieces.push(p);
        }
    }

    /// Bytes of the pieces waiting.
    pub fn pending_bytes(&self) -> u64 { self.pieces.iter().map(|p| p.bytes() as u64).sum() }

    pub fn take_pieces(&mut self) -> Vec<Piece> { std::mem::take(&mut self.pieces) }
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

    fn pieces(r: Rect, c: u8) -> Vec<Piece> { aurastudio_gpu::prepare((1000, 1000), r, vec![[c; 4]; r.area() as usize]) }

    #[test]
    fn a_covered_piece_is_dropped() {
        let mut p = Projection::new(1000, 1000);
        p.push(pieces(Rect::new(2, 2, 4, 4), 1));
        p.push(pieces(Rect::new(3, 3, 6, 6), 2));
        p.push(pieces(Rect::new(0, 0, 5, 5), 3));
        let rects: Vec<Rect> = p.take_pieces().into_iter().map(|x| x.rect()).collect();
        assert_eq!(rects, [Rect::new(3, 3, 6, 6), Rect::new(0, 0, 5, 5)]);
    }

    #[test]
    fn pieces_come_out_in_order_once_cut_by_page() {
        let mut p = Projection::new(1000, 1000);
        p.push(pieces(Rect::new(0, 0, 1, 1), 1));
        p.push(pieces(Rect::new(0, 0, 0, 1), 2));
        p.push(pieces(Rect::new(250, 5, 260, 6), 3));
        let got: Vec<Rect> = p.take_pieces().iter().map(Piece::rect).collect();
        assert_eq!(got, vec![Rect::new(0, 0, 1, 1), Rect::new(250, 5, 256, 6), Rect::new(256, 5, 260, 6)]);
        assert!(p.take_pieces().is_empty());
    }
}

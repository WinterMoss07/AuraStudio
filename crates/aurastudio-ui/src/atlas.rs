//! The interface's one texture cache (`ui`): icons, layer thumbnails,
//! recent files' thumbnails, stroke previews. One egui texture per picture
//! means one GPU object, one memory block and one draw call each; here
//! pictures share pages of 512 × 512 (1 MB), made one at a time when needed
//! (a bigger page cost 4 MB for a few icons), packed in rows ("shelves").
//!
//! - A freed place is given out again to a picture that fits it (the
//!   smallest such place, not much bigger than the picture).
//! - A page with nothing left in it starts over; one that stays empty for a
//!   while (not the first) goes back to the GPU (`tidy`).
//! - In a smoothed (LINEAR) atlas each picture has a one pixel ring that
//!   repeats its edge, so smoothing never reads a neighbour or a gap.
//! - A picture too big for a page is the caller's to keep on a texture of
//!   its own (`fits`); when the pages are full, `alloc` says None and the
//!   caller keeps showing what it had or tries again later (it never
//!   remembers the failure).

use std::time::{Duration, Instant};

use egui::{pos2, Color32, ColorImage, Context, Rect, TextureFilter, TextureHandle, TextureId, TextureOptions};

/// Side of a page, pixels (1 MB).
pub const PAGE: usize = 512;

/// Pages the interface's cache may hold (8 MB at most).
const UI_PAGES: usize = 8;

thread_local! {
    static UI: std::cell::RefCell<Atlas> = std::cell::RefCell::new(Atlas::new("ui", TextureOptions::LINEAR, UI_PAGES));
}

/// The interface's one texture cache (UI thread). Not to be called again
/// from inside `f`.
pub fn ui<R>(f: impl FnOnce(&mut Atlas) -> R) -> R { UI.with(|a| f(&mut a.borrow_mut())) }

/// A picture in the cache, freed when dropped.
pub struct Pic(pub Region);

impl Drop for Pic {
    fn drop(&mut self) { let r = self.0; let _ = UI.try_with(|a| { if let Ok(mut a) = a.try_borrow_mut() { a.free(r); } }); }
}

/// Draw region `r` of the cache in `rect`, tinted.
pub fn paint(painter: &egui::Painter, r: Region, rect: Rect, tint: Color32) {
    if let Some((id, uv)) = ui(|a| a.image_of(r)) { painter.image(id, rect, uv, tint); }
}

/// Pixels around each picture (its ring).
const RING: usize = 1;

/// A page empty this long (not the first) goes back to the GPU.
const EMPTY_FOR: Duration = Duration::from_secs(2);

/// A freed place is only given to a picture of at least this share of its area.
const MIN_FILL: f32 = 0.5;

/// Where a picture is: page and pixels, and the size of the place it was
/// given (at least the picture's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    page: u16,
    x:    u16,
    y:    u16,
    pub w: u16,
    pub h: u16,
    cap_w: u16,
    cap_h: u16,
}

impl Region {
    /// Pixels its place takes in the page (ring included).
    fn footprint(&self) -> usize { (self.cap_w as usize + 2 * RING) * (self.cap_h as usize + 2 * RING) }
}

/// One page of the cache, as the Memory report shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PageUse {
    pub bytes:    usize,
    /// Share of the page its pictures take, 0..1.
    pub filled:   f32,
    pub pictures: usize,
}

struct Shelf { y: usize, h: usize, x: usize }

struct Page {
    tex:         TextureHandle,
    shelves:     Vec<Shelf>,
    /// Freed places, to give out again.
    free:        Vec<Region>,
    /// Pictures in the page.
    live:        usize,
    /// Pixels their places take (rings included).
    area:        usize,
    empty_since: Option<Instant>,
}

pub struct Atlas {
    name:      &'static str,
    options:   TextureOptions,
    /// A released page leaves a hole (None) so that the page numbers held
    /// by regions never move.
    pages:     Vec<Option<Page>>,
    max_pages: usize,
}

impl Atlas {
    /// `max_pages`: pages it may hold at once; beyond it `alloc` says no.
    pub fn new(name: &'static str, options: TextureOptions, max_pages: usize) -> Self {
        Self { name, options, pages: Vec::new(), max_pages }
    }

    /// Whether a `w` × `h` picture can ever be placed (else: its own texture).
    pub fn fits(w: usize, h: usize) -> bool {
        w > 0 && h > 0 && w + 2 * RING <= PAGE && h + 2 * RING <= PAGE
    }

    /// Room for a `w` × `h` picture, or None: too big for a page, or the
    /// pages are full (nothing is remembered: ask again later).
    pub fn alloc(&mut self, ctx: &Context, w: usize, h: usize) -> Option<Region> {
        if !Self::fits(w, h) { return None; }
        for (i, p) in self.pages.iter_mut().enumerate() {
            let Some(p) = p else { continue };
            if let Some(r) = p.take(i, w, h) { return Some(r); }
        }
        if self.pages.iter().flatten().count() >= self.max_pages { return None; }
        let tex = ctx.load_texture(self.name, ColorImage::filled([PAGE, PAGE], Color32::TRANSPARENT), self.options);
        let page = Page { tex, shelves: Vec::new(), free: Vec::new(), live: 0, area: 0, empty_since: None };
        let i = match self.pages.iter().position(Option::is_none) {
            Some(i) => { self.pages[i] = Some(page); i }
            None => { self.pages.push(Some(page)); self.pages.len() - 1 }
        };
        self.pages[i].as_mut()?.take(i, w, h)
    }

    /// Put `image` (the size of the region) in its place.
    pub fn write(&mut self, r: Region, image: ColorImage) {
        assert_eq!(image.size, [r.w as usize, r.h as usize], "a picture is written at its own size");
        self.write_part(r, 0, 0, image);
    }

    /// Put `image` at (x, y) inside the picture of region `r` (the ring
    /// follows where the part reaches the picture's edge).
    pub fn write_part(&mut self, r: Region, x: usize, y: usize, image: ColorImage) {
        let [w, h] = image.size;
        assert!(x + w <= r.w as usize && y + h <= r.h as usize, "a part stays inside its picture");
        if w == 0 || h == 0 { return; }
        let smooth = self.options.magnification == TextureFilter::Linear || self.options.minification == TextureFilter::Linear;
        let options = self.options;
        let Some(p) = self.page_mut(r) else { return };
        if !smooth {
            p.tex.set_partial([r.x as usize + x, r.y as usize + y], image, options);
            return;
        }
        let edge = |at_edge: bool| if at_edge { RING } else { 0 };
        let (left, top) = (edge(x == 0), edge(y == 0));
        let (right, bottom) = (edge(x + w == r.w as usize), edge(y + h == r.h as usize));
        let ringed = with_ring(&image, [left, top, right, bottom]);
        p.tex.set_partial([r.x as usize + x - left, r.y as usize + y - top], ringed, options);
    }

    /// The region is free again.
    pub fn free(&mut self, r: Region) { self.free_at(r, Instant::now()); }

    fn free_at(&mut self, r: Region, now: Instant) {
        let Some(p) = self.page_mut(r) else { return };
        p.live = p.live.saturating_sub(1);
        p.area = p.area.saturating_sub(r.footprint());
        if p.live == 0 {
            // Nothing left: the page starts over (all of it is free).
            p.shelves.clear();
            p.free.clear();
            p.empty_since = Some(now);
        } else {
            p.free.push(r);
        }
    }

    /// The page texture and the part of it the picture covers (0..1);
    /// None for a region of a released page.
    pub fn image_of(&self, r: Region) -> Option<(TextureId, Rect)> {
        let page = self.pages.get(r.page as usize)?.as_ref()?;
        let s = PAGE as f32;
        let uv = Rect::from_min_max(pos2(r.x as f32 / s, r.y as f32 / s), pos2((r.x + r.w) as f32 / s, (r.y + r.h) as f32 / s));
        Some((page.tex.id(), uv))
    }

    /// Once per frame: pages (not the first) empty for a while go back to the GPU.
    pub fn tidy(&mut self) { self.tidy_at(Instant::now()); }

    fn tidy_at(&mut self, now: Instant) {
        for (i, slot) in self.pages.iter_mut().enumerate() {
            if i > 0 && slot.as_ref().is_some_and(|p| p.live == 0 && p.empty_since.is_some_and(|t| now.saturating_duration_since(t) >= EMPTY_FOR)) {
                *slot = None;
            }
        }
        while self.pages.last().is_some_and(Option::is_none) { self.pages.pop(); }
    }

    /// Pages held.
    #[cfg(test)]
    pub fn pages(&self) -> usize { self.pages.iter().flatten().count() }

    /// Bytes the pages hold on the GPU.
    #[cfg(test)]
    pub fn bytes(&self) -> usize { self.pages.iter().flatten().count() * PAGE * PAGE * 4 }

    /// Each page held: its bytes, the share its pictures take, how many
    /// (the Memory report).
    pub fn usage(&self) -> Vec<PageUse> {
        self.pages.iter().flatten().map(|p| PageUse { bytes: PAGE * PAGE * 4, filled: p.area as f32 / (PAGE * PAGE) as f32, pictures: p.live }).collect()
    }

    fn page_mut(&mut self, r: Region) -> Option<&mut Page> { self.pages.get_mut(r.page as usize)?.as_mut() }
}

/// `image` with a ring that repeats its edge, `[left, top, right,
/// bottom]` pixels wide.
fn with_ring(image: &ColorImage, [l, t, r, b]: [usize; 4]) -> ColorImage {
    let (w, h) = (image.size[0], image.size[1]);
    let rw = w + l + r;
    let mut ringed = ColorImage::filled([rw, h + t + b], Color32::TRANSPARENT);
    for y in 0..h + t + b {
        let sy = y.saturating_sub(t).min(h - 1);
        for x in 0..rw {
            let sx = x.saturating_sub(l).min(w - 1);
            ringed.pixels[y * rw + x] = image.pixels[sy * w + sx];
        }
    }
    ringed
}

impl Page {
    fn take(&mut self, page: usize, w: usize, h: usize) -> Option<Region> {
        let side = PAGE;
        let (bw, bh) = (w + 2 * RING, h + 2 * RING);
        // A freed place that fits: the smallest, if the picture fills enough of it.
        let best = self.free.iter().enumerate()
            .filter(|(_, r)| r.cap_w as usize >= w && r.cap_h as usize >= h
                && (w * h) as f32 >= MIN_FILL * (r.cap_w as usize * r.cap_h as usize) as f32)
            .min_by_key(|(_, r)| r.cap_w as usize * r.cap_h as usize).map(|(i, _)| i);
        if let Some(i) = best {
            let mut r = self.free.swap_remove(i);
            r.w = w as u16;
            r.h = h as u16;
            return Some(self.taken(r));
        }
        let region = |x: usize, y: usize| Region { page: page as u16, x: (x + RING) as u16, y: (y + RING) as u16, w: w as u16, h: h as u16, cap_w: w as u16, cap_h: h as u16 };
        // A shelf as high as the picture (up to a quarter more) with room.
        for s in &mut self.shelves {
            if s.h >= bh && s.h <= bh + bh / 4 && s.x + bw <= side {
                let r = region(s.x, s.y);
                s.x += bw;
                return Some(self.taken(r));
            }
        }
        let y = self.shelves.last().map_or(0, |s| s.y + s.h);
        if y + bh <= side && bw <= side {
            self.shelves.push(Shelf { y, h: bh, x: bw });
            return Some(self.taken(region(0, y)));
        }
        None
    }

    fn taken(&mut self, r: Region) -> Region {
        self.live += 1;
        self.area += r.footprint();
        self.empty_since = None;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(a: Region, b: Region) -> bool {
        // (The rings count: no two pictures share a pixel of either.)
        let (ax, ay, bx, by) = (a.x as i32 - 1, a.y as i32 - 1, b.x as i32 - 1, b.y as i32 - 1);
        let (aw, ah, bw, bh) = (a.cap_w as i32 + 2, a.cap_h as i32 + 2, b.cap_w as i32 + 2, b.cap_h as i32 + 2);
        a.page == b.page && ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
    }

    #[test]
    fn pictures_never_overlap_and_stay_in_their_page() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::NEAREST, 8);
        let mut rs: Vec<Region> = Vec::new();
        let mut seed = 7u32;
        for _ in 0..400 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let (w, h) = (4 + (seed >> 8) as usize % 90, 4 + (seed >> 20) as usize % 40);
            let Some(r) = at.alloc(&ctx, w, h) else { break };
            assert!((r.x as usize + r.w as usize + 1) <= PAGE && (r.y as usize + r.h as usize + 1) <= PAGE);
            for o in &rs { assert!(!overlap(r, *o), "{r:?} {o:?}"); }
            rs.push(r);
            let (_, uv) = at.image_of(r).unwrap();
            assert!(uv.min.x >= 0.0 && uv.max.x <= 1.0 && uv.max.y <= 1.0);
        }
        assert!(rs.len() > 100 && at.pages() > 1, "{} {}", rs.len(), at.pages());
    }

    #[test]
    fn a_freed_region_is_given_out_again_and_the_pages_are_capped() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::NEAREST, 1);
        let a = at.alloc(&ctx, 100, 50).unwrap();
        let b = at.alloc(&ctx, 100, 50).unwrap();
        assert_ne!(a, b);
        at.free(a);
        assert_eq!(at.alloc(&ctx, 100, 50), Some(a));
        // Too big for a page.
        assert!(at.alloc(&ctx, PAGE, 10).is_none());
        // One page: fills up, then None (and asking again is fine).
        let mut n = 2;
        while at.alloc(&ctx, 100, 50).is_some() { n += 1; assert!(n < 1000); }
        assert!(n >= 6 && at.pages() == 1);
        assert!(at.alloc(&ctx, 100, 50).is_none());
    }

    /// Pages of 1 MB, one more only when the others are full; the report
    /// tells how full each is.
    #[test]
    fn pages_are_1_mb_each_made_when_needed() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::LINEAR, 4);
        let a = at.alloc(&ctx, 20, 20).unwrap();
        assert_eq!(at.bytes(), 1 << 20);
        let (_, uv) = at.image_of(a).unwrap();
        assert!((uv.max.x - (a.x as f32 + 20.0) / PAGE as f32).abs() < 1e-6);
        let u = at.usage();
        assert_eq!((u.len(), u[0].pictures), (1, 1));
        assert!((u[0].filled - (22.0 * 22.0) / (PAGE * PAGE) as f32).abs() < 1e-6);
        // Room left in the first page: no second one.
        let b = at.alloc(&ctx, 400, 300).unwrap();
        assert_eq!((at.pages(), b.page), (1, 0));
        // No room left: a second page of the same size.
        let c = at.alloc(&ctx, 400, 300).unwrap();
        assert_eq!((at.pages(), c.page, at.bytes()), (2, 1, 2 << 20));
        at.free(a);
        assert_eq!(at.usage()[0].pictures, 1);
        // Too wide for a page: the caller's own texture.
        assert!(!Atlas::fits(PAGE - 1, 10) && Atlas::fits(PAGE - 2, 10));
        assert!(at.alloc(&ctx, PAGE + 10, 10).is_none());
    }

    /// Everything freed: the page starts over, so pictures of other sizes
    /// fill it as if new (an exact-size reuse would leave it unusable).
    #[test]
    fn an_emptied_page_is_reset_and_reused_for_other_sizes() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::NEAREST, 1);
        let mut first = Vec::new();
        while let Some(r) = at.alloc(&ctx, 100, 50) { first.push(r); assert!(first.len() < 1000); }
        let capacity = first.len();
        assert!(at.alloc(&ctx, 80, 80).is_none(), "full of 100 x 50");
        for r in first { at.free(r); }
        let mut second = Vec::new();
        while let Some(r) = at.alloc(&ctx, 80, 80) { second.push(r); assert!(second.len() < 1000); }
        assert!(second.len() >= capacity * 50 * 100 / (80 * 80) / 2, "{} of 80x80 after {capacity} of 100x50", second.len());
        for (i, a) in second.iter().enumerate() {
            for b in &second[i + 1..] { assert!(!overlap(*a, *b)); }
        }
        assert_eq!(at.pages(), 1);
    }

    /// A hole bigger than the picture is used if the picture fills half of it
    /// at least; a much bigger hole is left for bigger pictures.
    #[test]
    fn best_fit_reuses_a_bigger_hole_but_not_a_huge_one() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::NEAREST, 1);
        let big = at.alloc(&ctx, 60, 60).unwrap();
        let mid = at.alloc(&ctx, 40, 40).unwrap();
        let _keep = at.alloc(&ctx, 10, 10).unwrap();
        at.free(big);
        at.free(mid);
        // 36 x 36 fits the 40 x 40 hole (81 % of it), not worth the 60 x 60 one.
        let r = at.alloc(&ctx, 36, 36).unwrap();
        assert_eq!((r.x, r.y), (mid.x, mid.y));
        assert_eq!((r.w, r.h), (36, 36));
        // 20 x 20 is too small for either hole (11 % of the big one): a new place.
        let small = at.alloc(&ctx, 20, 20).unwrap();
        assert!(small.x != big.x || small.y != big.y);
        // The big hole still serves a picture of its size.
        let again = at.alloc(&ctx, 58, 58).unwrap();
        assert_eq!((again.x, again.y), (big.x, big.y));
    }

    #[test]
    fn an_empty_page_other_than_the_first_goes_back_after_two_seconds() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::NEAREST, 3);
        let a = at.alloc(&ctx, 400, 400).unwrap();
        let b = at.alloc(&ctx, 400, 400).unwrap();
        assert_eq!(at.pages(), 2, "a 400-wide pair needs two pages");
        let t0 = Instant::now();
        at.free_at(a, t0);
        at.free_at(b, t0);
        at.tidy_at(t0 + Duration::from_secs(1));
        assert_eq!(at.pages(), 2);
        at.tidy_at(t0 + Duration::from_secs(3));
        assert_eq!(at.pages(), 1, "the first page stays");
        // A later picture takes a page again, in the same numbering.
        let c = at.alloc(&ctx, 400, 400).unwrap();
        let d = at.alloc(&ctx, 400, 400).unwrap();
        assert!(at.image_of(c).unwrap().0 != at.image_of(d).unwrap().0);
    }

    /// In a smoothed atlas, the ring repeats the picture's edge pixels.
    #[test]
    fn the_ring_repeats_the_edge_pixels() {
        let image = ColorImage::new([2, 2], vec![Color32::RED, Color32::GREEN, Color32::BLUE, Color32::WHITE]);
        let r = with_ring(&image, [RING; 4]);
        assert_eq!(r.size, [4, 4]);
        let at = |x: usize, y: usize| r.pixels[y * 4 + x];
        assert_eq!((at(0, 0), at(3, 0), at(0, 3), at(3, 3)), (Color32::RED, Color32::GREEN, Color32::BLUE, Color32::WHITE));
        assert_eq!((at(1, 1), at(2, 1), at(1, 2), at(2, 2)), (Color32::RED, Color32::GREEN, Color32::BLUE, Color32::WHITE));
        assert_eq!((at(0, 1), at(3, 2), at(1, 0), at(2, 3)), (Color32::RED, Color32::WHITE, Color32::RED, Color32::WHITE));
        // A part at the picture's left edge only: a ring on that side only.
        let r = with_ring(&image, [RING, 0, 0, 0]);
        assert_eq!(r.size, [3, 2]);
        assert_eq!((r.pixels[0], r.pixels[1], r.pixels[2]), (Color32::RED, Color32::RED, Color32::GREEN));
    }

    /// Writing into a smoothed atlas does not panic at the page's edges
    /// (the ring of a picture in a corner stays inside the page).
    #[test]
    fn writing_a_corner_picture_stays_inside_the_page() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::LINEAR, 1);
        let r = at.alloc(&ctx, 10, 10).unwrap();
        assert!(r.x >= 1 && r.y >= 1);
        at.write(r, ColorImage::filled([10, 10], Color32::WHITE));
    }
}

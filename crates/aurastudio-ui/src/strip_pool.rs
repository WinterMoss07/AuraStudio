//! Same-size pictures (the stroke preview strips) stacked in a few small
//! textures, one class per size. A page holds only the slots its size
//! needs (the strips drawn plus a small margin), and stays under 1 MiB once
//! the driver has padded it: the Vulkan allocator rounds every texture
//! under 8 MB up to a power of two, so 1.05 MB would cost 2 MB.
//!
//! Pages that stay empty go back to the GPU after a while; the pages of a
//! size no longer drawn (the docker changed width, another screen scale)
//! go as soon as they are empty.
//!
//! Why not the shelf atlas: every strip has the same size, so a page is a
//! column of equal slots, nothing fragments, and a strip wider than an
//! atlas page (560 px at 200 %) still fits.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{pos2, Color32, ColorImage, Context, Rect, TextureHandle, TextureId, TextureOptions};

/// What a page may weigh once padded by the driver (bytes): under the
/// 1 MiB block it lands in.
const PAGE_BUDGET: usize = (1 << 20) - (64 << 10);
/// Largest texture side the pool asks for.
const MAX_SIDE: usize = 8192;
/// A page of a size still drawn, empty this long, goes back to the GPU.
const EMPTY_FOR: Duration = Duration::from_secs(2);
/// A size not drawn for this many passes (and holding nothing) goes away.
const UNUSED_PASSES: u64 = 2;

type Class = (u16, u16);

/// A place for one strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Strip {
    w: u16,
    h: u16,
    page: u16,
    slot: u16,
}

impl Strip {
    /// Its size, pixels.
    pub fn size(&self) -> (usize, usize) { (self.w as usize, self.h as usize) }
}

struct Page {
    tex:         TextureHandle,
    /// One per slot (its length is the page's slot count).
    used:        Vec<bool>,
    empty_since: Option<Instant>,
}

struct Sized {
    /// A released page leaves a hole (None) so that the indices held by
    /// strips never move.
    pages:      Vec<Option<Page>>,
    last_drawn: u64,
}

impl Sized {
    fn capacity(&self) -> usize { self.pages.iter().flatten().map(|p| p.used.len()).sum() }
}

#[derive(Default)]
pub struct StripPool {
    classes: HashMap<Class, Sized>,
    /// Pages made since the start (for the report and the tests).
    created: u64,
}

/// What the driver makes of a `w` × `rows` RGBA texture, about: rows of
/// 128 bytes and tiles of 32 rows (Intel's tiling; others pad less).
fn padded_bytes(w: usize, rows: usize) -> usize { (w * 4).div_ceil(128) * 128 * rows.div_ceil(32) * 32 }

/// Most slots a page of `w` × `h` strips may have and stay in its block
/// (at least one).
pub fn max_slots(w: usize, h: usize) -> usize {
    let mut n = (MAX_SIDE / (h + 1).max(1)).max(1);
    while n > 1 && padded_bytes(w, n * (h + 1)) > PAGE_BUDGET { n -= 1; }
    n
}

impl StripPool {
    /// Whether a strip of this size can have a place at all.
    pub fn fits(w: usize, h: usize) -> bool {
        w > 0 && h > 0 && w <= MAX_SIDE && h < MAX_SIDE && w <= u16::MAX as usize && h <= u16::MAX as usize
    }

    /// Note that a strip of this size was drawn in `pass`.
    pub fn mark_drawn(&mut self, w: usize, h: usize, pass: u64) {
        if let Some(c) = self.classes.get_mut(&(w as u16, h as u16)) { c.last_drawn = pass; }
    }

    /// A free slot for a `w` × `h` strip. A page is added only while the
    /// size holds fewer than `wanted` slots, with just the slots missing.
    /// None: no room (the caller lets an old strip go, or waits).
    pub fn alloc(&mut self, ctx: &Context, w: usize, h: usize, wanted: usize, pass: u64) -> Option<Strip> {
        if !Self::fits(w, h) { return None; }
        let class = self.classes.entry((w as u16, h as u16)).or_insert_with(|| Sized { pages: Vec::new(), last_drawn: pass });
        class.last_drawn = pass;
        let make = |page: usize, slot: usize| Strip { w: w as u16, h: h as u16, page: page as u16, slot: slot as u16 };
        for (pi, p) in class.pages.iter_mut().enumerate() {
            let Some(p) = p else { continue };
            if let Some(si) = p.used.iter().position(|u| !u) {
                p.used[si] = true;
                p.empty_since = None;
                return Some(make(pi, si));
            }
        }
        let have = class.capacity();
        let wanted = wanted.max(1);
        if have >= wanted { return None; }
        let n = (wanted - have).min(max_slots(w, h));
        let tex = ctx.load_texture(
            format!("stroke-strips {w}x{h}"),
            ColorImage::new([w, n * (h + 1)], Color32::TRANSPARENT),
            TextureOptions::NEAREST,
        );
        self.created += 1;
        let page = Page { tex, used: vec![false; n], empty_since: None };
        let pi = match class.pages.iter().position(|p| p.is_none()) {
            Some(i) => { class.pages[i] = Some(page); i }
            None => { class.pages.push(Some(page)); class.pages.len() - 1 }
        };
        class.pages[pi].as_mut()?.used[0] = true;
        Some(make(pi, 0))
    }

    /// Put `image` (the strip's size) in its slot.
    pub fn write(&mut self, s: Strip, image: ColorImage) {
        assert_eq!(image.size, [s.w as usize, s.h as usize], "a strip is written at its own size");
        if let Some(p) = self.page_mut(s) {
            p.tex.set_partial([0, s.slot as usize * (s.h as usize + 1)], image, TextureOptions::NEAREST);
        }
    }

    /// The slot is free again.
    pub fn free(&mut self, s: Strip, now: Instant) {
        if let Some(p) = self.page_mut(s) {
            if let Some(u) = p.used.get_mut(s.slot as usize) { *u = false; }
            if p.used.iter().all(|u| !u) { p.empty_since = Some(now); }
        }
    }

    /// The page texture and the part of it the strip covers.
    pub fn image(&self, s: Strip) -> Option<(TextureId, Rect)> {
        let class = self.classes.get(&(s.w, s.h))?;
        let p = class.pages.get(s.page as usize)?.as_ref()?;
        let total = (p.used.len() * (s.h as usize + 1)) as f32;
        let y0 = s.slot as f32 * (s.h as f32 + 1.0);
        Some((p.tex.id(), Rect::from_min_max(pos2(0.0, y0 / total), pos2(1.0, (y0 + s.h as f32) / total))))
    }

    /// Once per pass: empty pages go back to the GPU (at once for a size
    /// no longer drawn, after a while otherwise), then sizes left with
    /// nothing.
    pub fn tidy(&mut self, pass: u64, now: Instant) {
        for class in self.classes.values_mut() {
            let abandoned = pass > class.last_drawn + 1;
            for slot in &mut class.pages {
                let release = slot.as_ref().is_some_and(|p| p.empty_since.is_some_and(|t| abandoned || now.saturating_duration_since(t) >= EMPTY_FOR));
                if release { *slot = None; }
            }
            while class.pages.last().is_some_and(|p| p.is_none()) { class.pages.pop(); }
        }
        self.classes.retain(|_, c| !(c.pages.is_empty() && pass > c.last_drawn + UNUSED_PASSES));
    }

    /// When the next page would be released (for a repaint then).
    pub fn next_release(&self, now: Instant) -> Option<Duration> {
        self.classes.values().flat_map(|c| c.pages.iter().flatten())
            .filter_map(|p| p.empty_since).map(|t| (t + EMPTY_FOR).saturating_duration_since(now)).min()
    }

    pub fn textures(&self) -> usize { self.classes.values().map(|c| c.pages.iter().flatten().count()).sum() }

    /// Sizes holding at least one page.
    pub fn sizes(&self) -> usize { self.classes.values().filter(|c| c.pages.iter().flatten().next().is_some()).count() }

    /// Pages made since the start.
    pub fn created(&self) -> u64 { self.created }

    #[cfg(test)]
    pub fn pages_of(&self, w: usize, h: usize) -> usize {
        self.classes.get(&(w as u16, h as u16)).map_or(0, |c| c.pages.iter().flatten().count())
    }

    /// Slots of this size, in all its pages.
    #[cfg(test)]
    pub fn capacity_of(&self, w: usize, h: usize) -> usize {
        self.classes.get(&(w as u16, h as u16)).map_or(0, Sized::capacity)
    }

    pub fn bytes(&self) -> usize {
        self.classes.iter().map(|((w, h), c)| c.pages.iter().flatten().map(|p| p.used.len() * (*h as usize + 1) * *w as usize * 4).sum::<usize>()).sum()
    }

    /// Slots in use.
    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.classes.values().flat_map(|c| c.pages.iter().flatten()).map(|p| p.used.iter().filter(|u| **u).count()).sum()
    }

    fn page_mut(&mut self, s: Strip) -> Option<&mut Page> {
        self.classes.get_mut(&(s.w, s.h))?.pages.get_mut(s.page as usize)?.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, v: u8) -> ColorImage { ColorImage::new([w, h], Color32::from_gray(v)) }

    /// A page stays in its 1 MiB block, padding included, and never has
    /// fewer than one slot.
    #[test]
    fn a_page_stays_under_its_memory_block() {
        for (w, h) in [(560, 88), (564, 76), (444, 76), (608, 88), (1216, 176), (300, 40)] {
            let n = max_slots(w, h);
            assert!(n >= 1);
            assert!(padded_bytes(w, n * (h + 1)) <= 1 << 20, "{w}x{h}: {n} slots");
        }
        assert_eq!(max_slots(4000, 3000), 1);
        assert!((4..=6).contains(&max_slots(560, 88)), "{}", max_slots(560, 88));
    }

    /// A size gets only the slots it asks for: one strip, one slot.
    #[test]
    fn a_page_has_only_the_slots_wanted() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let s = pool.alloc(&ctx, 564, 76, 1, 1).unwrap();
        assert_eq!(pool.capacity_of(564, 76), 1);
        assert_eq!(pool.bytes(), 564 * 77 * 4);
        assert!(pool.alloc(&ctx, 564, 76, 1, 1).is_none(), "full at one");
        // Wanting two more adds a page of two.
        assert!(pool.alloc(&ctx, 564, 76, 3, 1).is_some());
        assert_eq!(pool.capacity_of(564, 76), 3);
        assert_eq!(pool.pages_of(564, 76), 2);
        pool.write(s, img(564, 76, 9));
    }

    #[test]
    fn strips_never_overlap_and_uvs_point_to_their_slot() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let (w, h) = (560, 88);
        let mut strips = Vec::new();
        for _ in 0..40 {
            strips.push(pool.alloc(&ctx, w, h, 40, 1).expect("room"));
        }
        for (i, a) in strips.iter().enumerate() {
            for b in &strips[i + 1..] { assert_ne!(a, b); }
        }
        assert_eq!(pool.capacity_of(w, h), 40);
        assert_eq!(pool.pages_of(w, h), 40usize.div_ceil(max_slots(w, h)));
        let mut seen = Vec::new();
        for s in &strips {
            let (id, uv) = pool.image(*s).unwrap();
            assert!(uv.min.y >= 0.0 && uv.max.y <= 1.0 && uv.min.x == 0.0 && uv.max.x == 1.0);
            // (The same uv on another page is another picture.)
            assert!(!seen.iter().any(|(i, u): &(TextureId, Rect)| *i == id && u.intersects(uv)), "uv overlap");
            seen.push((id, uv));
        }
        assert_eq!(pool.used(), 40);
    }

    #[test]
    fn the_slot_count_is_capped_and_a_freed_slot_comes_back() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let all: Vec<Strip> = (0..4).map(|_| pool.alloc(&ctx, 560, 88, 4, 1).unwrap()).collect();
        assert!(pool.alloc(&ctx, 560, 88, 4, 1).is_none(), "four wanted, four there");
        pool.free(all[2], Instant::now());
        assert_eq!(pool.alloc(&ctx, 560, 88, 4, 1), Some(all[2]));
        pool.write(all[0], img(560, 88, 9));
    }

    #[test]
    fn an_empty_page_is_released_after_two_seconds_and_a_size_not_drawn_goes() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let t0 = Instant::now();
        let n = max_slots(300, 40);
        let a: Vec<Strip> = (0..n + 1).map(|_| pool.alloc(&ctx, 300, 40, n + 1, 1).unwrap()).collect();
        assert_eq!(pool.pages_of(300, 40), 2);
        // The second page empties while the size is still drawn.
        pool.free(*a.last().unwrap(), t0);
        pool.mark_drawn(300, 40, 2);
        pool.tidy(2, t0 + Duration::from_secs(1));
        assert_eq!(pool.pages_of(300, 40), 2, "empty for 1 s only");
        assert!(pool.next_release(t0 + Duration::from_secs(1)).is_some());
        pool.mark_drawn(300, 40, 3);
        pool.tidy(3, t0 + Duration::from_secs(3));
        assert_eq!(pool.pages_of(300, 40), 1);
        // The strips of page 0 still resolve after the hole.
        assert!(pool.image(a[0]).is_some());
        // Everything freed, the size no longer drawn: gone at once.
        for s in &a[..n] { pool.free(*s, t0 + Duration::from_secs(3)); }
        pool.tidy(10, t0 + Duration::from_secs(3));
        assert_eq!(pool.textures(), 0);
        assert_eq!(pool.bytes(), 0);
        assert!(pool.classes.is_empty());
    }

    /// The docker changed width: the old size's pages go back as soon as
    /// they are empty, not two seconds later.
    #[test]
    fn the_pages_of_a_size_no_longer_drawn_go_at_once() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let t0 = Instant::now();
        let old: Vec<Strip> = (0..3).map(|_| pool.alloc(&ctx, 512, 88, 3, 1).unwrap()).collect();
        let _new: Vec<Strip> = (0..3).map(|_| pool.alloc(&ctx, 608, 88, 3, 5).unwrap()).collect();
        for s in &old { pool.free(*s, t0); }
        pool.mark_drawn(608, 88, 5);
        pool.tidy(5, t0);
        assert_eq!(pool.pages_of(512, 88), 0);
        assert_eq!(pool.pages_of(608, 88), 1);
        assert_eq!(pool.created(), 2);
    }

    #[test]
    fn a_hole_left_by_a_released_page_is_reused_without_moving_other_strips() {
        let ctx = Context::default();
        let mut pool = StripPool::default();
        let t0 = Instant::now();
        let n = max_slots(300, 40);
        let a: Vec<Strip> = (0..2 * n).map(|_| pool.alloc(&ctx, 300, 40, 2 * n, 1).unwrap()).collect();
        for s in &a[..n] { pool.free(*s, t0); }
        pool.mark_drawn(300, 40, 2);
        pool.tidy(2, t0 + Duration::from_secs(5));
        assert_eq!(pool.pages_of(300, 40), 1);
        let again = pool.alloc(&ctx, 300, 40, 2 * n, 3).unwrap();
        assert_eq!(pool.pages_of(300, 40), 2);
        // Strips of the page that stayed still resolve to the same place.
        assert!(a[n..].iter().all(|s| pool.image(*s).is_some()));
        assert!(pool.image(again).is_some());
    }

    #[test]
    fn a_strip_too_big_for_any_texture_has_no_place() {
        assert!(!StripPool::fits(9000, 40));
        assert!(!StripPool::fits(0, 40));
        assert!(StripPool::fits(2000, 400));
    }
}

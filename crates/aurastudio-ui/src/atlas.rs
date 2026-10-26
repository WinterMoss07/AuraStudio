//! Texture atlases for the many small pictures of the interface (icons,
//! stroke previews, thumbnails). One egui texture per picture means one
//! GPU object, one memory block and one draw call each; here pictures
//! share pages, made when needed (the first 256 × 256, 256 KB; the next
//! ones 512 × 512, 1 MB), packed in rows ("shelves"); a freed region is
//! reused by the next picture of the same size. A picture too big for a
//! page is the caller's to keep on a texture of its own.

use egui::{pos2, Color32, ColorImage, Context, Rect, TextureHandle, TextureId, TextureOptions};

/// Side of the first page and of the following ones, pixels.
const FIRST_PAGE: usize = 256;
pub const PAGE: usize = 512;

/// Transparent pixels left around each picture (so smoothing never reads
/// its neighbour).
const GUTTER: usize = 1;

/// Where a picture is: page and pixels (w × h without the gutter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    page: u16,
    x:    u16,
    y:    u16,
    pub w: u16,
    pub h: u16,
}

struct Shelf { y: usize, h: usize, x: usize }

struct Page {
    side:   usize,
    tex:    TextureHandle,
    shelves: Vec<Shelf>,
    /// Freed regions, to give out again at their size.
    free:   Vec<Region>,
}

pub struct Atlas {
    name:      &'static str,
    options:   TextureOptions,
    pages:     Vec<Page>,
    max_pages: usize,
}

impl Atlas {
    /// `max_pages`: pages it may make (1 MB each); beyond it `alloc` says
    /// no and the caller frees something.
    pub fn new(name: &'static str, options: TextureOptions, max_pages: usize) -> Self {
        Self { name, options, pages: Vec::new(), max_pages }
    }

    /// Whether a `w` × `h` picture can ever be placed (else: its own texture).
    pub fn fits(w: usize, h: usize) -> bool {
        w > 0 && h > 0 && w + 2 * GUTTER <= PAGE && h + 2 * GUTTER <= PAGE
    }

    /// Room for a `w` × `h` picture, or None (too big for a page, or all
    /// the pages are full).
    pub fn alloc(&mut self, ctx: &Context, w: usize, h: usize) -> Option<Region> {
        if !Self::fits(w, h) { return None; }
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some(r) = p.take(i, w, h) { return Some(r); }
        }
        if self.pages.len() >= self.max_pages { return None; }
        // The first page is small; a picture that does not fit it, or a
        // later page, is a full one.
        let side = if self.pages.is_empty() && w + 2 * GUTTER <= FIRST_PAGE && h + 2 * GUTTER <= FIRST_PAGE { FIRST_PAGE } else { PAGE };
        let tex = ctx.load_texture(self.name, ColorImage::new([side, side], Color32::TRANSPARENT), self.options);
        self.pages.push(Page { side, tex, shelves: Vec::new(), free: Vec::new() });
        let i = self.pages.len() - 1;
        self.pages[i].take(i, w, h)
    }

    /// Put `image` (the size of the region) in its place.
    pub fn write(&mut self, r: Region, image: ColorImage) {
        debug_assert_eq!(image.size, [r.w as usize, r.h as usize]);
        if let Some(p) = self.pages.get_mut(r.page as usize) {
            p.tex.set_partial([r.x as usize, r.y as usize], image, self.options);
        }
    }

    /// The region is free again.
    pub fn free(&mut self, r: Region) {
        if let Some(p) = self.pages.get_mut(r.page as usize) { p.free.push(r); }
    }

    /// The page texture and the part of it the picture covers (0..1).
    pub fn image(&self, r: Region) -> (TextureId, Rect) {
        let page = &self.pages[r.page as usize];
        let s = page.side as f32;
        let uv = Rect::from_min_max(pos2(r.x as f32 / s, r.y as f32 / s), pos2((r.x + r.w) as f32 / s, (r.y + r.h) as f32 / s));
        (page.tex.id(), uv)
    }

    #[cfg(test)]
    pub fn pages(&self) -> usize { self.pages.len() }

    /// Bytes the pages hold on the GPU.
    #[cfg(test)]
    pub fn bytes(&self) -> usize { self.pages.iter().map(|p| p.side * p.side * 4).sum() }
}

impl Page {
    fn take(&mut self, page: usize, w: usize, h: usize) -> Option<Region> {
        let side = self.side;
        // A freed region of this very size.
        if let Some(i) = self.free.iter().position(|r| r.w as usize == w && r.h as usize == h) {
            return Some(self.free.swap_remove(i));
        }
        let (bw, bh) = (w + GUTTER, h + GUTTER);
        let region = |x: usize, y: usize| Region { page: page as u16, x: (x + GUTTER) as u16, y: (y + GUTTER) as u16, w: w as u16, h: h as u16 };
        // A shelf as high as the picture (up to a quarter more) with room.
        for s in &mut self.shelves {
            if s.h >= bh && s.h <= bh + bh / 4 && s.x + bw <= side {
                let r = region(s.x, s.y);
                s.x += bw;
                return Some(r);
            }
        }
        let y = self.shelves.last().map_or(0, |s| s.y + s.h);
        if y + bh + GUTTER <= side && bw <= side {
            self.shelves.push(Shelf { y, h: bh, x: bw });
            return Some(region(0, y));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlap(a: Region, b: Region) -> bool {
        a.page == b.page && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
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
            assert!((r.x as usize + r.w as usize) <= PAGE && (r.y as usize + r.h as usize) <= PAGE);
            let _ = at.image(r);
            for o in &rs { assert!(!overlap(r, *o), "{r:?} {o:?}"); }
            rs.push(r);
        }
        assert!(rs.len() > 100 && at.pages() > 1, "{} {}", rs.len(), at.pages());
        // Uv inside the page.
        let (_, uv) = at.image(rs[0]);
        assert!(uv.min.x >= 0.0 && uv.max.x <= 1.0 && uv.max.y <= 1.0);
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
        // One page (a 256 one first, then it is full): fills up, then no.
        let mut n = 2;
        while at.alloc(&ctx, 100, 50).is_some() { n += 1; assert!(n < 1000); }
        assert!(n >= 6 && at.pages() == 1);
    }

    #[test]
    fn a_small_use_costs_a_small_page_and_uvs_follow_the_page_size() {
        let ctx = Context::default();
        let mut at = Atlas::new("test", TextureOptions::LINEAR, 4);
        let a = at.alloc(&ctx, 20, 20).unwrap();
        assert_eq!(at.bytes(), 256 * 256 * 4);
        let (_, uv) = at.image(a);
        assert!((uv.max.x - (a.x as f32 + 20.0) / 256.0).abs() < 1e-6);
        // A picture that needs the big page gets one (the small one stays).
        let b = at.alloc(&ctx, 300, 40).unwrap();
        assert_eq!(at.bytes(), 256 * 256 * 4 + 512 * 512 * 4);
        let (_, uvb) = at.image(b);
        assert!((uvb.max.x - (b.x as f32 + 300.0) / 512.0).abs() < 1e-6);
        // Too wide for any page: the caller's own texture.
        assert!(!Atlas::fits(PAGE - 1, 10) && Atlas::fits(PAGE - 2, 10));
        assert!(at.alloc(&ctx, 600, 10).is_none());
    }
}

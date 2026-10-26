//! Texture atlases for the many small pictures of the interface (icons,
//! stroke previews, thumbnails). One egui texture per picture means one
//! GPU object, one memory block and one draw call each; here pictures
//! share pages of 512 × 512 pixels (1 MB each, made when needed), packed
//! in rows ("shelves"), and a freed region is reused by the next picture
//! of the same size.

use egui::{pos2, Color32, ColorImage, Context, Rect, TextureHandle, TextureId, TextureOptions};

/// Side of a page, pixels.
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

    /// Room for a `w` × `h` picture, or None (too big for a page, or all
    /// the pages are full).
    pub fn alloc(&mut self, ctx: &Context, w: usize, h: usize) -> Option<Region> {
        if w == 0 || h == 0 || w + 2 * GUTTER > PAGE || h + 2 * GUTTER > PAGE { return None; }
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some(r) = p.take(i, w, h) { return Some(r); }
        }
        if self.pages.len() >= self.max_pages { return None; }
        let tex = ctx.load_texture(self.name, ColorImage::new([PAGE, PAGE], Color32::TRANSPARENT), self.options);
        self.pages.push(Page { tex, shelves: Vec::new(), free: Vec::new() });
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
        let s = PAGE as f32;
        let uv = Rect::from_min_max(pos2(r.x as f32 / s, r.y as f32 / s), pos2((r.x + r.w) as f32 / s, (r.y + r.h) as f32 / s));
        (self.pages[r.page as usize].tex.id(), uv)
    }

    #[cfg(test)]
    pub fn pages(&self) -> usize { self.pages.len() }
}

impl Page {
    fn take(&mut self, page: usize, w: usize, h: usize) -> Option<Region> {
        // A freed region of this very size.
        if let Some(i) = self.free.iter().position(|r| r.w as usize == w && r.h as usize == h) {
            return Some(self.free.swap_remove(i));
        }
        let (bw, bh) = (w + GUTTER, h + GUTTER);
        let region = |x: usize, y: usize| Region { page: page as u16, x: (x + GUTTER) as u16, y: (y + GUTTER) as u16, w: w as u16, h: h as u16 };
        // A shelf as high as the picture (up to a quarter more) with room.
        for s in &mut self.shelves {
            if s.h >= bh && s.h <= bh + bh / 4 && s.x + bw <= PAGE {
                let r = region(s.x, s.y);
                s.x += bw;
                return Some(r);
            }
        }
        let y = self.shelves.last().map_or(0, |s| s.y + s.h);
        if y + bh + GUTTER <= PAGE && bw <= PAGE {
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
        // One page of 512: fills up, then no.
        let mut n = 2;
        while at.alloc(&ctx, 100, 50).is_some() { n += 1; assert!(n < 1000); }
        assert!(n > 20 && at.pages() == 1);
    }
}

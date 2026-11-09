//! What brush engines paint on: the door between the engine and the
//! pixels. The engine never knows how the pixels are stored: runs are cut
//! where the implementer wants (a tile edge for `raster`, nowhere for a
//! flat buffer), so tile size and layout stay private to the storage.
//! Generic over the pixel type (8-bit RGBA by default). Coordinates are
//! signed: the canvas is unbounded, the document's frame only says what
//! the document shows by default.

use crate::geom::Rect;
use crate::pixel::{Pixel, Rgba8};

/// Layer pixels, the pixels before the stroke (base) and, inside a
/// selection, the most coverage allowed (limit), by runs of one row.
///
/// The engine's rule: the engine keeps the stroke's coverage itself (the
/// storage keeps no copy of it); the coverage it computes for a pixel is
/// capped by `limit` (when there is one) and the pixel is made from `base`
/// with it; a pixel at coverage 0 keeps its base value exactly. So a
/// selection clips the stroke at the source; nothing is blended again
/// afterwards.
pub trait PaintSurface<P: Pixel = Rgba8> {
    /// The document's frame (a stroke may go past it: the storage keeps
    /// what is painted anywhere, unless it says otherwise by giving empty
    /// runs there).
    fn frame(&self) -> Rect;

    /// Called before the engine reads or writes `rect`: the storage makes
    /// what it needs there (tiles, the stroke base), only where the stroke
    /// goes.
    fn prepare(&mut self, rect: Rect);

    /// The run starting at (x, y), at most `max` pixels long. It may be
    /// shorter (the storage cuts it where it wants); its length is
    /// `pixels.len()`, never 0 for `max > 0` where the surface holds pixels
    /// (0: nothing to paint there). The engine asks again from `x + len`
    /// for the rest.
    fn span_mut(&mut self, x: i32, y: i32, max: usize) -> Span<'_, P>;

    /// Every run of `rects` (disjoint) at once, prepared here, in no
    /// particular order, pushed to `out`, for work on several cores; the
    /// runs never overlap. Runs where the limit is 0 everywhere may be left
    /// out (the engine could paint nothing there). `false`: not offered, go
    /// one by one through `span_mut`. `out` is the caller's buffer, kept
    /// between dabs (`recycle_spans`).
    fn spans_in<'a>(&'a mut self, _rects: &[Rect], _out: &mut Vec<Span<'a, P>>) -> bool { false }

    /// `spans_in` for one rect.
    fn spans_mut<'a>(&'a mut self, rect: Rect, out: &mut Vec<Span<'a, P>>) -> bool {
        self.spans_in(std::slice::from_ref(&rect), out)
    }

    /// The pixel at (x, y) as it was before the stroke (smudge and color
    /// sampling read the base around the dab).
    fn base_pixel(&self, x: i32, y: i32) -> P;
}

/// The allocation of a span buffer, emptied, for the next dab: no
/// allocation per dab. (Same layout whatever the lifetime, so the
/// collect reuses the buffer in place.)
pub fn recycle_spans<'b, P: Pixel>(mut v: Vec<Span<'_, P>>) -> Vec<Span<'b, P>> {
    v.clear();
    v.into_iter().map(|_| unreachable!()).collect()
}

/// A run of one row: from (x, y), its pixels, base and limit (one byte
/// per pixel, or none: no selection).
pub struct Span<'a, P: Pixel = Rgba8> {
    pub x:      i32,
    pub y:      i32,
    pub pixels: &'a mut [P],
    pub base:   &'a [P],
    pub limit:  Option<&'a [u8]>,
}

impl<P: Pixel> Span<'_, P> {
    /// An empty run (nothing to paint there).
    #[inline]
    pub fn empty(x: i32, y: i32) -> Self { Span { x, y, pixels: &mut [], base: &[], limit: None } }

    /// Pixels in the run.
    #[inline]
    pub fn len(&self) -> usize { self.pixels.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.pixels.is_empty() }

    /// The rule for one pixel: coverage `cov` capped by the limit (0: keep
    /// the base).
    #[inline(always)]
    pub fn cover(&self, i: usize, cov: u8) -> u8 {
        match self.limit { Some(l) => cov.min(l[i]), None => cov }
    }
}

/// A plain surface holding every pixel (tests, benchmarks, examples).
pub struct FlatSurface<P: Pixel = Rgba8> {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<P>,
    pub base:   Vec<P>,
    pub limit:  Option<Vec<u8>>,
    /// Longest run handed out (tests cut runs short to check that the
    /// engines ask again); `usize::MAX`: whole rows.
    pub run:    usize,
}

impl<P: Pixel> FlatSurface<P> {
    /// Every pixel `fill`, as before a stroke.
    pub fn new(width: u32, height: u32, fill: P) -> Self {
        let n = width as usize * height as usize;
        Self { width, height, pixels: vec![fill; n], base: vec![fill; n], limit: None, run: usize::MAX }
    }

    /// Start another stroke over what is painted now.
    pub fn next_stroke(&mut self) {
        self.base.copy_from_slice(&self.pixels);
    }

    pub fn pixel(&self, x: u32, y: u32) -> P { self.pixels[y as usize * self.width as usize + x as usize] }

    fn inside(&self, x: i32, y: i32) -> bool { x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height }
}

impl<P: Pixel> PaintSurface<P> for FlatSurface<P> {
    fn frame(&self) -> Rect { Rect::of_size(self.width, self.height) }
    fn prepare(&mut self, _rect: Rect) {}

    /// (A flat buffer holds nothing outside itself: empty runs there.)
    fn span_mut(&mut self, x: i32, y: i32, max: usize) -> Span<'_, P> {
        if !self.inside(x, y) { return Span::empty(x, y); }
        let len = max.min(self.run).min((self.width - x as u32) as usize);
        let i = y as usize * self.width as usize + x as usize;
        let limit = self.limit.as_deref().map(|l| &l[i..i + len]);
        Span { x, y, pixels: &mut self.pixels[i..i + len], base: &self.base[i..i + len], limit }
    }

    fn spans_in<'a>(&'a mut self, rects: &[Rect], out: &mut Vec<Span<'a, P>>) -> bool {
        let w = self.width as usize;
        let run = self.run.max(1);
        let limit = self.limit.as_deref();
        let base = &self.base[..];
        // Each row cut at the rects' edges (they are disjoint), so every
        // piece is borrowed once.
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        for (y, px) in self.pixels.chunks_exact_mut(w).enumerate() {
            cuts.clear();
            for r in rects {
                let Some((x0, y0, x1, y1)) = r.clipped_u32(self.width, self.height) else { continue };
                if (y as u32) >= y0 && (y as u32) < y1 { cuts.push((x0 as usize, x1 as usize)); }
            }
            if cuts.is_empty() { continue; }
            cuts.sort_unstable();
            let (mut rest, mut off) = (px, 0usize);
            for &(a, z) in &cuts {
                let (_, p) = std::mem::take(&mut rest).split_at_mut(a - off);
                let (mut seg, after) = p.split_at_mut(z - a);
                (rest, off) = (after, z);
                let mut x = a;
                while !seg.is_empty() {
                    let n = run.min(seg.len());
                    let (p1, p2) = std::mem::take(&mut seg).split_at_mut(n);
                    seg = p2;
                    let i = y * w + x;
                    out.push(Span { x: x as i32, y: y as i32, pixels: p1, base: &base[i..i + n], limit: limit.map(|l| &l[i..i + n]) });
                    x += n;
                }
            }
        }
        true
    }

    fn base_pixel(&self, x: i32, y: i32) -> P {
        if !self.inside(x, y) { return P::default(); }
        self.base[y as usize * self.width as usize + x as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_runs_cover_the_rect_once() {
        let mut s = FlatSurface::new(10, 4, [1u8, 2, 3, 4]);
        s.run = 3;
        let mut spans = Vec::new();
        assert!(s.spans_mut(Rect::new(-2, 1, 8, 3), &mut spans));
        let total: usize = spans.iter().map(|sp| sp.len()).sum();
        assert_eq!(total, 8 * 2);
        assert!(spans.iter().all(|sp| sp.len() <= 3 && sp.pixels.len() == sp.len()));
        drop(spans);
        let sp = s.span_mut(8, 0, 100);
        assert_eq!(sp.len(), 2);
    }

    #[test]
    fn several_rects_are_cut_once() {
        let mut s = FlatSurface::new(10, 4, [0u8; 4]);
        s.run = 4;
        let mut spans = Vec::new();
        assert!(s.spans_in(&[Rect::new(0, 0, 3, 2), Rect::new(5, 1, 10, 4)], &mut spans));
        let total: usize = spans.iter().map(|sp| sp.len()).sum();
        assert_eq!(total, 3 * 2 + 5 * 3);
        for sp in &mut spans { sp.pixels.fill([1; 4]); }
        drop(spans);
        assert_eq!(s.pixel(2, 1), [1; 4]);
        assert_eq!(s.pixel(4, 1), [0; 4]);
        assert_eq!(s.pixel(9, 3), [1; 4]);
    }

    #[test]
    fn the_limit_caps_coverage() {
        let mut s = FlatSurface::new(4, 1, 0u8);
        s.limit = Some(vec![255, 100, 0, 255]);
        let sp = s.span_mut(0, 0, 4);
        let got: Vec<u8> = (0..4).map(|i| sp.cover(i, 200)).collect();
        assert_eq!(got, vec![200, 100, 0, 200]);
    }

    #[test]
    fn recycled_buffer_keeps_its_allocation() {
        let mut s = FlatSurface::new(64, 64, [0u8; 4]);
        let mut buf: Vec<Span<'static>> = Vec::with_capacity(256);
        let cap = buf.capacity();
        for _ in 0..3 {
            let mut v = recycle_spans(buf);
            s.spans_mut(Rect::new(0, 0, 64, 64), &mut v);
            assert_eq!(v.len(), 64);
            buf = recycle_spans(v);
            assert!(buf.is_empty() && buf.capacity() == cap);
        }
    }
}

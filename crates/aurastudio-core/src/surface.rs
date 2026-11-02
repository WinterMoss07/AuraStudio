//! What brush engines paint on: the door between the engine and the
//! pixels. The engine never knows how the pixels are stored: runs are cut
//! where the implementer wants (a tile edge for `raster`, nowhere for a
//! flat buffer), so tile size and layout stay private to the storage.
//! Generic over the pixel type (8-bit RGBA by default).

use crate::geom::Rect;
use crate::pixel::{Pixel, Rgba8};

/// Layer pixels, the pixels before the stroke (base), the stroke's
/// coverage (mask, one byte per pixel) and, inside a selection, the most
/// coverage allowed (limit), by runs of one row.
///
/// The engine's rule: the coverage it computes for a pixel is first capped
/// by `limit` (when there is one), then kept as the max with `mask`, and
/// the pixel is made from `base` with that mask; a pixel whose mask is
/// still 0 keeps its base value exactly. So a selection clips the stroke
/// at the source; nothing is blended again afterwards.
pub trait PaintSurface<P: Pixel = Rgba8> {
    fn width(&self) -> u32;
    fn height(&self) -> u32;

    /// Called before the engine reads or writes `rect`: the storage makes
    /// what it needs there (tiles, the stroke base), only where the stroke
    /// goes.
    fn prepare(&mut self, rect: Rect);

    /// The run starting at (x, y), at most `max` pixels long. It may be
    /// shorter (the storage cuts it where it wants); its length is
    /// `mask.len()`, never 0 for `max > 0` inside the surface. The engine
    /// asks again from `x + len` for the rest.
    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_, P>;

    /// Every run of `rect` at once (prepared, in no particular order),
    /// pushed to `out`, for work on several cores; the runs never
    /// overlap. Runs where the limit is 0 everywhere may be left out (the
    /// engine could paint nothing there). `false`: not offered, go one by
    /// one through `span_mut`. `out` is the caller's buffer, kept between
    /// dabs (`recycle_spans`).
    fn spans_mut<'a>(&'a mut self, _rect: Rect, _out: &mut Vec<Span<'a, P>>) -> bool { false }

    /// The pixel at (x, y) as it was before the stroke (smudge and color
    /// sampling read the base around the dab).
    fn base_pixel(&self, x: u32, y: u32) -> P;
}

/// The allocation of a span buffer, emptied, for the next dab: no
/// allocation per dab. (Same layout whatever the lifetime, so the
/// collect reuses the buffer in place.)
pub fn recycle_spans<'b, P: Pixel>(mut v: Vec<Span<'_, P>>) -> Vec<Span<'b, P>> {
    v.clear();
    v.into_iter().map(|_| unreachable!()).collect()
}

/// A run of one row: from (x, y), its pixels, base, mask (one byte per
/// pixel) and limit (one byte per pixel, or none: no selection).
pub struct Span<'a, P: Pixel = Rgba8> {
    pub x:      u32,
    pub y:      u32,
    pub pixels: &'a mut [P],
    pub base:   &'a [P],
    pub mask:   &'a mut [u8],
    pub limit:  Option<&'a [u8]>,
}

impl<P: Pixel> Span<'_, P> {
    /// Pixels in the run.
    #[inline]
    pub fn len(&self) -> usize { self.mask.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.mask.is_empty() }

    /// The rule for one pixel: coverage `cov` capped by the limit, max
    /// with the mask; returns the mask now (0: keep the base).
    #[inline(always)]
    pub fn cover(&mut self, i: usize, cov: u8) -> u8 {
        let c = match self.limit { Some(l) => cov.min(l[i]), None => cov };
        let m = self.mask[i].max(c);
        self.mask[i] = m;
        m
    }
}

/// A plain surface holding every pixel (tests, benchmarks, examples).
pub struct FlatSurface<P: Pixel = Rgba8> {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<P>,
    pub base:   Vec<P>,
    pub mask:   Vec<u8>,
    pub limit:  Option<Vec<u8>>,
    /// Longest run handed out (tests cut runs short to check that the
    /// engines ask again); `usize::MAX`: whole rows.
    pub run:    usize,
}

impl<P: Pixel> FlatSurface<P> {
    /// Every pixel `fill`, as before a stroke.
    pub fn new(width: u32, height: u32, fill: P) -> Self {
        let n = width as usize * height as usize;
        Self { width, height, pixels: vec![fill; n], base: vec![fill; n], mask: vec![0; n], limit: None, run: usize::MAX }
    }

    /// Start another stroke over what is painted now.
    pub fn next_stroke(&mut self) {
        self.base.copy_from_slice(&self.pixels);
        self.mask.fill(0);
    }

    pub fn pixel(&self, x: u32, y: u32) -> P { self.pixels[y as usize * self.width as usize + x as usize] }
}

impl<P: Pixel> PaintSurface<P> for FlatSurface<P> {
    fn width(&self) -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn prepare(&mut self, _rect: Rect) {}

    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_, P> {
        let len = max.min(self.run).min(self.width.saturating_sub(x) as usize);
        let i = y as usize * self.width as usize + x as usize;
        let limit = self.limit.as_deref().map(|l| &l[i..i + len]);
        Span { x, y, pixels: &mut self.pixels[i..i + len], base: &self.base[i..i + len], mask: &mut self.mask[i..i + len], limit }
    }

    fn spans_mut<'a>(&'a mut self, r: Rect, out: &mut Vec<Span<'a, P>>) -> bool {
        let Some((x0, y0, x1, y1)) = r.clipped_u32(self.width, self.height) else { return true };
        let w = self.width as usize;
        let run = self.run.max(1);
        let limit = self.limit.as_deref();
        let rows = self.pixels.chunks_exact_mut(w).zip(self.base.chunks_exact(w)).zip(self.mask.chunks_exact_mut(w));
        for (y, ((px, base), mask)) in rows.enumerate().skip(y0 as usize).take((y1 - y0) as usize) {
            let (a, z) = (x0 as usize, x1 as usize);
            let (mut px, mut base, mut mask) = (&mut px[a..z], &base[a..z], &mut mask[a..z]);
            let mut lim = limit.map(|l| &l[y * w + a..y * w + z]);
            let mut x = x0;
            while !mask.is_empty() {
                let n = run.min(mask.len());
                let (p, rp) = px.split_at_mut(n);
                let (b, rb) = base.split_at(n);
                let (m, rm) = mask.split_at_mut(n);
                let (l, rl) = match lim { Some(l) => { let (a, b) = l.split_at(n); (Some(a), Some(b)) } None => (None, None) };
                out.push(Span { x, y: y as u32, pixels: p, base: b, mask: m, limit: l });
                (px, base, mask, lim) = (rp, rb, rm, rl);
                x += n as u32;
            }
        }
        true
    }

    fn base_pixel(&self, x: u32, y: u32) -> P { self.base[y as usize * self.width as usize + x as usize] }
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
    fn the_limit_caps_coverage() {
        let mut s = FlatSurface::new(4, 1, 0u8);
        s.limit = Some(vec![255, 100, 0, 255]);
        let mut sp = s.span_mut(0, 0, 4);
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

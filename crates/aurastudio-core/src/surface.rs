//! What brush engines paint on: the door between `forge` and the pixels.
//! The engine never knows how the pixels are stored: runs are cut where
//! the implementer wants (a tile edge for `raster`, nowhere for a flat
//! buffer), so tile size and layout stay private to the storage.
//! Pixels are straight RGBA8 for now; the generic pixel type comes with
//! the other pixel depths.

use crate::geom::Rect;

/// Layer pixels, the pixels before the stroke (base), the stroke's
/// coverage (mask, one byte per pixel) and, inside a selection, the most
/// coverage allowed (limit), by runs of one row.
///
/// The engine's rule: the coverage it computes for a pixel is first capped
/// by `limit` (when there is one), then kept as the max with `mask`, and
/// the pixel is made from `base` with that mask; a pixel whose mask is
/// still 0 keeps its base value exactly. So a selection clips the stroke
/// at the source; nothing is blended again afterwards.
pub trait PaintSurface {
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
    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_>;

    /// Every run of `rect` at once (prepared, in no particular order),
    /// pushed to `out`, for work on several cores; the runs never
    /// overlap. `false`: not offered, go one by one through `span_mut`.
    /// `out` is the caller's buffer, kept between dabs (`recycle_spans`).
    fn spans_mut<'a>(&'a mut self, _rect: Rect, _out: &mut Vec<Span<'a>>) -> bool { false }
}

/// The allocation of a span buffer, emptied, for the next dab: no
/// allocation per dab. (Same layout whatever the lifetime, so the
/// collect reuses the buffer in place.)
pub fn recycle_spans<'b>(mut v: Vec<Span<'_>>) -> Vec<Span<'b>> {
    v.clear();
    v.into_iter().map(|_| unreachable!()).collect()
}

/// A run of one row: from (x, y), its pixels (RGBA8 bytes), base (RGBA8
/// bytes), mask (one byte per pixel) and limit (one byte per pixel, or
/// none: no selection).
pub struct Span<'a> {
    pub x:      u32,
    pub y:      u32,
    pub pixels: &'a mut [u8],
    pub base:   &'a [u8],
    pub mask:   &'a mut [u8],
    pub limit:  Option<&'a [u8]>,
}

impl Span<'_> {
    /// Pixels in the run.
    #[inline]
    pub fn len(&self) -> usize { self.mask.len() }

    #[inline]
    pub fn is_empty(&self) -> bool { self.mask.is_empty() }
}

/// A plain surface holding every pixel (tests, benchmarks, examples).
pub struct FlatSurface {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub base:   Vec<u8>,
    pub mask:   Vec<u8>,
    /// Longest run handed out (tests cut runs short to check that the
    /// engines ask again); `usize::MAX`: whole rows.
    pub run:    usize,
}

impl FlatSurface {
    /// Every pixel `rgba`, as before a stroke.
    pub fn new(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let pixels = rgba.repeat(width as usize * height as usize);
        Self { width, height, base: pixels.clone(), pixels, mask: vec![0; width as usize * height as usize], run: usize::MAX }
    }

    /// Start another stroke over what is painted now.
    pub fn next_stroke(&mut self) {
        self.base.copy_from_slice(&self.pixels);
        self.mask.fill(0);
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }
}

impl PaintSurface for FlatSurface {
    fn width(&self) -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn prepare(&mut self, _rect: Rect) {}

    fn span_mut(&mut self, x: u32, y: u32, max: usize) -> Span<'_> {
        let len = max.min(self.run).min(self.width.saturating_sub(x) as usize);
        let i = y as usize * self.width as usize + x as usize;
        Span { x, y, pixels: &mut self.pixels[i * 4..(i + len) * 4], base: &self.base[i * 4..(i + len) * 4], mask: &mut self.mask[i..i + len], limit: None }
    }

    fn spans_mut<'a>(&'a mut self, r: Rect, out: &mut Vec<Span<'a>>) -> bool {
        let Some((x0, y0, x1, y1)) = r.clipped_u32(self.width, self.height) else { return true };
        let w = self.width as usize;
        let run = self.run.max(1);
        let rows = self.pixels.chunks_exact_mut(w * 4).zip(self.base.chunks_exact(w * 4)).zip(self.mask.chunks_exact_mut(w));
        for (y, ((px, base), mask)) in rows.enumerate().skip(y0 as usize).take((y1 - y0) as usize) {
            let (mut px, mut base, mut mask) = (&mut px[x0 as usize * 4..x1 as usize * 4], &base[x0 as usize * 4..x1 as usize * 4], &mut mask[x0 as usize..x1 as usize]);
            let mut x = x0;
            while !mask.is_empty() {
                let n = run.min(mask.len());
                let (p, rp) = px.split_at_mut(n * 4);
                let (b, rb) = base.split_at(n * 4);
                let (m, rm) = mask.split_at_mut(n);
                out.push(Span { x, y: y as u32, pixels: p, base: b, mask: m, limit: None });
                (px, base, mask) = (rp, rb, rm);
                x += n as u32;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_runs_cover_the_rect_once() {
        let mut s = FlatSurface::new(10, 4, [1, 2, 3, 4]);
        s.run = 3;
        let mut spans = Vec::new();
        assert!(s.spans_mut(Rect::new(-2, 1, 8, 3), &mut spans));
        let total: usize = spans.iter().map(|sp| sp.mask.len()).sum();
        assert_eq!(total, 8 * 2);
        assert!(spans.iter().all(|sp| sp.mask.len() <= 3 && sp.pixels.len() == sp.mask.len() * 4));
        drop(spans);
        let sp = s.span_mut(8, 0, 100);
        assert_eq!((sp.pixels.len(), sp.len()), (4 * 2, 2));
    }

    #[test]
    fn recycled_buffer_keeps_its_allocation() {
        let mut s = FlatSurface::new(64, 64, [0; 4]);
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

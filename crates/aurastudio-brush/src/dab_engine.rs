//! Round dabs with "wash" accumulation (Krita's alpha darken), native.
//!
//! The stroke is a row of round dabs placed along the pen path. Each dab
//! moves the pixels it covers toward its opacity (brush opacity × pressure)
//! by the dab's shape value, never past it:
//!
//!   if mask < cap: mask += (cap - mask) × shape
//!
//! so a stroke never gets more opaque than its strongest dab (crossing a
//! 20 % stroke at 30 % gives 30 %), while overlapping soft edges add up
//! smoothly. That is what gives this engine its soft, even gradients and
//! clean crossings (a per-pixel max leaves a visible crease where two soft
//! parts of a stroke cross).
//!
//! It is also what keeps big brushes cheap: like Krita's auto spacing, the
//! gap between dabs grows with the brush (but only as √radius past a point,
//! so a hard edge never scallops by more than half a pixel), so a 1000 px
//! brush paints a dab every ~45 px of pen travel, not on every pen sample.
//!
//! The dab shape is the round pen's: opaque out to `radius × hardness`,
//! then a smoothstep fade to the edge.

use aurastudio_canvas::{blend::unit_to_u8, PaintSurface, Rect};

use crate::raster::{changed_span, min_u8, write_pixels, CHUNK, IOTA};

use crate::{Brush, PressureSmoother};

pub struct DabEngine {
    brush:   Brush,
    /// 1 / width of the fading edge, if wider than the 1 px anti-aliasing.
    smooth_edge: Option<f32>,
    /// Outer radius of the dab (brush radius + half a pixel).
    reach:   f32,
    /// Distance between two dabs, in pixels.
    spacing: f32,
    pub smoother: PressureSmoother,
    /// Last pen position and smoothed pressure.
    last:    Option<(f32, f32, f32)>,
    /// Pen travel left before the next dab.
    to_next: f32,
}

impl DabEngine {
    pub fn new() -> Self {
        let mut e = Self {
            brush: Brush::new(), smooth_edge: None, reach: 0.0, spacing: 1.0,
            smoother: PressureSmoother::new(0.5), last: None, to_next: 0.0,
        };
        e.sync_from(&Brush::new());
        e
    }

    /// Take size, opacity, hardness, color and blend mode from the brush,
    /// and build the dab shape for them.
    pub fn sync_from(&mut self, b: &Brush) {
        self.brush = b.clone();
        let r = b.radius.max(1) as f32;
        self.reach = r + 0.5;
        // 6 dabs per radius (what the hokusai engine used), but no more
        // than 2·√r apart: the edge of a hard brush then ripples by at
        // most s² / 8r = 0.5 px.
        self.spacing = (r / 6.0).min(2.0 * r.sqrt()).max(0.5);

        // Same shape as the round pen: full opacity out to `radius ×
        // hardness`, then a smoothstep fade to the edge (at least one pixel
        // wide: anti-aliasing).
        let soft = (r * (1.0 - b.hardness.clamp(0.0, 1.0))).max(1.0);
        self.smooth_edge = (soft > 1.0).then_some(1.0 / soft);
    }

    pub fn begin_stroke(&mut self) {
        self.last = None;
        self.to_next = 0.0;
        self.smoother.reset();
    }

    /// One pen sample (canvas pixels). Places the dabs due between the
    /// previous sample and this one. Returns the bounds of what changed.
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        let p = self.smoother.apply(x, y, pressure, self.brush.radius as f32);
        let mut dirty = None;
        let Some((lx, ly, lp)) = self.last else {
            // First sample: a dab right under the pen.
            self.last = Some((x, y, p));
            self.to_next = self.spacing;
            merge(&mut dirty, self.dab(surface, x, y, p));
            return dirty;
        };
        let (dx, dy) = (x - lx, y - ly);
        let len = (dx * dx + dy * dy).sqrt();
        let mut done = 0.0;
        while len - done >= self.to_next {
            done += self.to_next;
            let t = done / len;
            merge(&mut dirty, self.dab(surface, lx + dx * t, ly + dy * t, lp + (p - lp) * t));
            self.to_next = self.spacing;
        }
        self.to_next -= len - done;
        self.last = Some((x, y, p));
        dirty
    }

    /// Pen lifted: one last dab where it left, so the stroke reaches the
    /// end point (up to a spacing short otherwise).
    pub fn end_stroke<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        let (x, y, p) = self.last.take()?;
        if self.to_next >= self.spacing { return None; }
        self.dab(surface, x, y, p)
    }

    /// One dab, wash-accumulated into the stroke mask, and the layer pixels
    /// it changed recomposited from the layer as it was before the stroke.
    fn dab<S: PaintSurface>(&self, surface: &mut S, cx: f32, cy: f32, pressure: f32) -> Option<Rect> {
        let cap = unit_to_u8(self.brush.opacity * pressure);
        if cap == 0 { return None; }
        let (w, h) = (surface.width() as i32, surface.height() as i32);
        let reach = self.reach;
        let y0 = ((cy - reach).floor() as i32).max(0);
        let y1 = ((cy + reach).ceil() as i32 + 1).min(h);
        if y0 >= y1 { return None; }
        let x0 = ((cx - reach).floor() as i32).max(0);
        let x1 = ((cx + reach).ceil() as i32 + 1).min(w);
        if x0 >= x1 { return None; }
        surface.prepare((x0 as u32, y0 as u32, x1 as u32, y1 as u32));
        let sw = w as usize;
        let (pixels, base, mask) = surface.surfaces_mut();
        let (color, blend) = (self.brush.color, self.brush.blend);
        let (mut wx0, mut wy0, mut wx1, mut wy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);

        for py in y0..y1 {
            let dy = py as f32 + 0.5 - cy;
            let half2 = reach * reach - dy * dy;
            if half2 <= 0.0 { continue; }
            let half = half2.sqrt();
            let xa = ((cx - half).floor() as i32).max(0);
            let xb = ((cx + half).ceil() as i32 + 1).min(w);
            if xa >= xb { continue; }
            let row = py as usize * sw;
            let dy2 = dy * dy;
            let (mut row_x0, mut row_x1) = (i32::MAX, i32::MIN);
            // In chunks: first the new coverage of the whole chunk (plain
            // arithmetic the compiler vectorizes), then the layer pixels
            // only where it changed.
            let mut x = xa;
            while x < xb {
                let n = ((xb - x) as usize).min(CHUNK);
                let i0 = row + x as usize;
                let mrow = &mut mask[i0..i0 + n];
                // Already at this dab's opacity (the middle of a stroke at
                // steady pressure): nothing to do.
                if min_u8(mrow) >= cap {
                    x += n as i32;
                    continue;
                }
                // Dab shape over the run, 0..=256 (float math, vectorized)...
                let mut shape = [0u16; CHUNK];
                let shape = &mut shape[..n];
                let dx0 = x as f32 + 0.5 - cx;
                if let Some(inv_soft) = self.smooth_edge {
                    // Soft: smoothstep of the distance into the edge.
                    for (v, &j) in shape.iter_mut().zip(IOTA.iter()) {
                        let dx = dx0 + j;
                        let t = ((reach - (dx * dx + dy2).sqrt()) * inv_soft).clamp(0.0, 1.0);
                        *v = (t * t * (3.0 - 2.0 * t) * 256.0 + 0.5) as i32 as u16;
                    }
                } else {
                    // Hard: 1 px anti-aliased edge.
                    for (v, &j) in shape.iter_mut().zip(IOTA.iter()) {
                        let dx = dx0 + j;
                        let t = (reach - (dx * dx + dy2).sqrt()).clamp(0.0, 1.0);
                        *v = (t * 256.0 + 0.5) as i32 as u16;
                    }
                }
                // ...then the wash step toward the cap (integer, vectorized).
                let mut new = [0u8; CHUNK];
                let new = &mut new[..n];
                for ((nv, &cur), &sh) in new.iter_mut().zip(mrow.iter()).zip(shape.iter()) {
                    let gap = cap.saturating_sub(cur) as u16;
                    *nv = cur + ((gap * sh + 128) >> 8) as u8;
                }
                let prow = &mut pixels[i0 * 4..(i0 + n) * 4];
                let brow = &base[i0 * 4..(i0 + n) * 4];
                let Some((c0, c1)) = changed_span(mrow, new) else {
                    x += n as i32;
                    continue;
                };
                mrow[c0..c1].copy_from_slice(&new[c0..c1]);
                write_pixels(&mut prow[c0 * 4..c1 * 4], &brow[c0 * 4..c1 * 4], &new[c0..c1], color, blend);
                row_x0 = row_x0.min(x + c0 as i32);
                row_x1 = row_x1.max(x + c1 as i32);
                x += n as i32;
            }
            if row_x0 < row_x1 {
                wx0 = wx0.min(row_x0);
                wx1 = wx1.max(row_x1);
                wy0 = wy0.min(py);
                wy1 = wy1.max(py + 1);
            }
        }
        (wx0 < wx1).then_some((wx0 as u32, wy0 as u32, wx1 as u32, wy1 as u32))
    }
}

impl Default for DabEngine {
    fn default() -> Self { Self::new() }
}

fn merge(acc: &mut Option<Rect>, r: Option<Rect>) {
    let Some(r) = r else { return };
    *acc = Some(match *acc {
        None    => r,
        Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: u32 = 256;

    struct T { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for T {
        fn width(&self) -> u32 { N }
        fn height(&self) -> u32 { N }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    fn white() -> T {
        let px = vec![255u8; (N * N * 4) as usize];
        T { px: px.clone(), base: px, mask: vec![0; (N * N) as usize] }
    }

    fn red_at(s: &T, x: u32, y: u32) -> u8 { s.px[((y * N + x) * 4) as usize] }

    fn engine(radius: u32, hardness: f32) -> DabEngine {
        let mut b = Brush::new();
        b.radius = radius;
        b.hardness = hardness;
        let mut e = DabEngine::new();
        e.smoother.span = 0.0;
        e.sync_from(&b);
        e.begin_stroke();
        e
    }

    fn line(e: &mut DabEngine, s: &mut T, from: f32, to: f32, y: f32, p: f32) {
        let steps = 60;
        for i in 0..=steps {
            let x = from + (to - from) * i as f32 / steps as f32;
            e.stroke_to(s, x, y, p);
        }
    }

    #[test]
    fn pressure_gives_matching_gray() {
        // 20 % pressure, black: the middle of the stroke is 20 % gray (204).
        let mut s = white();
        let mut e = engine(10, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 204).abs() <= 2, "expected ~204, got {v}");
    }

    #[test]
    fn crossing_the_same_stroke_does_not_build_up() {
        let mut s = white();
        let mut e = engine(10, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        line(&mut e, &mut s, 200.0, 40.0, 128.0, 0.3);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 178).abs() <= 2, "expected ~178 (30 %), got {v}");
    }

    #[test]
    fn stroke_reaches_the_end_point() {
        let mut s = white();
        let mut e = engine(40, 0.9);
        line(&mut e, &mut s, 40.0, 197.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        assert!(red_at(&s, 197, 128) < 10, "end not painted: {}", red_at(&s, 197, 128));
    }

    #[test]
    fn soft_crossing_has_no_crease() {
        // Two soft strokes crossing at a right angle: along the diagonal
        // through the crossing, values must change smoothly (the max of two
        // gradients makes a kink there, seen as a light line).
        let mut s = white();
        let mut e = engine(40, 0.0);
        line(&mut e, &mut s, 20.0, 236.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        e.begin_stroke();
        for i in 0..=60 {
            e.stroke_to(&mut s, 128.0, 20.0 + 216.0 * i as f32 / 60.0, 1.0);
        }
        e.end_stroke(&mut s);
        // Along a row in the soft edge of the horizontal stroke, crossing
        // the vertical one: a per-pixel max keeps the row flat, then bends
        // sharply where the vertical stroke takes over (a crease, seen as a
        // light line). Wash accumulation bends smoothly.
        let v: Vec<i32> = (0..60).map(|k| 255 - red_at(&s, 128 + k, 160) as i32).collect();
        let worst = v.windows(3).map(|w| (w[0] - 2 * w[1] + w[2]).abs()).max().unwrap();
        assert!(worst <= 8, "kink along the row: {worst} ({v:?})");
    }

    #[test]
    fn big_brush_paints_few_dabs() {
        let mut e = engine(500, 0.5);
        assert!(e.spacing > 40.0, "spacing {}", e.spacing);
        e.begin_stroke();
    }
}

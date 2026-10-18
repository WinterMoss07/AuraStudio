//! The brush engine: dabs placed along the pen path, with Krita's "wash"
//! accumulation (alpha darken). Every brush that is not a MyPaint brush
//! goes through it; the round brush is one preset of it, and each brush
//! keeps its own settings (Tool options).
//!
//! Each dab moves the pixels it covers toward its opacity (brush opacity ×
//! pressure) by the dab's shape value, never past it:
//!
//!   if mask < cap: mask += (cap - mask) × shape
//!
//! so a stroke never gets more opaque than its strongest dab (crossing a
//! 20 % stroke at 30 % gives 30 %), while overlapping soft edges add up
//! smoothly (clean crossings: a per-pixel max leaves a crease).
//!
//! When the pressure falls, Krita's averaged opacity (KisPainter::
//! blendAverageOpacity + the "creamy" alpha darken): the stroke aims at an
//! average that follows a rise at once but decays by 10 % of the gap per
//! dab, and pixels already painted keep being pulled toward it. Without
//! it, the front of every stronger dab stayed visible as an arc ("beads")
//! wherever the pressure went down.
//!
//! Spacing as Krita's auto spacing: 0.8 × √diameter (Krita's Basic presets
//! use 0.8). The straight edge of a hard stroke then ripples by
//! s² / 8r = 0.16 px whatever the size, and a 1000 px brush paints a dab
//! every ~25 px of pen travel, not on every pen sample.
//!
//! Dab shape: opaque out to `radius × hardness`, then a smoothstep fade to
//! the edge (1 px of anti-aliasing at 100 %).

use aurastudio_canvas::{blend::unit_to_u8, tile_run, PaintSurface, Rect};

use crate::raster::{changed_span, erase_pixels, min_u8, write_pixels, CHUNK, IOTA};

use crate::Brush;

pub struct DabEngine {
    brush:   Brush,
    /// 1 / width of the fading edge, if wider than the 1 px anti-aliasing.
    smooth_edge: Option<f32>,
    /// Outer radius of the dab (brush radius + half a pixel).
    reach:   f32,
    /// Distance between two dabs, in pixels.
    spacing: f32,
    /// Last pen position and pressure.
    last:    Option<(f32, f32, f32)>,
    /// Pen travel left before the next dab.
    to_next: f32,
    /// Averaged opacity of the stroke (mask units), see the module doc.
    average: f32,
}

impl DabEngine {
    pub fn new() -> Self {
        let mut e = Self {
            brush: Brush::new(), smooth_edge: None, reach: 0.0, spacing: 1.0,
            last: None, to_next: 0.0, average: 0.0,
        };
        e.sync_from(&Brush::new());
        e
    }

    /// Take size, opacity, hardness, color and blend mode from the brush,
    /// and build the dab shape for them.
    pub fn sync_from(&mut self, b: &Brush) {
        self.brush = b.clone();
        let r = b.radius.max(0.5);
        self.reach = r + 0.5;
        // Krita's auto spacing, coefficient 0.8 (see the module doc).
        self.spacing = (0.8 * (2.0 * r).sqrt()).max(0.5);

        // Full opacity out to `radius × hardness`, then a smoothstep fade
        // to the edge (at least one pixel wide: anti-aliasing).
        let soft = (r * (1.0 - b.hardness.clamp(0.0, 1.0))).max(1.0);
        self.smooth_edge = (soft > 1.0).then_some(1.0 / soft);
    }

    pub fn begin_stroke(&mut self) {
        self.last = None;
        self.to_next = 0.0;
        self.average = 0.0;
    }

    /// One pen sample (canvas pixels). Places the dabs due between the
    /// previous sample and this one. Returns the bounds of what changed.
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        let p = pressure;
        let mut dirty = None;
        let Some((lx, ly, lp)) = self.last else {
            // First sample: a dab right under the pen.
            self.last = Some((x, y, p));
            self.to_next = self.spacing;
            merge(&mut dirty, self.dab(surface, x, y, p));
            return dirty;
        };
        // In f64: far off the canvas (zoomed out), f32 steps of one
        // spacing could stop advancing a huge distance, and never end.
        let (dx, dy) = ((x - lx) as f64, (y - ly) as f64);
        let len = (dx * dx + dy * dy).sqrt();
        let mut done = 0.0f64;
        while len - done >= self.to_next as f64 {
            done += self.to_next as f64;
            let t = done / len;
            let (px, py) = (lx as f64 + dx * t, ly as f64 + dy * t);
            merge(&mut dirty, self.dab(surface, px as f32, py as f32, lp + (p - lp) * t as f32));
            self.to_next = self.spacing;
        }
        self.to_next -= (len - done) as f32;
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
    fn dab<S: PaintSurface>(&mut self, surface: &mut S, cx: f32, cy: f32, pressure: f32) -> Option<Rect> {
        let opacity = (self.brush.opacity * pressure).clamp(0.0, 1.0) * 255.0;
        // Krita's averaged opacity: up at once, down by 10 % per dab.
        self.average = if self.average < opacity { opacity } else { 0.1 * opacity + 0.9 * self.average };
        let cap = unit_to_u8(opacity / 255.0);
        let average = unit_to_u8(self.average / 255.0);
        // Pressure falling: pull toward the average (see the module doc).
        let falling = average > cap;
        let target = cap.max(average);
        if target == 0 { return None; }
        let (capf, avgf) = (cap as f32, average as f32);
        let inv_avg = 1.0 / avgf.max(1.0);
        let (w, h) = (surface.width() as i32, surface.height() as i32);
        let reach = self.reach;
        let y0 = ((cy - reach).floor() as i32).max(0);
        let y1 = ((cy + reach).ceil() as i32 + 1).min(h);
        if y0 >= y1 { return None; }
        let x0 = ((cx - reach).floor() as i32).max(0);
        let x1 = ((cx + reach).ceil() as i32 + 1).min(w);
        if x0 >= x1 { return None; }
        surface.prepare((x0 as u32, y0 as u32, x1 as u32, y1 as u32));
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
            let dy2 = dy * dy;
            let (mut row_x0, mut row_x1) = (i32::MAX, i32::MIN);
            // In chunks (never across a layer tile): first the new
            // coverage of the whole chunk (plain arithmetic the compiler
            // vectorizes), then the layer pixels only where it changed.
            let mut x = xa;
            while x < xb {
                let n = tile_run(x as u32, ((xb - x) as usize).min(CHUNK));
                let (prow, brow, mrow) = surface.span_mut(x as u32, py as u32, n);
                // Already at this dab's opacity (the middle of a stroke at
                // steady pressure): nothing to do.
                if min_u8(mrow) >= target {
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
                // ...then the wash step (vectorized).
                let mut new = [0u8; CHUNK];
                let new = &mut new[..n];
                if falling {
                    // Krita's creamy alpha darken with averageOpacity >
                    // opacity: lerp(src, average, mask / average), never
                    // lowering a pixel. A new pixel gets the dab's own
                    // (lower) opacity; a painted one is pulled toward the
                    // average.
                    for ((nv, &cur), &sh) in new.iter_mut().zip(mrow.iter()).zip(shape.iter()) {
                        let src = capf * sh as f32 * (1.0 / 256.0);
                        let m = cur as f32;
                        let v = src + (avgf - src) * (m * inv_avg).min(1.0);
                        *nv = (v + 0.5).max(m) as i32 as u8;
                    }
                } else {
                    for ((nv, &cur), &sh) in new.iter_mut().zip(mrow.iter()).zip(shape.iter()) {
                        let gap = cap.saturating_sub(cur) as u16;
                        *nv = cur + ((gap * sh + 128) >> 8) as u8;
                    }
                }
                let Some((c0, c1)) = changed_span(mrow, new) else {
                    x += n as i32;
                    continue;
                };
                mrow[c0..c1].copy_from_slice(&new[c0..c1]);
                if self.brush.erase {
                    erase_pixels(&mut prow[c0 * 4..c1 * 4], &brow[c0 * 4..c1 * 4], &new[c0..c1]);
                } else {
                    write_pixels(&mut prow[c0 * 4..c1 * 4], &brow[c0 * 4..c1 * 4], &new[c0..c1], color, blend);
                }
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

    /// Zoomed out, samples can land millions of pixels off the canvas: the
    /// path must still end (f32 steps stopped advancing there) and paint
    /// its part on the canvas.
    #[test]
    fn huge_jumps_off_the_canvas_end() {
        let mut e = DabEngine::new();
        let mut b = Brush::new();
        b.radius = 1.0;
        e.sync_from(&b);
        let mut s = white();
        e.begin_stroke();
        e.stroke_to(&mut s, 10.0, 10.0, 1.0);
        e.stroke_to(&mut s, 3.0e7, -2.0e7, 1.0);
        e.stroke_to(&mut s, 30.0, 30.0, 1.0);
        e.end_stroke(&mut s);
        assert!(red_at(&s, 10, 10) < 128 && red_at(&s, 30, 30) < 128);
    }

    const N: u32 = 256;

    use aurastudio_canvas::FlatSurface as T;

    fn white() -> T {
        T::new(N, N, [255; 4])
    }

    fn red_at(s: &T, x: u32, y: u32) -> u8 { s.pixels[((y * N + x) * 4) as usize] }

    fn engine(radius: f32, hardness: f32) -> DabEngine {
        let mut b = Brush::new();
        b.radius = radius;
        b.hardness = hardness;
        let mut e = DabEngine::new();
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

    /// 1 px and 3 px brushes (line clean-up): a horizontal line through
    /// pixel centers is that many pixels thick.
    #[test]
    fn thin_brushes_have_their_width() {
        for (d, rows) in [(1.0f32, 1u32), (3.0, 3)] {
            let mut s = white();
            let mut e = engine(d / 2.0, 1.0);
            line(&mut e, &mut s, 10.0, 50.0, 30.5, 1.0);
            let dark: Vec<u32> = (20..41).filter(|&y| red_at(&s, 30, y) < 128).collect();
            assert_eq!(dark.len() as u32, rows, "{d} px brush: dark rows {dark:?}");
        }
    }

    #[test]
    fn pressure_gives_matching_gray() {
        // 20 % pressure, black: the middle of the stroke is 20 % gray (204).
        let mut s = white();
        let mut e = engine(10.0, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 204).abs() <= 2, "expected ~204, got {v}");
    }

    #[test]
    fn crossing_the_same_stroke_does_not_build_up() {
        let mut s = white();
        let mut e = engine(10.0, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        line(&mut e, &mut s, 200.0, 40.0, 128.0, 0.3);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 178).abs() <= 2, "expected ~178 (30 %), got {v}");
    }

    #[test]
    fn stroke_reaches_the_end_point() {
        let mut s = white();
        let mut e = engine(40.0, 0.9);
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
        let mut e = engine(40.0, 0.0);
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
        // (A per-pixel max bends by 30+ here; 8-bit rounding of the dabs
        // alone leaves a few levels of wobble in the faint edge.)
        assert!(worst <= 12, "kink along the row: {worst} ({v:?})");
    }

    #[test]
    fn falling_pressure_fades_without_beads() {
        // Pressure going down along a stroke: along its middle the values
        // only lighten, gradually (no darker arc left by each dab).
        let mut s = white();
        let mut e = engine(30.0, 1.0);
        for i in 0..=120 {
            let x = 30.0 + i as f32 * 1.6;
            e.stroke_to(&mut s, x, 128.0, 1.0 - 0.7 * i as f32 / 120.0);
        }
        let row: Vec<i32> = (60..200).map(|x| red_at(&s, x, 128) as i32).collect();
        assert!(row.windows(2).all(|w| w[1] >= w[0] - 1), "not monotonic: {row:?}");
    }

    #[test]
    fn big_brush_paints_few_dabs() {
        let mut e = engine(500.0, 0.5);
        assert!(e.spacing > 20.0, "spacing {}", e.spacing);
        e.begin_stroke();
    }
}

//! Wash painting (Krita's indirect painting): each dab pulls the stroke
//! mask toward its opacity, never past it; the layer is the pre-stroke
//! pixels with the color at the mask's coverage. Falling pressure follows
//! Krita's averaged opacity (no "beads"). Shape: opaque to radius ×
//! hardness, then a smoothstep.

use aurastudio_core::{blend::unit_to_u8, tile_run, BlendMode, PaintSurface, Rect};

use crate::forge::raster::{changed_span, erase_pixels, min_u8, round_small, write_pixels, CHUNK, IOTA};
use crate::forge::Dab;

/// A stroke's wash state.
#[derive(Default)]
pub struct Wash {
    /// Averaged opacity (0..255), see the module doc.
    average: f32,
}

impl Wash {
    pub fn begin_stroke(&mut self) { self.average = 0.0; }

    /// One dab; returns the pixels changed.
    pub fn dab<S: PaintSurface>(&mut self, surface: &mut S, dab: &Dab, opacity: f32, blend: BlendMode) -> Option<Rect> {
        let (cx, cy) = (dab.x, dab.y);
        let r = dab.radius.max(0.5);
        let reach = r + 0.5;
        let soft = (r * (1.0 - dab.hardness.clamp(0.0, 1.0))).max(1.0);
        let smooth_edge = (soft > 1.0).then_some(1.0 / soft);
        // Inside this distance the shape is full (256).
        let core = reach - soft;
        let core2 = if core > 0.0 { core * core } else { -1.0 };
        let erase = dab.alpha_eraser < 0.5;
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let color = [c(dab.color.r), c(dab.color.g), c(dab.color.b)];

        let opacity = (dab.opaque * opacity).clamp(0.0, 1.0) * 255.0;
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
        let y0 = ((cy - reach).floor() as i32).max(0);
        let y1 = ((cy + reach).ceil() as i32 + 1).min(h);
        if y0 >= y1 { return None; }
        let x0 = ((cx - reach).floor() as i32).max(0);
        let x1 = ((cx + reach).ceil() as i32 + 1).min(w);
        if x0 >= x1 { return None; }
        surface.prepare((x0 as u32, y0 as u32, x1 as u32, y1 as u32));
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
            // By chunks within a tile: the new coverage (vectorized), then the
            // pixels where it changed.
            let mut x = xa;
            while x < xb {
                let n = tile_run(x as u32, ((xb - x) as usize).min(CHUNK));
                let (prow, brow, mrow) = surface.span_mut(x as u32, py as u32, n);
                // Already at this opacity: nothing to do.
                if min_u8(mrow) >= target {
                    x += n as i32;
                    continue;
                }
                // Dab shape over the run, 0..=256.
                let mut shape = [0u16; CHUNK];
                let shape = &mut shape[..n];
                let dx0 = x as f32 + 0.5 - cx;
                let dx1 = dx0 + (n - 1) as f32;
                if dx0.abs().max(dx1.abs()).powi(2) + dy2 <= core2 {
                    // All of the run in the opaque core (most of a big dab).
                    shape.fill(256);
                } else if let Some(inv_soft) = smooth_edge {
                    for (v, &j) in shape.iter_mut().zip(IOTA.iter()) {
                        let dx = dx0 + j;
                        let t = ((reach - (dx * dx + dy2).sqrt()) * inv_soft).clamp(0.0, 1.0);
                        *v = round_small(t * t * (3.0 - 2.0 * t) * 256.0);
                    }
                } else {
                    for (v, &j) in shape.iter_mut().zip(IOTA.iter()) {
                        let dx = dx0 + j;
                        let t = (reach - (dx * dx + dy2).sqrt()).clamp(0.0, 1.0);
                        *v = round_small(t * 256.0);
                    }
                }
                // The wash step.
                let mut new = [0u8; CHUNK];
                let new = &mut new[..n];
                if falling {
                    // Krita's alpha darken with a higher average: never lowers a pixel.
                    for ((nv, &cur), &sh) in new.iter_mut().zip(mrow.iter()).zip(shape.iter()) {
                        let src = capf * sh as f32 * (1.0 / 256.0);
                        let m = cur as f32;
                        let v = src + (avgf - src) * (m * inv_avg).min(1.0);
                        *nv = round_small(v.max(m)) as u8;
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
                if erase {
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

#[cfg(test)]
mod tests {
    use aurastudio_core::FlatSurface as T;

    use crate::{Brush, Forge, ForgeBrush, PenSample};

    const N: u32 = 256;

    fn white() -> T { T::new(N, N, [255; 4]) }

    fn red_at(s: &T, x: u32, y: u32) -> u8 { s.pixels[((y * N + x) * 4) as usize] }

    fn engine(radius: f32, hardness: f32) -> Forge {
        let mut e = Forge::new(ForgeBrush::round());
        e.sync_from(&Brush { radius, hardness, ..Brush::new() });
        e.begin_stroke();
        e
    }

    fn at(x: f32, y: f32, p: f32, i: usize) -> PenSample { PenSample::new(x, y, p, i as f64 * 0.008) }

    fn line(e: &mut Forge, s: &mut T, from: f32, to: f32, y: f32, p: f32) {
        for i in 0..=60 {
            e.stroke_to(s, at(from + (to - from) * i as f32 / 60.0, y, p, i));
        }
    }

    /// Zoomed out, samples can land millions of pixels off the canvas: the
    /// path must still end and paint its part on the canvas.
    #[test]
    fn huge_jumps_off_the_canvas_end() {
        let mut e = engine(1.0, 1.0);
        let mut s = white();
        e.stroke_to(&mut s, at(10.0, 10.0, 1.0, 0));
        e.stroke_to(&mut s, at(3.0e7, -2.0e7, 1.0, 1));
        e.stroke_to(&mut s, at(30.0, 30.0, 1.0, 2));
        e.end_stroke(&mut s);
        assert!(red_at(&s, 10, 10) < 128 && red_at(&s, 30, 30) < 128, "{} {}", red_at(&s, 10, 10), red_at(&s, 30, 30));
    }

    /// A click paints a dot (Krita's dab on press).
    #[test]
    fn a_click_paints() {
        let mut e = engine(5.0, 1.0);
        let mut s = white();
        e.stroke_to(&mut s, at(50.0, 50.0, 1.0, 0));
        e.end_stroke(&mut s);
        assert!(red_at(&s, 50, 50) < 10);
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
        // Two soft strokes crossing at a right angle: along a row in the
        // soft edge of the horizontal one, crossing the vertical one, the
        // values bend smoothly (a per-pixel max bends sharply: a light line).
        let mut s = white();
        let mut e = engine(40.0, 0.0);
        line(&mut e, &mut s, 20.0, 236.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        e.begin_stroke();
        for i in 0..=60 {
            e.stroke_to(&mut s, at(128.0, 20.0 + 216.0 * i as f32 / 60.0, 1.0, i));
        }
        e.end_stroke(&mut s);
        let v: Vec<i32> = (0..60).map(|k| 255 - red_at(&s, 128 + k, 160) as i32).collect();
        let worst = v.windows(3).map(|w| (w[0] - 2 * w[1] + w[2]).abs()).max().unwrap();
        assert!(worst <= 12, "kink along the row: {worst} ({v:?})");
    }

    #[test]
    fn falling_pressure_fades_without_beads() {
        let mut s = white();
        let mut e = engine(30.0, 1.0);
        for i in 0..=120 {
            e.stroke_to(&mut s, at(30.0 + i as f32 * 1.6, 128.0, 1.0 - 0.7 * i as f32 / 120.0, i));
        }
        let row: Vec<i32> = (60..200).map(|x| red_at(&s, x, 128) as i32).collect();
        assert!(row.windows(2).all(|w| w[1] >= w[0] - 1), "not monotonic: {row:?}");
    }

    /// Krita's multiply: two pressure curves on opacity give their product.
    #[test]
    fn curves_multiply() {
        use crate::forge::{BrushInput, BrushSetting, InputMapping};
        let mut b = ForgeBrush::round();
        b.get_mut(BrushSetting::Opaque).inputs.push(InputMapping { input: BrushInput::Pressure, points: vec![(0.0, 0.0), (1.0, 1.0)] });
        let mut e = Forge::new(b);
        e.sync_from(&Brush { radius: 10.0, hardness: 0.9, ..Brush::new() });
        e.begin_stroke();
        let mut s = white();
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.5);
        // 0.5 × 0.5 = 25 %: 191.
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 191).abs() <= 2, "expected ~191, got {v}");
    }
}

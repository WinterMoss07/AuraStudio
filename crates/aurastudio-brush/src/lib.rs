use aurastudio_canvas::{blend::unit_to_u8, BlendMode, PaintSurface, Rect};
use raster::{changed_span, min_u8, write_pixels, CHUNK, IOTA};

mod dab_engine;
mod mypaint_engine;
mod raster;
pub use dab_engine::DabEngine;
pub use mypaint_engine::MyPaintEngine;
/// hokusai's brush type (a parsed .myb), for the brush library.
pub use hokusai::Brush as MyPaintBrush;
pub use hokusai::myb::from_str as parse_myb;

#[derive(Clone, Debug)]
pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    pub radius:  u32,
    /// How the brush color combines with what is already on the layer.
    pub blend:   BlendMode,
    /// Edge softness, 0..1. 1 = crisp edge (1 px anti-aliasing); lower
    /// values fade the coverage out from `radius * hardness` to `radius`.
    pub hardness: f32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25, blend: BlendMode::Normal, hardness: 1.0 }
    }
}

impl Default for Brush {
    fn default() -> Self { Self::new() }
}

/// Distance-based pressure smoothing, shared by both engines.
///
/// Pressure may only move toward the pen value in proportion to the
/// distance travelled, relative to the brush radius. Without it, a short
/// pressure spike leaves a full brush-sized disc at the higher opacity
/// (later, lighter samples can't lower it: no build-up means no "undo"
/// either), which shows as round blotches on big brushes.
/// Positions are never smoothed, so this adds no positional latency.
pub struct PressureSmoother {
    /// Smoothing distance as a fraction of the brush radius. 0 = raw.
    pub span: f32,
    last: Option<(f32, f32, f32)>,
}

impl PressureSmoother {
    pub fn new(span: f32) -> Self { Self { span, last: None } }

    pub fn reset(&mut self) { self.last = None; }

    pub fn apply(&mut self, x: f32, y: f32, pressure: f32, radius: f32) -> f32 {
        let span = radius * self.span;
        let p = match self.last {
            Some((lx, ly, lp)) if span > 0.0 => {
                let dist = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt();
                lp + (pressure - lp) * (dist / span).min(1.0)
            }
            _ => pressure,
        };
        self.last = Some((x, y, p));
        p
    }
}

/// Round pen, rendered as a continuous swept shape (not stamped dabs).
///
/// Each segment between two pointer samples is a capsule; a pixel's
/// opacity is the pressure interpolated at its projection on the segment,
/// so opacity changes smoothly along the stroke. Stamping dabs instead
/// leaves the circular edge of every dab visible as rings whenever the
/// pressure varies, which is very visible on big hard brushes.
///
/// Within one stroke each pixel keeps the MAX coverage it received
/// (stroke_mask): crossing the stroke again at 30% after 20% gives 30%,
/// not 50% (Krita / CSP behaviour with build-up off). The pixel is then
/// recomposited once from stroke_base (the surface before the stroke).
///
/// Returns the bounds of the pixels it changed (None if none).
pub fn stroke_line<S: PaintSurface>(
    from:  (f32, f32),
    to:    (f32, f32),
    brush: &Brush,
    surface: &mut S,
    pressure_from: f32,
    pressure_to:   f32,
) -> Option<Rect> {
    let r = brush.radius.max(1) as f32;
    let cap_a = (brush.opacity * pressure_from).clamp(0.0, 1.0);
    let cap_b = (brush.opacity * pressure_to).clamp(0.0, 1.0);

    let w = surface.width() as i32;
    let h = surface.height() as i32;
    let x0 = ((from.0.min(to.0) - r - 1.0).floor() as i32).max(0);
    let y0 = ((from.1.min(to.1) - r - 1.0).floor() as i32).max(0);
    let x1 = ((from.0.max(to.0) + r + 1.0).ceil() as i32 + 1).min(w);
    let y1 = ((from.1.max(to.1) + r + 1.0).ceil() as i32 + 1).min(h);
    if x0 >= x1 || y0 >= y1 { return None; }

    surface.prepare((x0 as u32, y0 as u32, x1 as u32, y1 as u32));
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len2 = dx * dx + dy * dy;
    let inv_len2 = if len2 > 0.0 { 1.0 / len2 } else { 0.0 };
    let sw = surface.width() as usize;
    let (pixels, stroke_base, stroke_mask) = surface.surfaces_mut();

    // Big brushes: consecutive segments overlap almost entirely, so most
    // pixels of this segment were already painted at (or above) its
    // highest opacity by the previous ones. Skipping them with one byte
    // compare, before any math, is what keeps 200+ px strokes cheap.
    let seg_max_u8 = unit_to_u8(cap_a.max(cap_b));
    let reach = r + 0.5;
    // Width of the fading edge: at least 1 px (anti-aliasing), up to the
    // whole radius at hardness 0.
    let soft = (r * (1.0 - brush.hardness.clamp(0.0, 1.0))).max(1.0);
    let inv_soft = 1.0 / soft;
    let smooth = soft > 1.0;

    // Bounds of the pixels actually written. With big brushes, most of the
    // segment's box is skipped (already covered), and the real change is a
    // thin crescent at the front: reporting only that keeps the layer
    // merge and the GPU upload small (1000 px brush: ~1 % of the box).
    let (mut wx0, mut wy0, mut wx1, mut wy1) = (u32::MAX, u32::MAX, 0u32, 0u32);

    for py in y0..y1 {
        let cy = py as f32 + 0.5;
        // Only the part of the row inside the capsule (a circle-ish shape
        // fills ~79 % of its box at most): no work on the corners.
        let Some((sx0, sx1)) = capsule_row_span(from, to, reach, cy) else { continue };
        let xa = (sx0.floor() as i32).max(x0);
        let xb = ((sx1.ceil() as i32) + 1).min(x1);
        if xa >= xb { continue; }
        let row = py as usize * sw;
        let ry = cy - from.1;
        let (mut row_x0, mut row_x1) = (i32::MAX, i32::MIN);
        let mut x = xa;
        while x < xb {
            let n = ((xb - x) as usize).min(CHUNK);
            let i0 = row + x as usize;
            let mrow = &mut stroke_mask[i0..i0 + n];
            // Big brushes: consecutive segments overlap almost entirely, so
            // most of this one was already painted at (or above) its
            // highest opacity by the previous ones: skip those runs.
            if min_u8(mrow) >= seg_max_u8 {
                x += n as i32;
                continue;
            }
            // Coverage of the run (float math, vectorized): opacity
            // interpolated at the closest point of the segment, times the
            // edge factor at the distance to it.
            let mut cov = [0u8; CHUNK];
            let cov = &mut cov[..n];
            let cx0 = x as f32 + 0.5 - from.0;
            for (c, &j) in cov.iter_mut().zip(IOTA.iter()) {
                let rx = cx0 + j;
                let t = ((rx * dx + ry * dy) * inv_len2).clamp(0.0, 1.0);
                let (ex, ey) = (rx - dx * t, ry - dy * t);
                let d = (ex * ex + ey * ey).sqrt();
                let e = ((reach - d) * inv_soft).clamp(0.0, 1.0);
                let edge = if smooth { e * e * (3.0 - 2.0 * e) } else { e };
                *c = ((cap_a + (cap_b - cap_a) * t) * edge * 255.0 + 0.5) as i32 as u8;
            }
            // Max with what the stroke already gave each pixel (no
            // build-up), then the layer pixels where that changed.
            for (c, &m) in cov.iter_mut().zip(mrow.iter()) {
                *c = (*c).max(m);
            }
            if let Some((c0, c1)) = changed_span(mrow, cov) {
                mrow[c0..c1].copy_from_slice(&cov[c0..c1]);
                let (p0, p1) = ((i0 + c0) * 4, (i0 + c1) * 4);
                write_pixels(&mut pixels[p0..p1], &stroke_base[p0..p1], &cov[c0..c1], brush.color, brush.blend);
                row_x0 = row_x0.min(x + c0 as i32);
                row_x1 = row_x1.max(x + c1 as i32);
            }
            x += n as i32;
        }
        if row_x0 < row_x1 {
            wx0 = wx0.min(row_x0 as u32);
            wx1 = wx1.max(row_x1 as u32);
            wy0 = wy0.min(py as u32);
            wy1 = wy1.max(py as u32 + 1);
        }
    }

    (wx0 < wx1).then_some((wx0, wy0, wx1, wy1))
}

/// Horizontal extent [x0, x1] of the capsule (segment `a`-`b`, radius
/// `reach`) on the row at height `y`, or None if the row misses it.
///
/// The capsule is convex, so its row section is one interval: the union
/// of the two end circles' sections and the swept band's section.
fn capsule_row_span(a: (f32, f32), b: (f32, f32), reach: f32, y: f32) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut add = |l: f32, h: f32| { lo = lo.min(l); hi = hi.max(h); };

    for c in [a, b] {
        let dy = y - c.1;
        let h2 = reach * reach - dy * dy;
        if h2 > 0.0 {
            let h = h2.sqrt();
            add(c.0 - h, c.0 + h);
        }
    }

    // Band: points whose projection falls inside the segment and whose
    // distance to its line is under `reach`.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len > 1e-3 && dx.abs() > 1e-6 {
        let (ux, uy) = (dx / len, dy / len);
        // Along the segment: 0 <= (x - a.x) * ux + (y - a.y) * uy <= len.
        let s0 = (0.0 - (y - a.1) * uy) / ux + a.0;
        let s1 = (len - (y - a.1) * uy) / ux + a.0;
        let (t_lo, t_hi) = (s0.min(s1), s0.max(s1));
        // Across: |(x - a.x) * -uy + (y - a.y) * ux| < reach.
        let (b_lo, b_hi) = if uy.abs() > 1e-6 {
            let c0 = ((y - a.1) * ux - reach) / uy + a.0;
            let c1 = ((y - a.1) * ux + reach) / uy + a.0;
            (c0.min(c1), c0.max(c1))
        } else if ((y - a.1) * ux).abs() < reach {
            (f32::NEG_INFINITY, f32::INFINITY)
        } else {
            (1.0, 0.0)
        };
        let (l, h) = (t_lo.max(b_lo), t_hi.min(b_hi));
        if l <= h { add(l, h); }
    } else if len > 1e-3 {
        // Vertical segment: the band is a column between the end circles.
        let (ymin, ymax) = (a.1.min(b.1), a.1.max(b.1));
        if y >= ymin && y <= ymax { add(a.0 - reach, a.0 + reach); }
    }

    (lo <= hi).then_some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: u32 = 128;

    struct T { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for T {
        fn width(&self) -> u32 { N }
        fn height(&self) -> u32 { N }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    fn surface(rgba: [u8; 4]) -> T {
        let px: Vec<u8> = rgba.repeat((N * N) as usize);
        T { px: px.clone(), base: px, mask: vec![0; (N * N) as usize] }
    }

    fn red_at(s: &T, x: u32, y: u32) -> u8 { s.px[((y * N + x) * 4) as usize] }

    #[test]
    fn visible_on_transparent_layer() {
        let mut s = surface([255, 255, 255, 0]);
        let mut b = Brush::new();
        b.radius = 5;
        stroke_line((64.0, 64.0), (64.0, 64.0), &b, &mut s, 1.0, 1.0);
        let i = ((64 * N + 64) * 4) as usize;
        assert_eq!(&s.px[i..i + 4], &[0, 0, 0, 255]);
    }

    #[test]
    fn pressure_gives_matching_gray_and_no_build_up() {
        let mut s = surface([255; 4]);
        let mut b = Brush::new();
        b.radius = 6;
        stroke_line((10.0, 64.0), (110.0, 64.0), &b, &mut s, 0.2, 0.2);
        assert_eq!(red_at(&s, 60, 64), 204);
        // Same stroke comes back over itself at 30%: 30%, not 50%.
        stroke_line((110.0, 64.0), (10.0, 64.0), &b, &mut s, 0.3, 0.3);
        assert_eq!(red_at(&s, 60, 64), 178);
    }

    #[test]
    fn multiply_brush_darkens_instead_of_covering() {
        // Mid-gray background, red multiply brush: result is darker red,
        // not plain red.
        let mut s = surface([128, 128, 128, 255]);
        let mut b = Brush::new();
        b.radius = 6;
        b.color = [255, 0, 0];
        b.blend = BlendMode::Multiply;
        stroke_line((64.0, 64.0), (64.0, 64.0), &b, &mut s, 1.0, 1.0);
        let i = ((64 * N + 64) * 4) as usize;
        assert_eq!(&s.px[i..i + 4], &[128, 0, 0, 255]);
    }

    #[test]
    fn soft_brush_fades_toward_the_edge() {
        let mut s = surface([255; 4]);
        let mut b = Brush::new();
        b.radius = 40;
        b.hardness = 0.0;
        stroke_line((64.0, 64.0), (64.0, 64.0), &b, &mut s, 1.0, 1.0);
        let (c, mid, out) = (red_at(&s, 64, 64), red_at(&s, 84, 64), red_at(&s, 103, 64));
        assert!(c < 10, "center {c}");
        assert!(mid > 90 && mid < 170, "half radius {mid}");
        assert!(out > 245, "edge {out}");
    }

    #[test]
    fn opacity_interpolates_along_the_segment() {
        // 0% -> 100% over the segment: the middle is ~50% (~128), and the
        // value only grows from left to right (no rings / steps back).
        let mut s = surface([255; 4]);
        let mut b = Brush::new();
        b.radius = 6;
        stroke_line((10.0, 64.0), (110.0, 64.0), &b, &mut s, 0.0, 1.0);
        let mid = red_at(&s, 60, 64) as i32;
        assert!((mid - 128).abs() <= 3, "middle {mid}");
        let row: Vec<u8> = (10..110).map(|x| red_at(&s, x, 64)).collect();
        assert!(row.windows(2).all(|w| w[1] <= w[0]), "not monotonic: {row:?}");
    }

    /// The row-span shortcut must never skip a pixel the full scan paints:
    /// compare with a brute-force reference on segments in all directions.
    #[test]
    fn row_spans_cover_every_painted_pixel() {
        let reach = 9.5;
        for (a, b) in [((40.0, 60.0), (90.0, 70.0)), ((64.0, 20.0), (64.0, 100.0)), ((30.0, 90.0), (95.0, 30.0)),
                       ((50.0, 50.0), (50.3, 50.1)), ((20.0, 64.0), (110.0, 64.0))] {
            for py in 0..128 {
                let y = py as f32 + 0.5;
                let span = capsule_row_span(a, b, reach, y);
                for px in 0..128 {
                    let x = px as f32 + 0.5;
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let (ex, ey) = (x - (a.0 + dx * t), y - (a.1 + dy * t));
                    if ex * ex + ey * ey < reach * reach {
                        let (lo, hi) = span.unwrap_or_else(|| panic!("row {py} missed for {a:?}-{b:?}"));
                        assert!(x >= lo.floor() && x <= hi.ceil() + 1.0, "px {px},{py} outside {lo}..{hi}");
                    }
                }
            }
        }
    }

    #[test]
    fn smoother_limits_pressure_spikes() {
        let mut sm = PressureSmoother::new(0.5);
        let r = 100.0;
        assert_eq!(sm.apply(0.0, 0.0, 0.2, r), 0.2);
        // A spike to 1.0 over 5 px (span = 50 px) only moves 10% of the way.
        let p = sm.apply(5.0, 0.0, 1.0, r);
        assert!((p - 0.28).abs() < 1e-4, "{p}");
    }
}

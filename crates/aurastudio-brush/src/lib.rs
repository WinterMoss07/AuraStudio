use aurastudio_canvas::{PaintSurface, Rect};

mod hokusai_engine;
pub use hokusai_engine::HokusaiEngine;

pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    pub radius:  u32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25 }
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

    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len2 = dx * dx + dy * dy;
    let sw = surface.width() as usize;
    let (pixels, stroke_base, stroke_mask) = surface.surfaces_mut();

    // Big brushes: consecutive segments overlap almost entirely, so most
    // pixels of this segment were already painted at (or above) its
    // highest opacity by the previous ones. Skipping them with one byte
    // compare, before any math, is what keeps 200+ px strokes cheap.
    let seg_max_u8 = (cap_a.max(cap_b) * 255.0).round() as u8;
    let reach2 = (r + 0.5) * (r + 0.5);

    for py in y0..y1 {
        let cy = py as f32 + 0.5;
        let row = py as usize * sw;
        for px in x0..x1 {
            let pidx = row + px as usize;
            if stroke_mask[pidx] >= seg_max_u8 { continue; }

            let cx = px as f32 + 0.5;
            // Closest point on the segment, and where along it (0..1).
            let t = if len2 > 0.0 {
                (((cx - from.0) * dx + (cy - from.1) * dy) / len2).clamp(0.0, 1.0)
            } else { 0.0 };
            let ex = cx - (from.0 + dx * t);
            let ey = cy - (from.1 + dy * t);
            let d2 = ex * ex + ey * ey;
            if d2 >= reach2 { continue; }
            // 1 px anti-aliased edge.
            let edge = (r + 0.5 - d2.sqrt()).clamp(0.0, 1.0);
            if edge <= 0.0 { continue; }

            let cov = (cap_a + (cap_b - cap_a) * t) * edge;
            let cov_u8 = (cov * 255.0).round() as u8;
            if cov_u8 <= stroke_mask[pidx] { continue; }
            stroke_mask[pidx] = cov_u8;

            // Straight-alpha "over" of the brush color on stroke_base:
            // needed on transparent layers (base alpha 0).
            let base  = pidx * 4;
            let sa    = cov_u8 as f32 / 255.0;
            let ba    = stroke_base[base + 3] as f32 / 255.0;
            let k     = ba * (1.0 - sa);
            let out_a = sa + k;
            for c in 0..3 {
                let v = (brush.color[c] as f32 * sa + stroke_base[base + c] as f32 * k) / out_a;
                pixels[base + c] = v.round() as u8;
            }
            pixels[base + 3] = (out_a * 255.0).round() as u8;
        }
    }

    Some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
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

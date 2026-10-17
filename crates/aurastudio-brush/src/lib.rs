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

    /// Peint un dab rond sur n'importe quelle PaintSurface (canvas final
    /// ou calque). La couverture est accumulée via max() dans
    /// stroke_mask (pas de double-blend sur les zones de chevauchement),
    /// puis recomposée depuis stroke_base + couleur en un seul calcul.
    fn paint<S: PaintSurface>(&self, surface: &mut S, cx: i32, cy: i32, pressure: f32) -> Option<Rect> {
        let r      = self.radius as i32;
        let r2     = (r * r) as f32;
        let cov_u8 = ((self.opacity * pressure).clamp(0.0, 1.0) * 255.0).round() as u8;

        let w = surface.width()  as i32;
        let h = surface.height() as i32;
        let x0 = (cx - r).max(0);
        let y0 = (cy - r).max(0);
        let x1 = (cx + r + 1).min(w);
        let y1 = (cy + r + 1).min(h);
        if x0 >= x1 || y0 >= y1 { return None; }

        let sw = surface.width();
        let (pixels, stroke_base, stroke_mask) = surface.surfaces_mut();

        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px - cx;
                let dy = py - cy;
                if (dx * dx + dy * dy) as f32 > r2 { continue; }

                let pidx = (py as u32 * sw + px as u32) as usize;
                if cov_u8 > stroke_mask[pidx] {
                    stroke_mask[pidx] = cov_u8;
                    let base = pidx * 4;
                    // "over" en alpha non prémultiplié : indispensable sur un
                    // calque transparent (alpha de base 0), sinon le RGB est
                    // écrit mais le pixel reste invisible.
                    let sa    = cov_u8 as f32 / 255.0;
                    let ba    = stroke_base[base + 3] as f32 / 255.0;
                    let k     = ba * (1.0 - sa);
                    let out_a = sa + k;
                    for c in 0..3 {
                        let v = (self.color[c] as f32 * sa + stroke_base[base + c] as f32 * k) / out_a;
                        pixels[base + c] = v.round() as u8;
                    }
                    pixels[base + 3] = (out_a * 255.0).round() as u8;
                }
            }
        }

        Some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
    }
}

fn merge(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (None, r) | (r, None) => r,
        (Some(a), Some(b)) => Some((
            a.0.min(b.0), a.1.min(b.1),
            a.2.max(b.2), a.3.max(b.3),
        )),
    }
}

pub fn stroke_line<S: PaintSurface>(
    from:     (f32, f32),
    to:       (f32, f32),
    brush:    &Brush,
    surface:  &mut S,
    pressure: f32,
) -> Option<Rect> {
    let dx   = to.0 - from.0;
    let dy   = to.1 - from.1;
    let dist = (dx * dx + dy * dy).sqrt();
    let step = (brush.radius as f32 * 0.5).max(1.0);
    let n    = (dist / step).ceil() as u32;

    let mut dirty: Option<Rect> = None;
    for i in 0..=n {
        let t  = if n == 0 { 0.0 } else { i as f32 / n as f32 };
        let cx = (from.0 + dx * t) as i32;
        let cy = (from.1 + dy * t) as i32;
        let r  = brush.paint(surface, cx, cy, pressure);
        dirty  = merge(dirty, r);
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    struct T { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for T {
        fn width(&self) -> u32 { 64 }
        fn height(&self) -> u32 { 64 }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    #[test]
    fn simple_brush_is_visible_on_transparent_layer() {
        let clear: Vec<u8> = [255, 255, 255, 0].repeat(64 * 64);
        let mut s = T { px: clear.clone(), base: clear, mask: vec![0; 64 * 64] };
        let mut b = Brush::new();
        b.radius = 5;
        stroke_line((32.0, 32.0), (32.0, 32.0), &b, &mut s, 1.0);
        let i = (32 * 64 + 32) * 4;
        assert_eq!(&s.px[i..i + 4], &[0, 0, 0, 255]);
    }
}

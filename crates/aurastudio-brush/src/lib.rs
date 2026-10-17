use aurastudio_canvas::{Canvas, Rect};

pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    pub radius:  u32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25 }
    }

    /// Peint un dab rond. La couverture est accumulée via max() dans
    /// stroke_mask (pas de double-blend sur les zones de chevauchement),
    /// puis recomposée depuis stroke_base + couleur en un seul calcul.
    fn paint(&self, canvas: &mut Canvas, cx: i32, cy: i32, pressure: f32) -> Option<Rect> {
        let r      = self.radius as i32;
        let r2     = (r * r) as f32;
        let cov_u8 = ((self.opacity * pressure).clamp(0.0, 1.0) * 255.0).round() as u8;

        let w = canvas.width as i32;
        let h = canvas.height as i32;
        let x0 = (cx - r).max(0);
        let y0 = (cy - r).max(0);
        let x1 = (cx + r + 1).min(w);
        let y1 = (cy + r + 1).min(h);
        if x0 >= x1 || y0 >= y1 { return None; }

        let cw = canvas.width;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px - cx;
                let dy = py - cy;
                if (dx * dx + dy * dy) as f32 > r2 { continue; }

                let pidx = (py as u32 * cw + px as u32) as usize;
                if cov_u8 > canvas.stroke_mask[pidx] {
                    canvas.stroke_mask[pidx] = cov_u8;
                    let alpha = cov_u8 as f32 / 255.0;
                    let ia    = 1.0 - alpha;
                    let base  = pidx * 4;

                    canvas.pixels[base]     = (self.color[0] as f32 * alpha + canvas.stroke_base[base]     as f32 * ia) as u8;
                    canvas.pixels[base + 1] = (self.color[1] as f32 * alpha + canvas.stroke_base[base + 1] as f32 * ia) as u8;
                    canvas.pixels[base + 2] = (self.color[2] as f32 * alpha + canvas.stroke_base[base + 2] as f32 * ia) as u8;
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

pub fn stroke_line(
    from:     (f32, f32),
    to:       (f32, f32),
    brush:    &Brush,
    canvas:   &mut Canvas,
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
        let r  = brush.paint(canvas, cx, cy, pressure);
        dirty  = merge(dirty, r);
    }
    dirty
}

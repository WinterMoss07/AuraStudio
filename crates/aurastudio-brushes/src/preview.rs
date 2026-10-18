//! Brush previews: a short S stroke painted by the brush's own engine
//! (DabEngine or MyPaint) with its current settings, as the Brushes and
//! Tool options dockers show it. The size follows the brush's (true size
//! for small brushes, compressed for big ones), so does the softness,
//! opacity and blend mode.

use aurastudio_brush::{Brush, DabEngine, MyPaintEngine};
use aurastudio_canvas::FlatSurface;

use crate::Engine;

/// Stroke color: light, for the dark dockers.
pub const INK: [u8; 3] = [226, 226, 232];

/// Radius (preview pixels) of a brush of radius `r` (canvas pixels) in a
/// strip `h` preview pixels high, `scale` preview pixels per canvas pixel
/// at true size. True size up to a third of the room, then compressed
/// (logarithmic up to 1000 px): every size change still shows.
pub fn shown_radius(r: f32, h: u32, scale: f32) -> f32 {
    let rmax = h as f32 * 0.36;
    let r0 = rmax / 3.0;
    let true_r = r.max(0.5) * scale;
    if true_r <= r0 { return true_r; }
    let top = (1000.0 * scale).max(r0 * 1.01);
    r0 + (rmax - r0) * ((true_r / r0).ln() / (top / r0).ln()).min(1.0)
}

/// Paint the preview: `w` × `h` straight RGBA, transparent around the
/// stroke. `settings.erase` erases (the Eraser tool).
pub fn render(engine: &Engine, settings: &Brush, w: u32, h: u32, scale: f32) -> Vec<u8> {
    let mut b = settings.clone();
    b.color = INK;
    b.radius = shown_radius(settings.radius, h, scale);
    // Erasers, smudge brushes... need paint to work on: color bands.
    let on_paint = b.erase || match engine {
        Engine::MyPaint(p) => MyPaintEngine::works_on_paint(p),
        Engine::Native => false,
    };
    let mut blank = FlatSurface::new(w, h, [255, 255, 255, 0]);
    if on_paint { backdrop(&mut blank); }

    // Brushes whose dynamics grow the dab well past its radius (pressure,
    // random offsets) would spill out of the strip: smaller until it fits.
    let mut out = stroke(engine, &b, &blank);
    for _ in 0..3 {
        if fits(&out, &blank.pixels, w, h) || b.radius <= 0.5 { break; }
        b.radius = (b.radius * 0.6).max(0.5);
        let smaller = stroke(engine, &b, &blank);
        // Too small to paint anything (dynamics): keep the last one.
        if smaller == blank.pixels { break; }
        out = smaller;
    }
    out
}

fn stroke(engine: &Engine, b: &Brush, blank: &FlatSurface) -> Vec<u8> {
    let (w, h) = (blank.width, blank.height);
    let mut s = FlatSurface::new(w, h, [0; 4]);
    s.pixels.copy_from_slice(&blank.pixels);
    s.next_stroke();
    let path = s_curve(w, h, b.radius);
    match engine {
        Engine::Native => {
            let mut e = DabEngine::new();
            e.sync_from(b);
            e.begin_stroke();
            for &(x, y, p, _) in &path { e.stroke_to(&mut s, x, y, p); }
            e.end_stroke(&mut s);
        }
        Engine::MyPaint(p) => {
            let mut e = MyPaintEngine::new((**p).clone());
            e.sync_from(b);
            e.begin_stroke();
            for &(x, y, p, t) in &path { e.stroke_to(&mut s, x, y, p, t); }
            e.end_stroke(&mut s);
        }
    }
    s.pixels
}

/// The stroke stays off the strip's top and bottom rows.
fn fits(px: &[u8], blank: &[u8], w: u32, h: u32) -> bool {
    let row = (w * 4) as usize;
    let touched = |y: u32| { let r = y as usize * row..(y as usize + 1) * row; px[r.clone()] != blank[r] };
    h < 4 || !(touched(0) || touched(1) || touched(h - 1) || touched(h - 2))
}

/// Pen samples (x, y, pressure, time) along an S across the strip, at
/// 200 Hz, pressure rising then falling (tapered ends).
fn s_curve(w: u32, h: u32, r: f32) -> Vec<(f32, f32, f32, f64)> {
    let (w, h) = (w as f32, h as f32);
    let margin = r + 3.0;
    let amp = ((h / 2.0 - margin) * 0.7).max(0.0);
    let n = ((w - 2.0 * margin) / 3.0).ceil().max(8.0) as usize;
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let x = margin + t * (w - 2.0 * margin);
        let y = h / 2.0 - amp * (t * std::f32::consts::TAU).sin();
        let p = 0.1 + 0.9 * (t * std::f32::consts::PI).sin().powf(0.8);
        (x, y, p, i as f64 * 0.005)
    }).collect()
}

/// Muted color bands across the middle of the strip.
fn backdrop(s: &mut FlatSurface) {
    const BANDS: [[u8; 4]; 3] = [[150, 92, 84, 255], [92, 130, 104, 255], [88, 104, 150, 255]];
    let (w, h) = (s.width, s.height);
    let (y0, y1) = (h / 5, h - h / 5);
    for y in y0..y1 {
        for x in 0..w {
            let c = BANDS[(x * 3 / w.max(1)) as usize % 3];
            let i = ((y * w + x) * 4) as usize;
            s.pixels[i..i + 4].copy_from_slice(&c);
        }
    }
    s.next_stroke();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    fn coverage(px: &[u8]) -> usize { px.chunks(4).filter(|p| p[3] > 0).count() }

    #[test]
    fn size_and_softness_show() {
        let mut b = Brush::new();
        b.hardness = 1.0;
        let sizes: Vec<usize> = [0.5, 3.0, 10.0, 50.0, 300.0, 1000.0].iter().map(|&r| {
            b.radius = r;
            coverage(&render(&Engine::Native, &b, 240, 64, 1.0))
        }).collect();
        assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?}");
        assert!(shown_radius(1000.0, 64, 1.0) <= 64.0 * 0.36 + 0.01);
        assert_eq!(shown_radius(2.0, 64, 2.0), 4.0);

        // Soft: lighter edges, so less full coverage.
        b.radius = 300.0;
        let full = |px: &[u8]| px.chunks(4).filter(|p| p[3] > 250).count();
        let hard = full(&render(&Engine::Native, &b, 240, 64, 1.0));
        b.hardness = 0.0;
        let soft = full(&render(&Engine::Native, &b, 240, 64, 1.0));
        assert!(soft < hard / 2, "hard {hard} soft {soft}");
    }

    #[test]
    fn every_bundled_brush_draws_something() {
        let lib = Library::load();
        for p in &lib.presets {
            let px = render(&p.engine, &p.settings, 160, 48, 1.0);
            let blank = FlatSurface::new(160, 48, [255, 255, 255, 0]).pixels;
            let mut with_band = FlatSurface::new(160, 48, [255, 255, 255, 0]);
            backdrop(&mut with_band);
            assert!(px != blank && px != with_band.pixels, "{} / {}: empty preview", p.group, p.name);
        }
    }
}

//! Brush previews: a short S stroke painted by the brush's own engine
//! (DabEngine or MyPaint) with its current settings, as the Brushes and
//! Tool options dockers show it. The size follows the brush's (true size
//! for small brushes, compressed for big ones), so does the softness,
//! opacity and blend mode.

use aurastudio_brush::{Brush, DabEngine, MyPaintEngine, PaintUse};
use aurastudio_canvas::FlatSurface;

use crate::Engine;

/// Stroke color: light, for the dark dockers.
pub const INK: [u8; 3] = [226, 226, 232];

/// Largest radius shown, as a share of the strip's height.
const MAX_RADIUS: f32 = 0.26;

/// Radius (preview pixels) of a brush of radius `r` (canvas pixels) in a
/// strip `h` preview pixels high, `scale` preview pixels per canvas pixel
/// at true size. True size up to a third of the room, then compressed
/// (logarithmic up to 1000 px): every size change still shows.
pub fn shown_radius(r: f32, h: u32, scale: f32) -> f32 {
    let rmax = h as f32 * MAX_RADIUS;
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
    // Erasers, blenders... need paint to work on: a first stroke. An
    // eraser or a wet brush crosses it (the cuts / the mixing show); a
    // blender goes along it three times, back and forth (dragging each
    // color into the next), like scrubbing.
    let mut use_ = match engine {
        Engine::MyPaint(p) => MyPaintEngine::paint_use(p),
        Engine::Native => PaintUse::None,
    };
    if b.erase { use_ = PaintUse::Erases; }
    let mut blank = FlatSurface::new(w, h, [255, 255, 255, 0]);
    let along = use_ == PaintUse::Moves;
    if use_ != PaintUse::None { base_stroke(&mut blank, if along { 1.0 } else { -1.0 }, b.radius); }
    // Brushes whose dynamics grow the dab well past its radius (pressure,
    // random offsets) would spill out of the strip: smaller until it fits.
    let mut out = stroke(engine, &b, &blank, along);
    for _ in 0..3 {
        if fits(&out, &blank.pixels, w, h) || b.radius <= 0.5 { break; }
        b.radius = (b.radius * 0.6).max(0.5);
        let smaller = stroke(engine, &b, &blank, along);
        // Too small to paint anything (dynamics): keep the last one.
        if smaller == blank.pixels { break; }
        out = smaller;
    }
    out
}

/// The brush's S over `blank` (`scrub`: there, back and there again).
fn stroke(engine: &Engine, b: &Brush, blank: &FlatSurface, scrub: bool) -> Vec<u8> {
    let (w, h) = (blank.width, blank.height);
    let mut s = FlatSurface::new(w, h, [0; 4]);
    s.pixels.copy_from_slice(&blank.pixels);
    s.next_stroke();
    let mut path = s_curve(w, h, 1.0);
    if scrub {
        // There, back, there again: blenders are gentle, one pass barely
        // shows.
        let n = path.len();
        let fwd = path.clone();
        for pass in 1..3 {
            let leg: Vec<_> = if pass % 2 == 1 { fwd.iter().rev().copied().collect() } else { fwd.clone() };
            path.extend(leg.into_iter().enumerate().map(|(i, (x, y, p, _))| (x, y, p, (pass * n + i) as f64 * 0.005)));
        }
    }
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
/// 200 Hz, pressure rising then falling (tapered ends). The same S
/// whatever the brush's size (only the stroke's width changes); `phase`
/// -1 flips it.
fn s_curve(w: u32, h: u32, phase: f32) -> Vec<(f32, f32, f32, f64)> {
    let (w, h) = (w as f32, h as f32);
    let margin = h * MAX_RADIUS + 3.0;
    let amp = (h / 2.0 - margin).max(0.0) * 0.9;
    let n = ((w - 2.0 * margin) / 3.0).ceil().max(8.0) as usize;
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let x = margin + t * (w - 2.0 * margin);
        let y = h / 2.0 - phase * amp * (t * std::f32::consts::TAU).sin();
        let p = 0.1 + 0.9 * (t * std::f32::consts::PI).sin().powf(0.8);
        (x, y, p, i as f64 * 0.005)
    }).collect()
}

/// The stroke erasers and blenders work on: the S (`phase` -1: flipped),
/// a bit wider than the brush (radius `r`), in
/// muted warm and cool segments (a blender dragging along the stroke
/// shows by smearing one into the next).
fn base_stroke(s: &mut FlatSurface, phase: f32, r: f32) {
    let (w, h) = (s.width, s.height);
    let mut b = Brush::new();
    // As wide as the brush (a bit wider): it covers it at any size.
    b.radius = (r * 1.3).clamp(2.0, h as f32 * 0.2);
    b.hardness = 0.9;
    b.color = [255, 255, 255];
    let mut e = DabEngine::new();
    e.sync_from(&b);
    e.begin_stroke();
    for (x, y, _, _) in s_curve(w, h, phase) { e.stroke_to(s, x, y, 1.0); }
    e.end_stroke(s);
    const COLORS: [[u8; 3]; 2] = [[176, 138, 116], [104, 124, 166]];
    for (i, p) in s.pixels.chunks_exact_mut(4).enumerate() {
        let segment = (i as u32 % w) * 6 / w.max(1);
        p[..3].copy_from_slice(&COLORS[segment as usize % 2]);
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
        assert!(shown_radius(1000.0, 64, 1.0) <= 64.0 * MAX_RADIUS + 0.01);
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
            let mut base = FlatSurface::new(160, 48, [255, 255, 255, 0]);
            base_stroke(&mut base, 1.0, 10.0);
            assert!(px != blank && px != base.pixels, "{} / {}: empty preview", p.group, p.name);
        }
    }
}

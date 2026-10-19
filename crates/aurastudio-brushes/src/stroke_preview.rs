//! Stroke previews: a stroke painted by the brush's own engine (DabEngine
//! or MyPaint) with its current settings, as the Tools and Tool options
//! dockers show it.
//!
//! - `Mode::List` (the Tools docker): the stroke alone, light, on nothing
//!   (the UI shows through). Erasers and blenders are drawn as if they
//!   painted: the list shows the shape of the stroke (hard, soft,
//!   textured, tapered).
//! - `Mode::Detail` (Tool options), like Clip Studio Paint: the left half
//!   is light gray paint, the right half nothing (the dark UI shows
//!   through); the stroke starts in the paint and leaves it. A brush paints
//!   over both, an eraser cuts through the paint, a blender drags it out.
//!
//! The size follows the brush's (true size for small brushes, compressed
//! for big ones); the S stays the same whatever the size.

use aurastudio_brush::{Brush, DabEngine, MyPaintEngine};
use aurastudio_canvas::FlatSurface;

use crate::Engine;

/// Stroke color: light, for the dark UI.
pub const INK: [u8; 3] = [236, 236, 240];
/// The paint of the detail mode's left half.
pub const PAPER: [u8; 3] = [178, 178, 182];

/// Largest radius shown, as a share of the strip's height.
const MAX_RADIUS: f32 = 0.26;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode { List, Detail }

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

/// Paint the preview: `w` × `h` straight RGBA (transparent where nothing
/// is). `settings.erase`: the Eraser tool.
pub fn render(engine: &Engine, settings: &Brush, mode: Mode, w: u32, h: u32, scale: f32) -> Vec<u8> {
    let mut b = settings.clone();
    b.color = INK;
    b.radius = shown_radius(settings.radius, h, scale);
    let mut blank = FlatSurface::new(w, h, [255, 255, 255, 0]);
    let engine = match (mode, engine) {
        // The list shows the stroke's shape: everything paints.
        (Mode::List, Engine::MyPaint(p)) => {
            b.erase = false;
            Engine::MyPaint(std::sync::Arc::new(MyPaintEngine::painting_only(p)))
        }
        (Mode::List, e) => { b.erase = false; e.clone() }
        (Mode::Detail, e) => {
            paper(&mut blank);
            e.clone()
        }
    };

    // Brushes whose dynamics grow the dab well past its radius (pressure,
    // random offsets) would spill out of the strip: smaller until it fits.
    let mut out = stroke(&engine, &b, &blank);
    for _ in 0..3 {
        if fits(&out, &blank.pixels, w, h) || b.radius <= 0.5 { break; }
        b.radius = (b.radius * 0.6).max(0.5);
        let smaller = stroke(&engine, &b, &blank);
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
    let path = s_curve(w, h);
    match engine {
        Engine::Native => {
            let mut e = DabEngine::new();
            e.sync_from(b);
            e.set_spacing(b.radius * 0.08);
            e.begin_stroke();
            for &(x, y, p, _) in &path { e.stroke_to(&mut s, x, y, p); }
            e.end_stroke(&mut s);
        }
        Engine::MyPaint(p) => {
            MyPaintEngine::reset_random();
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

/// Pen samples (x, y, pressure, time) at 200 Hz along an S across the
/// strip, pressure rising then falling (tapered ends).
fn s_curve(w: u32, h: u32) -> Vec<(f32, f32, f32, f64)> {
    let (w, h) = (w as f32, h as f32);
    let margin = h * MAX_RADIUS + 3.0;
    let amp = (h / 2.0 - margin).max(0.0) * 0.9;
    let n = ((w - 2.0 * margin) / 3.0).ceil().max(8.0) as usize;
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let p = 0.1 + 0.9 * (t * std::f32::consts::PI).sin().powf(0.8);
        (margin + t * (w - 2.0 * margin), h / 2.0 - amp * (t * std::f32::consts::TAU).sin(), p, i as f64 * 0.005)
    }).collect()
}

/// The detail mode's paint: the left half, light gray, opaque.
fn paper(s: &mut FlatSurface) {
    let (w, h) = (s.width, s.height);
    for y in 0..h {
        for x in 0..w / 2 {
            let i = ((y * w + x) * 4) as usize;
            s.pixels[i..i + 4].copy_from_slice(&[PAPER[0], PAPER[1], PAPER[2], 255]);
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
            coverage(&render(&Engine::Native, &b, Mode::List, 240, 64, 1.0))
        }).collect();
        assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?}");
        assert!(shown_radius(1000.0, 64, 1.0) <= 64.0 * MAX_RADIUS + 0.01);
        assert_eq!(shown_radius(2.0, 64, 2.0), 4.0);

        // Soft: lighter edges, so less full coverage.
        b.radius = 300.0;
        let full = |px: &[u8]| px.chunks(4).filter(|p| p[3] > 250).count();
        let hard = full(&render(&Engine::Native, &b, Mode::List, 240, 64, 1.0));
        b.hardness = 0.0;
        let soft = full(&render(&Engine::Native, &b, Mode::List, 240, 64, 1.0));
        assert!(soft < hard / 2, "hard {hard} soft {soft}");
    }

    #[test]
    fn every_bundled_brush_draws_something_in_both_modes() {
        let lib = Library::load();
        let mut detail_blank = FlatSurface::new(160, 48, [255, 255, 255, 0]);
        paper(&mut detail_blank);
        for p in &lib.presets {
            let list = render(&p.engine, &p.settings, Mode::List, 160, 48, 1.0);
            assert!(coverage(&list) > 20, "{} / {}: empty list preview", p.medium, p.name);
            let detail = render(&p.engine, &p.settings, Mode::Detail, 160, 48, 1.0);
            assert!(detail != detail_blank.pixels, "{} / {}: empty detail preview", p.medium, p.name);
        }
    }

    #[test]
    fn eraser_cuts_the_paint_in_detail_mode() {
        let mut b = Brush::new();
        b.radius = 8.0;
        b.erase = true;
        let px = render(&Engine::Native, &b, Mode::Detail, 200, 60, 1.0);
        let holes = px.chunks(4).enumerate().filter(|(i, p)| (*i as u32 % 200) < 100 && p[3] < 128).count();
        let painted_right = px.chunks(4).enumerate().filter(|(i, p)| (*i as u32 % 200) >= 100 && p[3] > 0).count();
        assert!(holes > 200 && painted_right == 0, "holes {holes}, right {painted_right}");
    }


}

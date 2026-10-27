//! Stroke previews painted by Forge: alone in the Tools list (erasers and
//! blenders drawn as paint), over half a gray band in Tool options (CSP).
//! A size change only grows the dabs: they keep their places (fixed
//! spacing, ½, 1, 2, 4... px) and their random numbers (seeds by place);
//! a brush painting past its radius gets a smaller size (`overflow`).

use aurastudio_forge::forge::{BrushSetting, Spacing};
use aurastudio_forge::{Brush, Forge, ForgeBrush, PenSample};
use aurastudio_core::FlatSurface;

/// Bump when `render` or the brush engine changes how a preview looks:
/// previews kept on disk under another version are not used.
pub const RENDER_VERSION: u32 = 1;

/// Stroke color: light, for the dark UI.
pub const INK: [u8; 3] = [236, 236, 240];
/// The paint of the detail mode's left half.
pub const PAPER: [u8; 3] = [178, 178, 182];

/// Largest radius shown, as a share of the strip's height.
const MAX_RADIUS: f32 = 0.26;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode { List, Detail }

/// What the stroke draws: a brush's S, or a geometry tool's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape { S, Line, Rectangle, Ellipse, Arc }

/// Thickness (diameter, preview pixels) of a geometry tool's shape for a
/// brush of diameter `d`: 1 to 15, logarithmic over 1..1000 px.
pub fn shape_thickness(d: f32) -> f32 {
    1.0 + 14.0 * (d.clamp(1.0, 1000.0).ln() / 1000f32.ln())
}

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

/// Finest dab spacing of the previews, px.
const FINEST: f32 = 0.5;

/// Paint the preview: `w` × `h` straight RGBA (transparent where nothing
/// is). `settings.erase`: the Eraser tool.
pub fn render(brush: &ForgeBrush, settings: &Brush, shape: Shape, mode: Mode, w: u32, h: u32, scale: f32) -> Vec<u8> {
    let mut b = settings.clone();
    b.color = INK;
    // The list shows the stroke's shape: everything paints.
    let painting = Forge::painting_only(brush);
    let over = overflow(brush, &painting, &b, shape, w, h);
    b.radius = match shape {
        Shape::S => {
            let room = h as f32 * MAX_RADIUS + 1.0;
            shown_radius(settings.radius, h, scale).min(room / over)
        }
        _ => shape_thickness(settings.radius * 2.0) / 2.0 / over,
    }.max(0.5);
    let mut blank = FlatSurface::new(w, h, [255, 255, 255, 0]);
    let brush = match mode {
        Mode::List => {
            b.erase = false;
            painting
        }
        Mode::Detail => {
            paper(&mut blank);
            brush.clone()
        }
    };

    // Spacing step: the brush's own spacing rounded down to ½, 1, 2...
    // px; near the next step, a fade into it.
    let steps = (natural_spacing(&brush, b.radius, settings.radius) / FINEST).max(1.0).log2();
    let level = steps.floor();
    let spacing = FINEST * level.exp2();
    let stride = 1u32 << (level as u32).min(16);
    let out = stroke(&brush, &b, shape, &blank, spacing, stride);
    let t = ((steps - level - 0.6) / 0.4).clamp(0.0, 1.0);
    let fade = t * t * (3.0 - 2.0 * t);
    if fade <= 0.0 { return out; }
    let next = stroke(&brush, &b, shape, &blank, spacing * 2.0, stride * 2);
    mix(&out, &next, fade)
}

/// Dab spacing for a dab of radius `r` (px), as dense (dabs per radius)
/// as the brush paints at its real radius `real` (overlapping dabs add
/// up: a soft brush gets harder with denser dabs).
fn natural_spacing(brush: &ForgeBrush, r: f32, real: f32) -> f32 {
    let per_radius = match brush.options.spacing {
        Spacing::Auto(k) => { let real = real.max(0.5); real / (k * (2.0 * real).sqrt()).max(0.5) }
        _ => brush.get(BrushSetting::DabsPerActualRadius).base_value + brush.get(BrushSetting::DabsPerBasicRadius).base_value,
    };
    if per_radius > 0.0 { r / per_radius } else { r / 2.0 }
}

fn stroke(brush: &ForgeBrush, b: &Brush, shape: Shape, blank: &FlatSurface, spacing: f32, stride: u32) -> Vec<u8> {
    let (w, h) = (blank.width, blank.height);
    let mut s = FlatSurface::new(w, h, [0; 4]);
    s.pixels.copy_from_slice(&blank.pixels);
    s.next_stroke();
    paint(brush, b, &mut s, &path(shape, w, h), spacing, stride, 0.0);
    s.pixels
}

fn paint(brush: &ForgeBrush, b: &Brush, s: &mut FlatSurface, path: &[(f32, f32, f32, f64)], spacing: f32, stride: u32, dy: f32) {
    Forge::reset_random();
    let mut e = Forge::new(brush.clone());
    e.sync_from(b);
    e.preview_spacing(spacing, stride);
    e.begin_stroke();
    for &(x, y, p, t) in path { e.stroke_to(s, PenSample::new(x, y + dy, p, t)); }
    e.end_stroke(s);
}

/// `a` faded into `b` (straight RGBA, by premultiplied colors).
fn mix(a: &[u8], b: &[u8], t: f32) -> Vec<u8> {
    a.chunks(4).zip(b.chunks(4)).flat_map(|(p, q)| {
        let (pa, qa) = (p[3] as f32 * (1.0 - t), q[3] as f32 * t);
        let al = pa + qa;
        let c = |k: usize| if al > 0.0 { ((p[k] as f32 * pa + q[k] as f32 * qa) / al).round() as u8 } else { 0 };
        [c(0), c(1), c(2), al.round().min(255.0) as u8]
    }).collect()
}

/// How far the brush paints from its path, in radii (1 for a plain round
/// brush; more with dynamics growing the dab or scattering it). Measured
/// once per brush (and hardness, shape kind), on the stroke's own path in
/// a strip tall enough not to clip it.
fn overflow(brush: &ForgeBrush, painting: &ForgeBrush, b: &Brush, shape: Shape, w: u32, h: u32) -> f32 {
    use std::collections::HashMap;
    use std::sync::Mutex;
    /// (brush, hardness, S or shape, strip size) -> overflow.
    type Key = (usize, u32, bool, u32, u32);
    static CACHE: Mutex<Option<HashMap<Key, f32>>> = Mutex::new(None);
    let key = (brush as *const ForgeBrush as usize, b.hardness.to_bits(), shape == Shape::S, w, h);
    if let Some(&v) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) { return v; }

    const R: f32 = 6.0;
    let probe = Brush { radius: R, erase: false, ..b.clone() };
    // A straight line stands for the shapes (drawn at full pressure).
    let line = |w: u32, h: u32| vec![(4.0, h as f32 / 2.0, 1.0, 0.0), (w as f32 - 4.0, h as f32 / 2.0, 1.0, 0.1)];
    let (path, half) = match shape {
        Shape::S => {
            let margin = h as f32 * MAX_RADIUS + 3.0;
            (path(Shape::S, w, h), (h as f32 / 2.0 - margin).max(0.0) * 0.9)
        }
        _ => (line(w, h), 0.0),
    };
    let tall = (h * 4).max(64);
    let dy = (tall - h) as f32 / 2.0;
    let mut s = FlatSurface::new(w, tall, [255, 255, 255, 0]);
    paint(painting, &probe, &mut s, &path, natural_spacing(painting, R, R).max(FINEST), 1, dy);
    let row = |y: u32| s.pixels[(y * w * 4) as usize..((y + 1) * w * 4) as usize].chunks(4).any(|c| c[3] > 8);
    let center = h as f32 / 2.0 + dy;
    let v = match ((0..tall).find(|&y| row(y)), (0..tall).rev().find(|&y| row(y))) {
        (Some(top), Some(bottom)) => {
            let reach = ((center - half) - top as f32).max(bottom as f32 + 1.0 - (center + half));
            (reach / R).max(1.0)
        }
        _ => 1.0,
    };
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, v);
    v
}

/// Pen samples (x, y, pressure, time) at 200 Hz. The S crosses the strip
/// left to right, pressure rising then falling (tapered ends); shapes are
/// drawn at full pressure, as the geometry tools paint them.
fn path(shape: Shape, w: u32, h: u32) -> Vec<(f32, f32, f32, f64)> {
    let (w, h) = (w as f32, h as f32);
    let margin = if shape == Shape::S { h * MAX_RADIUS } else { h * 0.12 } + 3.0;
    let (x0, x1, y0, y1) = (margin, w - margin, margin, h - margin);
    let (cx, cy) = ((x0 + x1) / 2.0, h / 2.0);
    let half = ((y1 - y0) * 1.6).min(x1 - x0) / 2.0;
    let tau = std::f32::consts::TAU;
    let pts: Vec<(f32, f32, f32)> = match shape {
        Shape::S => {
            let amp = (h / 2.0 - margin).max(0.0) * 0.9;
            let n = ((x1 - x0) / 3.0).ceil().max(8.0) as usize;
            (0..=n).map(|i| {
                let t = i as f32 / n as f32;
                let p = 0.1 + 0.9 * (t * std::f32::consts::PI).sin().powf(0.8);
                (x0 + t * (x1 - x0), cy - amp * (t * tau).sin(), p)
            }).collect()
        }
        Shape::Line => polyline(&[(cx - half, y1), (cx + half, y0)]),
        Shape::Rectangle => polyline(&[(cx - half, y0), (cx + half, y0), (cx + half, y1), (cx - half, y1), (cx - half, y0)]),
        Shape::Ellipse => (0..=96).map(|i| {
            let a = i as f32 / 96.0 * tau;
            (cx + half * a.cos(), cy + (y1 - y0) / 2.0 * a.sin(), 1.0)
        }).collect(),
        Shape::Arc => (0..=64).map(|i| {
            // A quadratic curve bulging upward.
            let t = i as f32 / 64.0;
            let u = 1.0 - t;
            let (ax, bx, ky) = (cx - half, cx + half, y0 - (y1 - y0));
            (u * u * ax + 2.0 * u * t * cx + t * t * bx, u * u * y1 + 2.0 * u * t * ky + t * t * y1, 1.0)
        }).collect(),
    };
    pts.into_iter().enumerate().map(|(i, (x, y, p))| (x, y, p, i as f64 * 0.005)).collect()
}

/// Points every ~2 px along the segments, full pressure.
fn polyline(corners: &[(f32, f32)]) -> Vec<(f32, f32, f32)> {
    let mut out = vec![(corners[0].0, corners[0].1, 1.0)];
    for w in corners.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = (((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() / 2.0).ceil().max(1.0) as usize;
        out.extend((1..=n).map(|i| { let t = i as f32 / n as f32; (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, 1.0) }));
    }
    out
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
            coverage(&render(&ForgeBrush::round(), &b, Shape::S, Mode::List, 240, 64, 1.0))
        }).collect();
        assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?}");
        assert!(shown_radius(1000.0, 64, 1.0) <= 64.0 * MAX_RADIUS + 0.01);
        assert_eq!(shown_radius(2.0, 64, 2.0), 4.0);

        // Soft: lighter edges, so less full coverage.
        b.radius = 300.0;
        let full = |px: &[u8]| px.chunks(4).filter(|p| p[3] > 250).count();
        let hard = full(&render(&ForgeBrush::round(), &b, Shape::S, Mode::List, 240, 64, 1.0));
        b.hardness = 0.0;
        let soft = full(&render(&ForgeBrush::round(), &b, Shape::S, Mode::List, 240, 64, 1.0));
        assert!((soft as f32) < hard as f32 * 0.6, "hard {hard} soft {soft}");
    }

    #[test]
    fn every_bundled_brush_draws_something_in_both_modes() {
        let lib = Library::load(None);
        let mut detail_blank = FlatSurface::new(160, 48, [255, 255, 255, 0]);
        paper(&mut detail_blank);
        for p in &lib.presets {
            let list = render(&p.brush, &p.settings, Shape::S, Mode::List, 160, 48, 1.0);
            assert!(coverage(&list) > 20, "{} / {}: empty list preview", p.medium, p.name);
            let detail = render(&p.brush, &p.settings, Shape::S, Mode::Detail, 160, 48, 1.0);
            assert!(detail != detail_blank.pixels, "{} / {}: empty detail preview", p.medium, p.name);
        }
    }

    #[test]
    fn shapes_stay_thin_and_follow_the_size() {
        assert_eq!(shape_thickness(1.0), 1.0);
        assert!((shape_thickness(1000.0) - 15.0).abs() < 1e-4);
        assert!(shape_thickness(100.0) < shape_thickness(500.0));
        let cov = |d: f32, shape| {
            let b = Brush { radius: d / 2.0, ..Brush::new() };
            coverage(&render(&ForgeBrush::round(), &b, shape, Mode::List, 200, 56, 1.0))
        };
        for shape in [Shape::Line, Shape::Rectangle, Shape::Ellipse, Shape::Arc] {
            assert!(cov(10.0, shape) < cov(100.0, shape) && cov(100.0, shape) < cov(1000.0, shape), "{shape:?}");
        }
    }

    #[test]
    fn eraser_cuts_the_paint_in_detail_mode() {
        let mut b = Brush::new();
        b.radius = 8.0;
        b.erase = true;
        let px = render(&ForgeBrush::round(), &b, Shape::S, Mode::Detail, 200, 60, 1.0);
        let holes = px.chunks(4).enumerate().filter(|(i, p)| (*i as u32 % 200) < 100 && p[3] < 128).count();
        let painted_right = px.chunks(4).enumerate().filter(|(i, p)| (*i as u32 % 200) >= 100 && p[3] > 0).count();
        assert!(holes > 200 && painted_right == 0, "holes {holes}, right {painted_right}");
    }


}

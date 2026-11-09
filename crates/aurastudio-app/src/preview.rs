//! The stroke preview of the brush editor (CSP's sub tool preview): an S
//! across a small transparent picture, pressed lightly, then hard, then
//! lifting, slow then fast, so that pressure, velocity and the random
//! parts show. Made on a thread of its own; the interface keeps the
//! result in its texture cache.

use aurastudio_forge::{BrushSettings, Forge, PenSample};
use aurastudio_raster::Image;

/// Pen samples along the S.
const SAMPLES: usize = 160;

/// The brush's stroke on a `w` × `h` transparent picture, straight RGBA.
/// A big brush is shown smaller (it would fill the picture); its seed is
/// fixed (the same brush, the same preview).
pub fn stroke_preview(brush: &BrushSettings, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w.max(8), h.max(8));
    let mut b = brush.clone();
    let most = (h as f32 * 0.42).max(1.0);
    if b.size > most { b.resize_with_dual_brush(most); }
    b.seed = 1;
    let mut img = Image::new(w, h, 64, [0; 4]);
    let state = img.stroke_state(None);
    let mut stroke = state.on(&mut img);
    let mut engine = Forge::new(b);
    let (m, fw, fh) = (h as f32 * 0.4, w as f32, h as f32);
    let mut time = 0.0f64;
    for k in 0..=SAMPLES {
        let t = k as f32 / SAMPLES as f32;
        let x = m + t * (fw - 2.0 * m);
        let y = fh * 0.5 - (t * std::f32::consts::TAU).sin() * fh * 0.22;
        // Light, hard, lifting; tilted more toward the end.
        let pressure = ((t * std::f32::consts::PI).sin().powf(0.8) * 1.05).min(1.0);
        let tilt = 60.0 * t;
        // Slow at first, faster after the middle (velocity).
        time += if t < 0.5 { 0.008 } else { 0.003 };
        engine.stroke_to(&mut stroke, PenSample::new(x, y, pressure, time).with_pen((tilt, 0.0), 0.0));
    }
    engine.end_stroke(&mut stroke);
    let _ = stroke.end();
    img.read(img.rect()).into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview draws something inside the picture, the same each time.
    #[test]
    fn a_preview_draws_a_stroke() {
        let b = BrushSettings { size: 900.0, ..BrushSettings::default() };
        let px = stroke_preview(&b, 300, 60);
        assert_eq!(px.len(), 300 * 60 * 4);
        assert!(px.chunks_exact(4).any(|p| p[3] > 0));
        assert_eq!(px, stroke_preview(&b, 300, 60));
    }
}

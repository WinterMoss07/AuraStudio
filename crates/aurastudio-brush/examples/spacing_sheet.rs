// Straight strokes with big brushes at several hardness values: the dab
// spacing must not show inside soft strokes (beads / ripples).
// Usage: cargo run --release -p aurastudio-brush --example spacing_sheet -- out.png
use aurastudio_brush::{Brush, DabEngine};

const W: u32 = 1400;
const H: u32 = 1300;

use aurastudio_canvas::FlatSurface as S;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "spacing.png".into());
    let mut s = S::new(W, H, [255; 4]);
    for (k, hardness) in [0.0f32, 0.5, 0.8, 1.0].iter().enumerate() {
        s.base.copy_from_slice(&s.pixels);
        s.mask.fill(0);
        let mut b = Brush::new();
        b.radius = if std::env::var("SHEET_LOOP").is_ok() { 50.0 } else { 140.0 };
        b.hardness = *hardness;
        let mut e = DabEngine::new();
        e.sync_from(&b);
        e.begin_stroke();
        let y = 160.0 + k as f32 * 320.0;
        let looped = std::env::var("SHEET_LOOP").is_ok();
        // Pen samples ~3 px apart, slight pressure wobble (like a hand).
        // SHEET_LOOP: a self-crossing loop with tight turns instead.
        for i in 0..=360 {
            let t = i as f32 / 360.0 * std::f32::consts::TAU;
            let (x, yy) = if looped {
                (700.0 + 480.0 * t.sin(), y + 90.0 * (2.0 * t).sin())
            } else {
                (180.0 + i as f32 * 3.0, y + (i as f32 * 0.01).sin() * 20.0)
            };
            let p = if std::env::var("SHEET_CONST").is_ok() { 0.75 } else { 0.75 + 0.05 * (i as f32 * 0.07).sin() };
            e.stroke_to(&mut s, x, yy, p);
        }
        e.end_stroke(&mut s);
    }
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&s.pixels).unwrap();
}

// Renders the same soft and hard strokes (a loop crossing itself, with a
// pressure ramp) with the continuous round pen and the round dabs engine,
// side by side, to compare crossings and pressure ramps.
// Usage: cargo run --release -p aurastudio-brush --example crossing_sheet -- out.png
use aurastudio_brush::{stroke_line, Brush, DabEngine};
use aurastudio_canvas::PaintSurface;

const CELL: u32 = 640;

struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
impl PaintSurface for S {
    fn width(&self) -> u32 { CELL }
    fn height(&self) -> u32 { CELL }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
}

/// A figure-eight-like path with pressure rising then falling.
fn path() -> Vec<(f32, f32, f32)> {
    let n = 400;
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let a = t * std::f32::consts::TAU;
        let x = 320.0 + 220.0 * a.sin();
        let y = 320.0 + 160.0 * (2.0 * a).sin();
        let p = (0.15 + 1.7 * t).min(1.0).min(0.2 + 2.0 * (1.0 - t));
        (x, y, p)
    }).collect()
}

fn render(dabs: bool, hardness: f32) -> S {
    let white = vec![255u8; (CELL * CELL * 4) as usize];
    let mut s = S { px: white.clone(), base: white, mask: vec![0; (CELL * CELL) as usize] };
    let mut b = Brush::new();
    b.radius = 60;
    b.hardness = hardness;
    let pts = path();
    if dabs {
        let mut e = DabEngine::new();
        e.smoother.span = 0.0;
        e.sync_from(&b);
        e.begin_stroke();
        for &(x, y, p) in &pts { e.stroke_to(&mut s, x, y, p); }
        e.end_stroke(&mut s);
    } else {
        for w in pts.windows(2) {
            stroke_line((w[0].0, w[0].1), (w[1].0, w[1].1), &b, &mut s, w[0].2, w[1].2);
        }
    }
    s
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "crossings.png".into());
    let cells = [(false, 0.0), (true, 0.0), (false, 1.0), (true, 1.0)];
    let (w, h) = (CELL * 2, CELL * 2);
    let mut img = vec![0u8; (w * h * 4) as usize];
    for (k, &(dabs, hard)) in cells.iter().enumerate() {
        let s = render(dabs, hard);
        let (ox, oy) = ((k as u32 % 2) * CELL, (k as u32 / 2) * CELL);
        for y in 0..CELL {
            let src = (y * CELL * 4) as usize;
            let dst = (((oy + y) * w + ox) * 4) as usize;
            img[dst..dst + (CELL * 4) as usize].copy_from_slice(&s.px[src..src + (CELL * 4) as usize]);
        }
    }
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&img).unwrap();
    println!("{out}: left = continuous pen, right = round dabs; top = hardness 0, bottom = hardness 1");
}

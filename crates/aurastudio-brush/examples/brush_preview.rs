// Renders test strokes into a PPM image to tune brushes without a tablet.
// Left: hokusai engine, right: swept round pen. Rows: an infinity sign at
// constant 40% pressure (self-crossing must not darken), then a wave with
// a 0 -> 1 -> 0 pressure ramp at three sizes, then a big self-overlapping
// scribble with jittery pressure.
// Usage: cargo run --release -p aurastudio-brush --example brush_preview -- out.ppm
use aurastudio_brush::{stroke_line, Brush, HokusaiEngine, PressureSmoother};
use aurastudio_canvas::PaintSurface;

struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8>, w: u32, h: u32 }
impl PaintSurface for S {
    fn width(&self) -> u32 { self.w }
    fn height(&self) -> u32 { self.h }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
}

impl S {
    fn new_stroke(&mut self) {
        self.base.copy_from_slice(&self.px);
        self.mask.fill(0);
    }
}

fn stroke(s: &mut S, e: &mut HokusaiEngine, b: &Brush, hokusai: bool, pts: &[(f32, f32, f32)]) {
    s.new_stroke();
    if hokusai {
        e.sync_from(b);
        e.begin_stroke();
        for (k, &(x, y, p)) in pts.iter().enumerate() { e.stroke_to(s, x, y, p, k as f64 * 0.005); }
        e.end_stroke(s);
    } else {
        let mut sm = PressureSmoother::new(0.5);
        let mut last = (pts[0].0, pts[0].1, sm.apply(pts[0].0, pts[0].1, pts[0].2, b.radius as f32));
        for &(x, y, p) in pts {
            let p = sm.apply(x, y, p, b.radius as f32);
            stroke_line((last.0, last.1), (x, y), b, s, last.2, p);
            last = (x, y, p);
        }
    }
}

fn main() {
    let (w, h) = (800u32, 820u32);
    let white = vec![255u8; (w * h * 4) as usize];
    let mut s = S { px: white.clone(), base: white, mask: vec![0; (w * h) as usize], w, h };
    let mut e = HokusaiEngine::new();
    if let Some(v) = std::env::args().nth(2) { e.smoother.span = v.parse().unwrap(); }

    for (col, hokusai) in [(0.0f32, true), (400.0, false)] {
        let mut b = Brush::new();
        b.radius = 14;
        let inf: Vec<_> = (0..=400).map(|i| {
            let t = i as f32 / 400.0 * std::f32::consts::TAU;
            let d = 1.0 + t.sin().powi(2);
            (col + 200.0 + 150.0 * t.cos() / d, 90.0 + 150.0 * t.sin() * t.cos() / d, 0.4)
        }).collect();
        stroke(&mut s, &mut e, &b, hokusai, &inf);

        for (row, (radius, color)) in [(4u32, [0, 0, 0]), (12, [200, 40, 40]), (30, [30, 60, 200])].iter().enumerate() {
            b.radius = *radius;
            b.color = *color;
            let y0 = 230.0 + row as f32 * 100.0;
            let pts: Vec<_> = (0..=300).map(|i| {
                let t = i as f32 / 300.0;
                (col + 30.0 + t * 340.0, y0 + (t * 12.0).sin() * 20.0, 1.0 - (2.0 * t - 1.0).abs())
            }).collect();
            stroke(&mut s, &mut e, &b, hokusai, &pts);
        }

        // Big brush, hand-like jittery pressure, self-overlapping scribble:
        // the case where per-dab rings / blotches would show.
        b.radius = 60;
        b.color = [0, 0, 0];
        let mut seed = 12345u32;
        let mut p = 0.5f32;
        let pts: Vec<_> = (0..=500).map(|i| {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let noise = ((seed >> 16) & 0x7fff) as f32 / 32767.0 - 0.5;
            p = (p + noise * 0.15).clamp(0.15, 0.9);
            let p = p * (i as f32 / 60.0).min(1.0);
            let t = i as f32 / 500.0 * std::f32::consts::TAU * 2.0;
            (col + 200.0 + t.cos() * 110.0 + (t * 0.5).sin() * 30.0, 650.0 + (t * 1.5).sin() * 90.0, p)
        }).collect();
        stroke(&mut s, &mut e, &b, hokusai, &pts);
    }

    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
    for px in s.px.chunks_exact(4) { out.extend_from_slice(&px[..3]); }
    std::fs::write(std::env::args().nth(1).expect("output path"), out).unwrap();
}

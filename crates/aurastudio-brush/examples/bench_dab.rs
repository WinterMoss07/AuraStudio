// Cost of single big dabs (no pen path), for profiling the dab engine.
use std::time::Instant;
use aurastudio_brush::{Brush, DabEngine};
use aurastudio_canvas::PaintSurface;
struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
impl PaintSurface for S {
    fn width(&self) -> u32 { 2048 }
    fn height(&self) -> u32 { 2048 }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
}
fn main() {
    let white = vec![255u8; 2048 * 2048 * 4];
    let mut s = S { px: white.clone(), base: white, mask: vec![0; 2048 * 2048] };
    s.mask.fill(1); s.mask.fill(0);
    let mut b = Brush::new();
    b.radius = 500;
    b.hardness = 0.0;
    let mut e = DabEngine::new();
    e.smoother.span = 0.0;
    e.sync_from(&b);
    let n = 40;
    let t0 = Instant::now();
    for i in 0..n {
        e.begin_stroke();
        s.mask.fill(0);
        // Dabs far apart along x: each is a fresh full dab.
        e.stroke_to(&mut s, 1000.0 + (i % 3) as f32, 1000.0, 1.0);
    }
    let dt = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let px = std::f64::consts::PI * 500.5f64.powi(2);
    println!("dab r=500 soft: {dt:.2} ms ({:.1} ns/px)", dt * 1e6 / px);
}

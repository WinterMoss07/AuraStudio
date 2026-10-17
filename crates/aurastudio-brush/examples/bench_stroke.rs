// Measures CPU cost per pointer sample of the painting path, without a
// window: hokusai dabs + layer composite of the touched rect.
// Usage: cargo run --release -p aurastudio-brush --example bench_stroke
use std::time::{Duration, Instant};
use aurastudio_brush::{stroke_line, Brush, HokusaiEngine};
use aurastudio_canvas::PaintSurface;

struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8>, w: u32, h: u32 }
impl PaintSurface for S {
    fn width(&self) -> u32 { self.w }
    fn height(&self) -> u32 { self.h }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
}

fn main() {
    let (w, h) = (2048u32, 2048u32);
    // Optional filters: BENCH_RADIUS=500 BENCH_ENGINE=simple|hokusai
    let only_r: Option<u32> = std::env::var("BENCH_RADIUS").ok().and_then(|v| v.parse().ok());
    let only_e = std::env::var("BENCH_ENGINE").ok();
    for radius in [10u32, 50, 130, 250, 500] {
        if only_r.is_some_and(|r| r != radius) { continue; }
        for hokusai in [true, false] {
            if only_e.as_deref().is_some_and(|e| (e == "hokusai") != hokusai) { continue; }
            let white = vec![255u8; (w * h * 4) as usize];
            let mut s = S { px: white.clone(), base: white, mask: vec![0; (w * h) as usize], w, h };
            // Touch every page first: a fresh buffer costs a page fault on
            // first write, which made timings depend on what ran before. The
            // app reuses its buffers, so warm memory is what it sees.
            s.mask.fill(1);
            s.mask.fill(0);
            let mut b = Brush::new();
            b.radius = radius;
            let mut e = HokusaiEngine::new();
            e.sync_from(&b);
            e.begin_stroke();
            let n = 200;
            let mut total = Duration::ZERO;
            let mut worst = Duration::ZERO;
            let mut last = (300.0f32, 1000.0f32);
            let mut last_p = 0.3f32;
            for i in 0..n {
                // ~8 px between samples: a fast-ish stroke at 200 Hz.
                let t = i as f32 / n as f32;
                let p = (300.0 + i as f32 * 8.0, 1000.0 + (t * 20.0).sin() * 300.0);
                let pr = 0.3 + 0.7 * (t * 9.0).sin().abs();
                let t0 = Instant::now();
                if hokusai { e.stroke_to(&mut s, p.0, p.1, pr); } else { stroke_line(last, p, &b, &mut s, last_p, pr); }
                last_p = pr;
                let dt = t0.elapsed();
                total += dt; worst = worst.max(dt); last = p;
            }
            println!("radius {radius:>3} {:<8} avg {:>7.3} ms/sample  worst {:>7.3} ms",
                if hokusai { "hokusai" } else { "simple" },
                total.as_secs_f64() * 1000.0 / n as f64, worst.as_secs_f64() * 1000.0);
        }
    }
}

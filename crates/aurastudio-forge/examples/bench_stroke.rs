// Measures CPU cost per pointer sample of the painting path, without a
// window: the round brush.
// Usage: cargo run --release -p aurastudio-forge --example bench_stroke
use std::time::{Duration, Instant};
use aurastudio_forge::{Brush, Forge, ForgeBrush, PenSample};

use aurastudio_core::FlatSurface as S;

fn main() {
    let (w, h) = (2048u32, 2048u32);
    // Optional filter: BENCH_RADIUS=500
    let only_r: Option<u32> = std::env::var("BENCH_RADIUS").ok().and_then(|v| v.parse().ok());
    // BENCH_PRESSURE=steady: near-constant pressure (typical painting);
    // default: pressure swinging all the time (worst case: every rise
    // repaints the whole disc).
    let steady = std::env::var("BENCH_PRESSURE").is_ok_and(|v| v == "steady");
    // BENCH_HARDNESS=0..1 (default 1 = hard edge).
    let hardness: f32 = std::env::var("BENCH_HARDNESS").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    for radius in [10u32, 50, 130, 250, 500] {
        if only_r.is_some_and(|r| r != radius) { continue; }
        {
            let mut s = S::new(w, h, [255; 4]);
            // Touch every page first: a fresh buffer costs a page fault on
            // first write, which made timings depend on what ran before. The
            // app reuses its buffers, so warm memory is what it sees.
            s.mask.fill(1);
            s.mask.fill(0);
            let mut b = Brush::new();
            b.radius = radius as f32;
            b.hardness = hardness;
            let mut d = Forge::new(ForgeBrush::round());
            d.sync_from(&b);
            d.begin_stroke();
            let n = 200;
            let mut total = Duration::ZERO;
            let mut worst = Duration::ZERO;
            for i in 0..n {
                // ~8 px between samples: a fast-ish stroke at 200 Hz.
                let t = i as f32 / n as f32;
                let p = (300.0 + i as f32 * 8.0, 1000.0 + (t * 20.0).sin() * 300.0);
                let pr = if steady { 0.8 + 0.02 * (t * 9.0).sin() } else { 0.3 + 0.7 * (t * 9.0).sin().abs() };
                let t0 = Instant::now();
                d.stroke_to(&mut s, PenSample::new(p.0, p.1, pr, i as f64 * 0.005));
                let dt = t0.elapsed();
                total += dt; worst = worst.max(dt);
            }
            println!("radius {radius:>3}  avg {:>7.3} ms/sample  worst {:>7.3} ms",
                total.as_secs_f64() * 1000.0 / n as f64, worst.as_secs_f64() * 1000.0);
        }
    }
}

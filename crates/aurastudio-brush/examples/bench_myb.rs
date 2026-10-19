// Measures CPU cost per pointer sample of a MyPaint brush at a given size.
// Usage: cargo run --release -p aurastudio-brush --example bench_myb -- brush.myb [radius]
use std::time::{Duration, Instant};
use aurastudio_brush::{parse_myb, Brush, MyPaintEngine};

use aurastudio_canvas::FlatSurface as S;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let preset = parse_myb(&std::fs::read_to_string(&args[0]).unwrap()).unwrap();
    let radius: f32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(150.0);
    let (w, h) = (2048u32, 2048u32);
    let mut s = S::new(w, h, [255; 4]);
    let mut e = MyPaintEngine::new(preset);
    let mut b = Brush::new();
    b.color = [40, 60, 120];
    b.radius = radius;
    e.sync_from(&b);
    e.begin_stroke();
    let n = 200;
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for i in 0..n {
        // ~8 px between samples at 200 Hz, steady pressure.
        let t = i as f32 / n as f32;
        let p = (300.0 + i as f32 * 8.0, 1000.0 + (t * 20.0).sin() * 300.0);
        let t0 = Instant::now();
        e.stroke_to(&mut s, aurastudio_brush::PenSample::new(p.0, p.1, 0.8, i as f64 * 0.005));
        let dt = t0.elapsed();
        total += dt;
        worst = worst.max(dt);
    }
    e.end_stroke(&mut s);
    println!("radius {radius}  avg {:.3} ms/sample (pen 5 ms)  worst {:.3} ms",
        total.as_secs_f64() * 1000.0 / n as f64, worst.as_secs_f64() * 1000.0);
}

//! Time per pen sample of a big brush on a 2000 × 3000 surface.
//! cargo run --release -p aurastudio-forge-library --example big_brush -- Oil 400

use std::time::Instant;
use aurastudio_core::FlatSurface;
use aurastudio_forge::{Brush, Forge, PenSample};
use aurastudio_forge_library::Library;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).map_or("Oil", String::as_str);
    let radius: f32 = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(400.0);
    let lib = Library::load_bundled();
    let forge = lib.presets.iter().find(|p| p.name == name).expect("brush").brush.clone();
    let mut s = FlatSurface::new(2000, 3000, [0; 4]);
    let mut brush = Brush::new();
    brush.radius = radius;
    let mut e = Forge::new((*forge).clone());
    e.sync_from(&brush);
    e.begin_stroke();
    let mut times = Vec::new();
    for i in 0..120 {
        let t = i as f32 / 120.0;
        let p = PenSample::new(300.0 + i as f32 * 10.0, 1500.0 + (t * 9.0).sin() * 500.0, 0.4 + 0.6 * (t * 7.0).sin().abs(), i as f64 * 0.005);
        let t0 = Instant::now();
        e.stroke_to(&mut s, p);
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let avg = times.iter().sum::<f64>() / times.len() as f64;
    let max = times.iter().cloned().fold(0.0, f64::max);
    println!("{name} {} px: avg {avg:.2} max {max:.1} ms per sample", radius * 2.0);
    if std::env::var("SLOWEST").is_ok() { let mut v: Vec<(usize, f64)> = times.iter().copied().enumerate().collect(); v.sort_by(|a, b| b.1.total_cmp(&a.1)); println!("{:?}", &v[..8]); }
}

// Time of every filter on an A4 300 dpi layer (defaults, and a 100 px blur).
// Usage: cargo run --release -p aurastudio-filters --example bench_filters
use aurastudio_filters::{Filter, Kind, GROUPS};

fn main() {
    let (w, h) = (2480usize, 3508usize);
    let src: Vec<u8> = (0..w * h).flat_map(|i| { let (x, y) = (i % w, i / w); [(x % 256) as u8, (y % 256) as u8, 128, if (x / 64 + y / 64) % 3 == 0 { 0 } else { 255 }] }).collect();
    let mut all: Vec<Filter> = GROUPS.iter().flat_map(|g| g.1.iter().map(|&k| Filter::new(k))).collect();
    all.push(Filter { values: vec![100.0], ..Filter::new(Kind::GaussianBlur) });
    all.push(Filter { values: vec![20.0], ..Filter::new(Kind::Dilate) });
    for f in all {
        let t = std::time::Instant::now();
        let out = f.apply(&src, w, h);
        println!("{:24} {:?} {:6.0} ms", f.kind.name(), f.values, t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(out);
    }
}

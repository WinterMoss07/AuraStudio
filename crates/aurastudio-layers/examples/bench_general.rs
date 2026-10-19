// The general merge (clipping, filter layers) against the plain one.
// Usage: cargo run --release -p aurastudio-layers --example bench_general
use aurastudio_layers::{LayerStack, Pick};
use std::time::Instant;

fn main() {
    let (w, h) = (2480u32, 3508u32);
    let mut s = LayerStack::new_document(w, h);
    for i in 0..20 {
        let id = s.add_layer(&format!("l{i}"));
        let r = (i * 50, i * 70, i * 50 + 1200, i * 70 + 1600);
        let n = ((r.2 - r.0) * (r.3 - r.1)) as usize;
        s.set_layer_rect(id, r, &[(i * 10) as u8, 80, 160, 200].repeat(n));
    }
    let rect = (600, 600, 1112, 1112);
    let t = Instant::now();
    for _ in 0..5 { std::hint::black_box(s.composite_rect(rect)); }
    println!("plain 512²: {:.1} ms", t.elapsed().as_secs_f64() * 200.0);
    let mut out = vec![0u8; 512 * 512 * 4];
    let t = Instant::now();
    for _ in 0..5 { std::hint::black_box(s.composite_pick(rect, Pick { skip: 1, ..Pick::ALL }, false)); }
    println!("general 512² (20 layers): {:.1} ms", t.elapsed().as_secs_f64() * 200.0);
    let _ = &mut out;
}

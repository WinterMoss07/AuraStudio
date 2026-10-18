// Cost of merging layers into the projection, per pixel.
// Usage: cargo run --release -p aurastudio-layers --example bench_composite
use std::time::Instant;
use aurastudio_layers::LayerStack;

fn main() {
    let (w, h) = (2048u32, 2048u32);
    let mut stack = LayerStack::new(w, h);
    let id = stack.add_layer("ink");
    // Half-covered ink layer: a realistic mix of empty and painted pixels.
    let n = (w * h / 2) as usize;
    stack.set_layer_rect(id, (0, 0, w, h / 2), &[30u8, 60, 90, 200].repeat(n));
    stack.add_layer("empty");
    let mut out = vec![0u8; 1000 * 1000 * 4];
    out.fill(1);
    let rect = (0, 0, 1000, 1000);
    for _ in 0..3 { stack.composite_rect_to(rect, &mut out); }
    let runs = 20;
    let t = Instant::now();
    for _ in 0..runs { stack.composite_rect_to(rect, &mut out); }
    let per = t.elapsed().as_secs_f64() / runs as f64;
    println!("composite 1000x1000, 3 layers: {:.2} ms ({:.1} ns/px)", per * 1000.0, per * 1e9 / 1e6);
}

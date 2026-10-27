// Renders a sample sheet of MyPaint brushes through Forge (the
// exact path the app uses), one row per brush, pressure rising left to
// right, on a white layer. Checks .myb support by eye.
// Usage: cargo run --release -p aurastudio-forge --example myb_sheet -- out.png a.myb b.myb ...
use aurastudio_forge::{parse_myb, Brush, Forge};

use aurastudio_canvas::FlatSurface as S;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = &args[0];
    let files = &args[1..];
    let (w, row_h) = (900u32, 90u32);
    let h = row_h * files.len() as u32;
    let mut s = S::new(w, h, [255; 4]);
    for (k, f) in files.iter().enumerate() {
        let json = std::fs::read_to_string(f).unwrap();
        let preset = match parse_myb(&json) { Ok(b) => b, Err(e) => { eprintln!("{f}: {e:?}"); continue } };
        let size = Forge::native_size(&preset);
        let mut e = Forge::new(preset);
        let mut b = Brush::new();
        b.color = [40, 60, 120];
        b.radius = (size as f32 / 2.0).clamp(2.0, 30.0);
        e.sync_from(&b);
        s.base.copy_from_slice(&s.pixels);
        e.begin_stroke();
        let y0 = k as f32 * row_h as f32 + row_h as f32 / 2.0;
        for i in 0..=300 {
            let t = i as f32 / 300.0;
            let x = 40.0 + t * (w as f32 - 80.0);
            let y = y0 + (t * 12.0).sin() * 18.0;
            e.stroke_to(&mut s, aurastudio_forge::PenSample::new(x, y, 0.15 + 0.85 * t, i as f64 * 0.006));
        }
        e.end_stroke(&mut s);
        println!("{f}: native size {size}px");
    }
    let file = std::fs::File::create(out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&s.pixels).unwrap();
}

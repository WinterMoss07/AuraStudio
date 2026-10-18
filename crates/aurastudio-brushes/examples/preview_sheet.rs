// Every brush preview on the dockers' background, to check them by eye.
// Usage: cargo run --release -p aurastudio-brushes --example preview_sheet -- out.png [filter]
use aurastudio_brushes::{preview, Library};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter = args.get(1).map(|s| s.to_lowercase());
    let lib = Library::load();
    let presets: Vec<_> = lib.presets.iter().filter(|p| filter.as_ref().is_none_or(|f| p.name.to_lowercase().contains(f) || p.group.to_lowercase().contains(f))).collect();
    let (w, h, cols) = (360u32, 72u32, 4u32);
    let rows = (presets.len() as u32).div_ceil(cols);
    let (sw, sh) = (cols * (w + 6), rows * (h + 6));
    let mut out = [0x22u8, 0x22, 0x23, 255].repeat((sw * sh) as usize);
    let t = std::time::Instant::now();
    for (k, p) in presets.iter().enumerate() {
        let px = preview::render(&p.engine, &p.settings, w, h, 2.0);
        let (ox, oy) = ((k as u32 % cols) * (w + 6), (k as u32 / cols) * (h + 6));
        for y in 0..h {
            for x in 0..w {
                let s = &px[((y * w + x) * 4) as usize..][..4];
                let d = &mut out[(((oy + y) * sw + ox + x) * 4) as usize..][..4];
                let a = s[3] as u32;
                for c in 0..3 { d[c] = ((s[c] as u32 * a + d[c] as u32 * (255 - a)) / 255) as u8; }
            }
        }
    }
    eprintln!("{} previews in {:?}", presets.len(), t.elapsed());
    let f = std::fs::File::create(&args[0]).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), sw, sh);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&out).unwrap();
}

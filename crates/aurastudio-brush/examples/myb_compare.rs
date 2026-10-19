// Same strokes as tools/mypaint-reference/ref.c (the real libmypaint), painted by Forge's
// MyPaint path directly (no app settings applied): compares .myb fidelity.
// Usage: cargo run --release -p aurastudio-brush --example myb_compare -- brush.myb out.ppm
// (Constants written as in ref.c, for the same floats.)
#![allow(clippy::approx_constant, clippy::needless_range_loop)]
use std::collections::HashMap;

use aurastudio_brush::forge::tile::{TilePixels, TILE_SIZE};
use aurastudio_brush::forge::{BrushSetting, BrushState, TiledSurface};

const W: i32 = 640;
const H: i32 = 220;

struct Tiles(HashMap<(i32, i32), Box<TilePixels>>);

impl TiledSurface for Tiles {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.0.entry((tx, ty)).or_insert_with(|| Box::new([[[0; 4]; TILE_SIZE]; TILE_SIZE]))
    }
    fn tile_request_end(&mut self, _: i32, _: i32) {}
    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> { self.0.get(&(tx, ty)).map(|b| &**b) }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut b = aurastudio_brush::parse_myb(&std::fs::read_to_string(&a[0]).unwrap()).unwrap();
    b.get_mut(BrushSetting::ColorH).base_value = 0.62;
    b.get_mut(BrushSetting::ColorS).base_value = 0.8;
    b.get_mut(BrushSetting::ColorV).base_value = 0.55;
    let mut s = Tiles(HashMap::new());
    for ty in 0..(H + 63) / 64 {
        for tx in 0..(W + 63) / 64 {
            let t = s.tile_request_start(tx, ty);
            for y in 0..64 {
                for x in 0..64 {
                    let (xx, yy) = (tx * 64 + x as i32, ty * 64 + y as i32);
                    if (120..200).contains(&yy) {
                        t[y][x] = [((xx % 256) as f64 / 255.0 * 32768.0) as u16, (0.45 * 32768.0) as u16,
                            ((255 - xx % 256) as f64 / 255.0 * 32768.0) as u16, 32768];
                    }
                }
            }
        }
    }
    for k in 0..2 {
        let mut st = BrushState::default();
        let n = if k == 0 { 160 } else { 60 };
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let (x, y, p) = if k == 0 {
                (40.0 + t * 560.0, 80.0 - 50.0 * (t * 6.2831853).sin(), (t * 3.1415926).sin().max(0.05))
            } else {
                (40.0 + t * 560.0, 160.0, 0.8)
            };
            b.stroke_to(&mut st, &mut s, x, y, p, 0.0, 0.0, if i == 0 { 1.0 } else { 0.008 });
        }
    }
    let mut img = vec![255u8; (W * H * 3) as usize];
    for (&(tx, ty), t) in &s.0 {
        for y in 0..64 {
            for x in 0..64 {
                let (xx, yy) = (tx * 64 + x, ty * 64 + y);
                if xx < 0 || yy < 0 || xx >= W || yy >= H { continue; }
                let p = t[y as usize][x as usize];
                let al = p[3] as f64 / 32768.0;
                for c in 0..3 {
                    let v = p[c] as f64 / 32768.0 + (1.0 - al);
                    img[((yy * W + xx) * 3 + c as i32) as usize] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    let mut out = format!("P6\n{W} {H}\n255\n").into_bytes();
    out.extend_from_slice(&img);
    std::fs::write(&a[1], out).unwrap();
}

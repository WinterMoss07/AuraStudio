// Rend des traits test (gauche: hokusai, droite: brush simple, pression
// en rampe 0→1→0) dans une image PPM, pour régler les pinceaux sans
// tablette. Usage: cargo run --release -p aurastudio-brush --example brush_preview -- out.ppm
use aurastudio_brush::{Brush, HokusaiEngine, stroke_line};
use aurastudio_canvas::PaintSurface;
struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8>, w: u32, h: u32 }
impl PaintSurface for S {
    fn width(&self) -> u32 { self.w } fn height(&self) -> u32 { self.h }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
}
fn main() {
    let (w, h) = (800u32, 400u32);
    let white = vec![255u8; (w*h*4) as usize];
    let mut s = S { px: white.clone(), base: white, mask: vec![0; (w*h) as usize], w, h };
    let mut e = HokusaiEngine::new();
    for (row, (radius, color)) in [(4u32,[0,0,0]),(12,[200,40,40]),(30,[30,60,200])].iter().enumerate() {
        let mut b = Brush::new(); b.radius = *radius; b.color = *color;
        let y0 = 70.0 + row as f32 * 110.0;
        e.sync_from(&b);
        s.base.copy_from_slice(&s.px.clone()); s.mask.fill(0);
        e.begin_stroke();
        for i in 0..=300 { let t = i as f32/300.0;
            let p = 1.0 - (2.0*t-1.0).abs();
            e.stroke_to(&mut s, 30.0 + t*360.0, y0 + (t*12.0).sin()*20.0, p); }
        e.end_stroke(&mut s);
        s.base.copy_from_slice(&s.px.clone()); s.mask.fill(0);
        let mut last = (430.0, y0);
        for i in 0..=300 { let t = i as f32/300.0; let p = 1.0 - (2.0*t-1.0).abs();
            let pt = (430.0 + t*340.0, y0 + (t*12.0).sin()*20.0);
            stroke_line(last, pt, &b, &mut s, p); last = pt; }
    }
    let mut out = format!("P6 {w} {h} 255\n").into_bytes();
    for px in s.px.chunks_exact(4) { out.extend_from_slice(&px[..3]); }
    std::fs::write(std::env::args().nth(1).unwrap(), out).unwrap();
}

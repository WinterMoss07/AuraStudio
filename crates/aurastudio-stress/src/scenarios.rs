//! The scenarios. Each rebuilt crate brings its own (painting, layers,
//! view, history…); until the brush engine is back, `raster stroke`
//! paints with a plain round dab.

use aurastudio_core::blend::paint_pixel_u8;
use aurastudio_core::{BlendMode, DocType, PaintSurface, Rect, Rng, Span};
use aurastudio_diag::report::Report;
use aurastudio_raster::Image;

use crate::Scenario;

/// Nothing to do for a while: what wakes up, what allocates. For now the
/// process alone (the app's event loop joins it with the app crate).
pub struct Idle { pub millis: u64 }

impl Scenario for Idle {
    fn name(&self) -> &'static str { "idle" }
    fn run(&mut self, _report: &mut Report) {
        // (No zone around the wait: a zone is work, and waiting is not.)
        std::thread::sleep(std::time::Duration::from_millis(self.millis));
    }
}

/// Long strokes of round dabs on a surface of each document type's size
/// and tile size, then undo / redo: the cost of the stroke path in
/// `raster` alone (prepare, spans, end, patch), and its allocations.
pub struct RasterStroke { pub kind: DocType, pub w: u32, pub h: u32, pub strokes: u32, pub radius: f32 }

impl Scenario for RasterStroke {
    fn name(&self) -> &'static str {
        match self.kind { DocType::Animation => "raster stroke (animation 4K, 16 px tiles)", DocType::Comics => "raster stroke (manga B4 600 dpi, 32 px tiles)", DocType::Illustration => "raster stroke (illustration A4 300 dpi, 128 px tiles)" }
    }

    fn run(&mut self, _report: &mut Report) {
        let mut s = Image::new(self.w, self.h, self.kind.default_tile_size(), [255, 255, 255, 0]);
        let mut rng = Rng::new(1);
        let mut patches = Vec::new();
        let mut spans = Vec::new();
        for _ in 0..self.strokes {
            let (mut x, mut y) = (rng.range(0.0, self.w as f32), rng.range(0.0, self.h as f32));
            // Across about half the canvas, staying on it.
            let step = (self.w.min(self.h) as f32 / 400.0).min(self.radius * 0.25);
            let (dx, dy) = (rng.range(-1.0, 1.0) * step, rng.range(-1.0, 1.0) * step);
            let color = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8];
            aurastudio_core::zone!("stroke");
            let mut st = s.begin_stroke(None);
            for _ in 0..200 {
                aurastudio_core::zone!("dab");
                let r = self.radius;
                let rect = Rect::around((x - r) as f64, (y - r) as f64, (x + r) as f64, (y + r) as f64);
                let mut buf = aurastudio_core::surface::recycle_spans(std::mem::take(&mut spans));
                st.spans_mut(rect, &mut buf);
                for sp in buf.drain(..) { round(sp, (x, y), r, color); }
                spans = aurastudio_core::surface::recycle_spans(buf);
                x = (x + dx).clamp(0.0, self.w as f32);
                y = (y + dy).clamp(0.0, self.h as f32);
            }
            aurastudio_core::zone!("end stroke");
            patches.push(st.end());
        }
        aurastudio_core::zone!("undo redo");
        for p in patches.iter_mut().rev() { p.swap(&mut s); }
        for p in patches.iter_mut() { p.swap(&mut s); }
        aurastudio_core::count!("stroke tiles kept for undo", patches.iter().map(|p| p.len()).sum::<usize>());
    }
}

/// A round dab, coverage falling at the edge, by the `PaintSurface` rule.
fn round(mut s: Span<'_>, (cx, cy): (f32, f32), r: f32, color: [u8; 3]) {
    let y = s.y as f32 + 0.5 - cy;
    for i in 0..s.len() {
        let x = s.x as f32 + i as f32 + 0.5 - cx;
        let d = (x * x + y * y).sqrt();
        let m = s.cover(i, aurastudio_core::math::to_u8((r - d).clamp(0.0, 1.0)));
        if m > 0 { s.pixels[i] = paint_pixel_u8(s.base[i], color, m, BlendMode::Normal); }
    }
}

/// A page of strokes, then save it, paint a little, save again (the
/// second save reuses the unchanged tiles), and load it back.
pub struct RasterFile { pub w: u32, pub h: u32 }

impl Scenario for RasterFile {
    fn name(&self) -> &'static str { "raster save / load (A3 300 dpi, 64 px tiles)" }
    fn run(&mut self, report: &mut Report) {
        let mut s = Image::new(self.w, self.h, 64, [255, 255, 255, 0]);
        let mut rng = Rng::new(5);
        for _ in 0..60 {
            let r = Rect::from_xywh(rng.below(self.w as u64) as i32, rng.below(self.h as u64) as i32, 400, 300);
            s.fill_rect(r, [rng.below(256) as u8, rng.below(256) as u8, 90, 255]);
        }
        let mut st = s.begin_stroke(None);
        for i in 0..400 { let x = 100.0 + i as f32 * 8.0; let rect = Rect::around((x - 40.0) as f64, 1960.0, (x + 40.0) as f64, 2040.0); let mut buf = Vec::new(); st.spans_mut(rect, &mut buf); for sp in buf { round(sp, (x, 2000.0), 40.0, [10, 10, 10]); } }
        st.end();
        let mut cache = aurastudio_raster::SaveCache::default();
        let bytes = { aurastudio_core::zone!("first save"); s.save_with(&mut cache).0 };
        s.fill_rect(Rect::new(10, 10, 60, 60), [1, 2, 3, 255]);
        let (_, stats) = { aurastudio_core::zone!("second save (one change)"); s.save_with(&mut cache) };
        if stats.encoded > 4 { report.suspect(self.name(), "save", format!("second save encoded {} tiles for one small change", stats.encoded), "the save cache should reuse unchanged tiles"); }
        aurastudio_core::zone!("load");
        let back = Image::load(&bytes, 20_000);
        if back.is_none() { report.suspect(self.name(), "save", "the saved surface does not load".into(), "raster save / load"); }
        aurastudio_core::count!("saved bytes", bytes.len());
    }
}

/// Line art (one color, soft edges) on an animation sheet: how much the
/// coverage form saves against pixels.
pub struct RasterLineArt;

impl Scenario for RasterLineArt {
    fn name(&self) -> &'static str { "raster line art (HD sheet, 16 px tiles)" }
    fn run(&mut self, report: &mut Report) {
        use aurastudio_core::HeapSize;
        let mut s = Image::new(1920, 1080, 16, [255, 255, 255, 0]);
        let mut rng = Rng::new(9);
        for _ in 0..40 {
            let (mut x, mut y) = (rng.range(100.0, 1800.0), rng.range(100.0, 980.0));
            let (dx, dy) = (rng.range(-2.0, 2.0), rng.range(-2.0, 2.0));
            let mut st = s.begin_stroke(None);
            for _ in 0..300 {
                let rect = Rect::around((x - 3.0) as f64, (y - 3.0) as f64, (x + 3.0) as f64, (y + 3.0) as f64);
                let mut buf = Vec::new();
                st.spans_mut(rect, &mut buf);
                for sp in buf { round(sp, (x, y), 2.5, [20, 20, 30]); }
                x = (x + dx).clamp(0.0, 1919.0);
                y = (y + dy).clamp(0.0, 1079.0);
            }
            st.end();
        }
        let st = s.stored();
        aurastudio_core::count!("line art tiles: coverage form", st.alpha);
        aurastudio_core::count!("line art tiles: pixels", st.full);
        aurastudio_core::count!("line art bytes", s.heap_bytes());
        if st.full > st.alpha { report.suspect(self.name(), "forms", format!("{} pixel tiles for {} coverage tiles on one-color line art", st.full, st.alpha), "settle() should store one-color tiles as coverage"); }
    }
}

/// Every scenario, in report order./// Every scenario, in report order.
pub fn all() -> Vec<Box<dyn Scenario>> {
    let n = crate::env_u64("STROKES", 20) as u32;
    vec![
        Box::new(Idle { millis: crate::env_u64("IDLE_MS", 2000) }),
        Box::new(RasterStroke { kind: DocType::Animation, w: 3840, h: 2160, strokes: n, radius: 8.0 }),
        Box::new(RasterStroke { kind: DocType::Comics, w: 5906, h: 8339, strokes: n, radius: 12.0 }),
        Box::new(RasterStroke { kind: DocType::Illustration, w: 2480, h: 3508, strokes: n, radius: 150.0 }),
        Box::new(RasterFile { w: 3508, h: 4961 }),
        Box::new(RasterLineArt),
    ]
}

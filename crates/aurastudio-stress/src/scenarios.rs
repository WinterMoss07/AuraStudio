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
fn round(s: Span<'_>, (cx, cy): (f32, f32), r: f32, color: [u8; 3]) {
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

/// Every filter on a half-painted A4 300 dpi layer (128 px tiles: a
/// painted band, a flat color band, line art, the rest empty), then a
/// large blur inside a selection and a selection's feather: what each
/// costs, and whether the one-color tiles stay one color.
pub struct Filters;

impl Scenario for Filters {
    fn name(&self) -> &'static str { "filters (illustration A4 300 dpi, 128 px tiles)" }
    fn run(&mut self, report: &mut Report) {
        use aurastudio_filters::{blur_mask, Filter, Kind};
        let (w, h) = (2480, 3508);
        let mut base = Image::new(w, h, 128, [255, 255, 255, 0]);
        let band = Rect::new(0, 0, w as i32, 1200);
        let px: Vec<[u8; 4]> = (0..band.area() as usize).map(|i| {
            let (x, y) = (i % w as usize, i / w as usize);
            [(x % 256) as u8, (y % 256) as u8, 128, if (x / 64 + y / 64) % 3 == 0 { 0 } else { 255 }]
        }).collect();
        base.write(band, &px);
        base.fill_rect(Rect::new(0, 1200, w as i32, 1800), [200, 180, 150, 255]);
        let ink: Vec<u8> = (0..(w as usize * 400)).map(|i| if (i % w as usize) % 37 < 3 { 255 } else { 0 }).collect();
        base.fill_mask(Rect::new(0, 1800, w as i32, 2200), &ink, [20, 20, 30, 255]);
        // (Whole tiles: the frame's last column is only partly the band's.)
        let flat = Rect::new(0, 1280, 2432, 1792);
        let flat_before = base.uniform_in(flat).is_some();
        for k in Kind::all() {
            let mut s = base.clone();
            let f = Filter::new(k);
            {
                aurastudio_core::zone!(k.name());
                f.apply_to(&mut s, None);
            }
            if k.is_point() && flat_before && s.uniform_in(flat).is_none() {
                report.suspect(self.name(), k.name(), "a one-color band came out as pixels".into(), "layer::Job::piece should map one-color tiles once");
            }
        }
        let mut sel = aurastudio_raster::Mask::new(w, h, 128, 0);
        sel.fill_rect(Rect::new(400, 400, 2000, 2000), 255);
        {
            let mut s = base.clone();
            aurastudio_core::zone!("blur 50 px in a selection");
            Filter { values: vec![50.0], ..Filter::new(Kind::GaussianBlur) }.apply_to(&mut s, Some(&sel));
        }
        aurastudio_core::zone!("feather 20 px");
        blur_mask(&mut sel, 20.0);
    }
}

/// A layer-sized source (1610 × 1247) turned and scaled at 85 / 40 /
/// 160 %, bilinear and bicubic, then with a bent grid: the transform's
/// render and its pyramid.
pub struct Transform;

impl Scenario for Transform {
    fn name(&self) -> &'static str { "transform (1610 × 1247 source)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_core::Point;
        use aurastudio_transform::{corners, render, Interp, Mesh, Source, Warp};
        let (w, h) = (1610i32, 1247i32);
        let r = Rect::new(0, 0, w, h);
        let px: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 61) as u8, 255]).collect();
        let src = {
            aurastudio_core::zone!("transform source");
            Source::new(r, &px)
        };
        let (c, a) = (Point::new(w as f64 / 2.0, h as f64 / 2.0), 0.15f64);
        let mut out = vec![[0u8; 4]; r.area() as usize];
        for (name, k) in [("transform 85 %", 0.85), ("transform 40 %", 0.4), ("transform 160 %", 1.6)] {
            let quad = corners(r).map(|p| { let d = (p - c) * k; Point::new(c.x + d.x * a.cos() - d.y * a.sin(), c.y + d.x * a.sin() + d.y * a.cos()) });
            for interp in [Interp::Bilinear, Interp::Bicubic] {
                aurastudio_core::zone!(name);
                render(&src, &Warp { quad, mesh: None }, interp, r, &mut out);
            }
            let mut m = Mesh::new(4, 4);
            m.pts[6].x += 0.05;
            aurastudio_core::zone!("transform mesh");
            render(&src, &Warp { quad, mesh: Some(m) }, Interp::Bilinear, r, &mut out);
        }
    }
}

/// Selections on an A4 300 dpi frame: a lasso, an ellipse, grow, shrink,
/// feather, border, invert, the outline at 1:1 and zoomed out.
pub struct Selections;

impl Scenario for Selections {
    fn name(&self) -> &'static str { "selection (A4 300 dpi, 128 px tiles)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_core::Point;
        use aurastudio_selection::{Selection, Space};
        let s = Space { width: 2480, height: 3508, tile: 128 };
        let lasso: Vec<Point> = (0..2000).map(|i| {
            let t = i as f64 / 2000.0 * std::f64::consts::TAU;
            let rr = 900.0 + 120.0 * (7.0 * t).sin();
            Point::new(1240.0 + rr * t.cos(), 1750.0 + rr * 1.3 * t.sin())
        }).collect();
        let sel = { aurastudio_core::zone!("selection lasso"); Selection::polygon(s, &lasso, true) };
        let Some(sel) = sel else { return };
        { aurastudio_core::zone!("selection ellipse"); let _ = Selection::ellipse(s, [200.0, 300.0, 2200.0, 3000.0], true); }
        { aurastudio_core::zone!("selection grow 20"); let _ = sel.grown(20.0); }
        { aurastudio_core::zone!("selection shrink 20"); let _ = sel.shrunk(20.0); }
        { aurastudio_core::zone!("selection feather 20"); let _ = sel.feathered(20.0); }
        { aurastudio_core::zone!("selection border 10"); let _ = sel.bordered(10.0); }
        { aurastudio_core::zone!("selection invert"); let _ = sel.inverted(); }
        { aurastudio_core::zone!("selection outline 1:1"); let _ = sel.outline(1); }
        { aurastudio_core::zone!("selection outline 1:8"); let _ = sel.outline(8); }
    }
}

/// The tools' geometry per frame: shape paths as a drag would ask for
/// them, a transform box dragged (scale, turn, warp) with its lines.
pub struct Tools;

impl Scenario for Tools {
    fn name(&self) -> &'static str { "tools (shapes, transform box)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_core::Point;
        use aurastudio_tools::{shapes, Hit, Mods, Polylines, TransformMode, Xform};
        for i in 0..240 {
            let b = Point::new(100.0 + i as f64 * 8.0, 80.0 + i as f64 * 5.0);
            aurastudio_core::zone!("shape paths (frame)");
            std::hint::black_box((shapes::ellipse(Point::new(50.0, 50.0), b), shapes::rectangle(Point::new(50.0, 50.0), b), shapes::arc(Point::new(0.0, 0.0), b, Point::new(b.x, 0.0))));
        }
        let mut x = Xform::new(Rect::new(0, 0, 2480, 3508));
        let mut lines = Polylines::default();
        for (hit, mode) in [(Hit::Corner(2), TransformMode::Free), (Hit::Outside, TransformMode::Free), (Hit::Point(12), TransformMode::Warp)] {
            if mode == TransformMode::Warp { x.set_grid(4); }
            let start = match hit { Hit::Point(_) => x.mesh_points()[12], Hit::Corner(i) => x.warp.quad[i], _ => Point::new(3000.0, 1754.0) };
            x.press(hit, start);
            for k in 0..240 {
                aurastudio_core::zone!("transform drag (frame)");
                x.drag_to(start + Point::new(k as f64 * 2.0, k as f64), Mods::default(), mode, true);
                x.lines(mode, &mut lines);
            }
            x.release();
        }
    }
}

/// Display tables (what the app builds when the monitor, the profile or
/// the view mode changes) and an A4 300 dpi layer converted to another
/// profile.
pub struct Color;

impl Scenario for Color {
    fn name(&self) -> &'static str { "color (display tables, A4 conversion)" }
    fn run(&mut self, report: &mut Report) {
        use aurastudio_color::{convert_buffer, Channel, DisplaySetup, Intent, Profile, Proof, ViewMode};
        let p3 = Profile::display_p3();
        let base = DisplaySetup { monitor: p3.clone(), ..DisplaySetup::plain() };
        let proof = Proof { output: Profile::srgb(), intent: Intent::RelativeColorimetric, paper_white: false, gamut_warning: Some([0, 255, 255]) };
        let cases: [(&'static str, DisplaySetup); 4] = [
            ("table sRGB → P3 monitor", base.clone()),
            ("table grayscale", DisplaySetup { mode: ViewMode::Grayscale, ..base.clone() }),
            ("table channel a", DisplaySetup { mode: ViewMode::Channel(Channel::A), ..base.clone() }),
            ("table soft proof + warning", DisplaySetup { document: p3.clone(), mode: ViewMode::Proof(proof), ..base }),
        ];
        for (name, s) in cases {
            aurastudio_core::zone!(name);
            if let Err(e) = s.build() { report.suspect(self.name(), name, e.0, "DisplaySetup::build"); }
        }
        let mut px: Vec<[u8; 4]> = (0..2480usize * 3508).map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 61) as u8, 255]).collect();
        aurastudio_core::zone!("convert A4 sRGB → P3");
        let _ = convert_buffer(&Profile::srgb(), &p3, Intent::RelativeColorimetric, &mut px);
    }
}

/// The fill tool on an A4 300 dpi line-art layer (a grid of ink lines):
/// a flood of one cell, of a blank page, the magic wand everywhere; and the layer tree's edits on a 60-node stack.
pub struct Layers;

impl Scenario for Layers {
    fn name(&self) -> &'static str { "layers (fill on A4 line art, 60-node tree)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_layers::fill::{flood, similar_all};
        use aurastudio_layers::{LayerStack, Place, Start};
        let (w, h) = (2480i32, 3508i32);
        let mut ink = Image::new(w as u32, h as u32, 128, [255, 255, 255, 0]);
        for k in (200..w).step_by(400) { ink.fill_rect(Rect::new(k, 0, k + 3, h), [0, 0, 0, 255]); }
        for k in (200..h).step_by(400) { ink.fill_rect(Rect::new(0, k, w, k + 3), [0, 0, 0, 255]); }
        let frame = Rect::new(0, 0, w, h);
        { aurastudio_core::zone!("fill one cell"); std::hint::black_box(flood(&ink, frame, 500, 500, 32, 1, None)); }
        let blank = Image::new(w as u32, h as u32, 128, [255, 255, 255, 0]);
        { aurastudio_core::zone!("fill a blank page"); std::hint::black_box(flood(&blank, frame, 10, 10, 32, 1, None)); }
        { aurastudio_core::zone!("magic wand everywhere"); std::hint::black_box(similar_all(&ink, frame, 10, 10, 32, 0)); }
        let mut s = LayerStack::new(w as u32, h as u32, 128, Start::Paper);
        for i in 0..20 {
            let f = s.add_folder("f");
            s.add_layer("a");
            s.add_layer("b");
            s.set_active(f);
            if i % 3 == 0 { s.set_active(1); }
        }
        aurastudio_core::zone!("tree edits (200)");
        for i in 0..200u64 {
            let ids = s.layer_ids();
            let (a, b) = (ids[(i as usize * 7) % ids.len()], ids[(i as usize * 13 + 5) % ids.len()]);
            s.move_node(a, b, if i % 2 == 0 { Place::Above } else { Place::Below });
            let _ = s.rows();
            let _ = s.tree_state();
        }
    }
}

/// The merge on an A4 300 dpi document of 20 layers (painted bands,
/// some blend modes, an isolated folder): the whole frame, then a stroke
/// on a middle layer shown through the stroke cache, dab-sized rects.
pub struct Compose;

impl Scenario for Compose {
    fn name(&self) -> &'static str { "compose (A4 300 dpi, 20 layers)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_compose::{merge, Opts, StrokeCache};
        use aurastudio_layers::{FolderBlend, LayerStack, Node, Start};
        let (w, h) = (2480i32, 3508i32);
        let mut s = LayerStack::new(w as u32, h as u32, 128, Start::Paper);
        let mut rng = Rng::new(4);
        let mut mid = 0;
        for i in 0..20 {
            if i == 10 {
                let f = s.add_folder("f");
                if let Some(Node::Folder(x)) = s.node_mut(f) { x.blend = FolderBlend::Isolated(BlendMode::Normal); x.opacity = 0.8; }
            }
            let id = s.add_layer("l");
            let (x, y) = (rng.below(w as u64) as i32 / 2, rng.below(h as u64) as i32 / 2);
            let r = Rect::new(x, y, x + 900, y + 1400);
            let c = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8, 200];
            s.set_layer_rect(id, r, &vec![c; r.area() as usize]);
            if let Some(Node::Layer(l)) = s.node_mut(id) { if i < 5 && i % 2 == 1 { l.blend = BlendMode::Multiply; } }
            if i == 5 { mid = id; }
        }
        for _ in 0..2 {
            aurastudio_core::zone!("merge A4 frame");
            std::hint::black_box(merge(&s, s.frame(), Opts::default()));
        }
        let mut sc = StrokeCache::default();
        for k in 0..120 {
            let (x, y) = (300 + k * 12, 600 + k * 9);
            aurastudio_core::zone!("merge stroke rect (64 px)");
            std::hint::black_box(sc.merge(&s, mid, Rect::new(x, y, x + 64, y + 64)));
        }
    }
}

/// A document at work (A4 300 dpi): strokes as undo steps, undo / redo,
/// a filter preview, a transform dragged, a fill, save to a snapshot and
/// back.
pub struct Document;

impl Scenario for Document {
    fn name(&self) -> &'static str { "document (A4 300 dpi)" }
    fn run(&mut self, _report: &mut Report) {
        use aurastudio_document::{DocId, Document as Doc, Edit};
        use aurastudio_filters::{Filter, Kind};
        use aurastudio_transform::{corners, Interp, Warp};
        let mut d = Doc::new(DocId(1), "a4", 2480, 3508, DocType::Illustration);
        let mut rng = Rng::new(9);
        for _ in 0..20 {
            let (x, y) = (rng.below(2300) as i32, rng.below(3300) as i32);
            let r = Rect::new(x, y, x + 160, y + 160);
            let c = [rng.below(256) as u8, 40, 90, 255];
            aurastudio_core::zone!("document stroke 160 px");
            d.stroke("Brush stroke", |s| {
                s.prepare(r);
                for y in r.y0..r.y1 {
                    let mut x = r.x0;
                    while x < r.x1 {
                        let sp = s.span_mut(x, y, (r.x1 - x) as usize);
                        let n = sp.len();
                        if n == 0 { break; }
                        sp.pixels.fill(c);
                        x += n as i32;
                    }
                }
            });
        }
        for _ in 0..10 { aurastudio_core::zone!("document undo"); d.apply(Edit::Undo); }
        for _ in 0..10 { aurastudio_core::zone!("document redo"); d.apply(Edit::Redo); }
        for v in [10.0, 30.0, 60.0] {
            aurastudio_core::zone!("document filter preview (brightness)");
            d.apply(Edit::FilterPreview(Some(Filter { values: vec![v, 0.0, 0.0], ..Filter::new(Kind::BrightnessContrast) })));
        }
        { aurastudio_core::zone!("document filter cancel"); d.apply(Edit::FilterCancel); }
        let id = d.paint_id().unwrap_or(0);
        { aurastudio_core::zone!("document transform begin"); d.apply(Edit::TransformBegin(vec![id])); }
        let src = d.transform_source().unwrap_or(Rect::EMPTY);
        for k in 0..5 {
            aurastudio_core::zone!("document transform preview");
            d.apply(Edit::TransformPreview { warp: Warp { quad: corners(src.moved(k * 20, k * 10)), mesh: None }, interp: Interp::Bilinear });
        }
        { aurastudio_core::zone!("document transform cancel"); d.apply(Edit::TransformCancel); }
        { aurastudio_core::zone!("document fill"); d.apply(Edit::Fill { x: 5, y: 5, color: [0, 200, 0, 255], params: Default::default() }); }
        let snap = { aurastudio_core::zone!("document snapshot"); d.snapshot(None) };
        let back = { aurastudio_core::zone!("document from snapshot"); Doc::from_snapshot(DocId(2), "back", snap) };
        std::hint::black_box(back);
    }
}

/// Every scenario, in report order.
pub fn all() -> Vec<Box<dyn Scenario>> {
    let n = crate::env_u64("STROKES", 20) as u32;
    vec![
        Box::new(Idle { millis: crate::env_u64("IDLE_MS", 2000) }),
        Box::new(RasterStroke { kind: DocType::Animation, w: 3840, h: 2160, strokes: n, radius: 8.0 }),
        Box::new(RasterStroke { kind: DocType::Comics, w: 5906, h: 8339, strokes: n, radius: 12.0 }),
        Box::new(RasterStroke { kind: DocType::Illustration, w: 2480, h: 3508, strokes: n, radius: 150.0 }),
        Box::new(RasterFile { w: 3508, h: 4961 }),
        Box::new(RasterLineArt),
        Box::new(Filters),
        Box::new(Transform),
        Box::new(Selections),
        Box::new(Tools),
        Box::new(Color),
        Box::new(Layers),
        Box::new(Compose),
        Box::new(Document),
    ]
}

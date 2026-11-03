use aurastudio_animation::Sheets;
use aurastudio_core::{DocType, PaintSurface, Point, Rect, Rgba8};
use aurastudio_filters::{Filter, Kind};
use aurastudio_layers::FillParams;
use aurastudio_selection::{Mode, Selection};
use aurastudio_transform::{corners, Interp, Warp};

use crate::*;

/// A stroke painting `r` with `c` (inside the selection).
fn paint(d: &mut Document, r: Rect, c: Rgba8) -> Done { d.stroke("Brush stroke", |s| { s.prepare(r); rect_in(s, r, c) }) }

fn rect_in(s: &mut aurastudio_raster::Stroke<'_, Rgba8>, r: Rect, c: Rgba8) {
        for y in r.y0..r.y1 {
            let mut x = r.x0;
            while x < r.x1 {
                let sp = s.span_mut(x, y, (r.x1 - x) as usize);
                let n = sp.len();
                if n == 0 { break; }
                for i in 0..n {
                    let k = sp.limit.map_or(255, |l| l[i]);
                    if k == 0 { continue; }
                    sp.pixels[i] = c;
                    sp.mask[i] = k;
                }
                x += n as i32;
            }
        }
}

fn doc(w: u32, h: u32) -> Document { Document::new(DocId(1), "t", w, h, DocType::Illustration) }

fn px(d: &Document, id: u64, x: i32, y: i32) -> Rgba8 { d.stack().image(id).unwrap().pixel(x, y) }

fn add_layer(d: &mut Document) -> u64 { d.apply(Edit::AddLayer("ink".into())).made.unwrap() }

#[test]
fn undo_redo_restore_exact_pixels() {
    let mut d = doc(64, 64);
    let id = add_layer(&mut d);
    let done = paint(&mut d, Rect::new(10, 10, 20, 20), [255, 0, 0, 255]);
    assert!(matches!(done.region, Region::Rects(_)));
    paint(&mut d, Rect::new(15, 15, 30, 30), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 16, 16), [0, 0, 255, 255]);
    let u = d.apply(Edit::Undo);
    assert!(u.changed() && !u.tree);
    assert_eq!(px(&d, id, 16, 16), [255, 0, 0, 255]);
    assert_eq!(px(&d, id, 25, 25)[3], 0);
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 12, 12)[3], 0);
    // Then the new layer itself.
    assert!(d.apply(Edit::Undo).tree);
    assert!(d.stack().node(id).is_none());
    assert!(!d.apply(Edit::Undo).changed());
    assert!(d.apply(Edit::Redo).tree);
    d.apply(Edit::Redo);
    d.apply(Edit::Redo);
    assert_eq!(px(&d, id, 16, 16), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 12, 12), [255, 0, 0, 255]);
    assert!(!d.apply(Edit::Redo).changed());
}

/// A stroke from one corner to the other keeps only the tiles it changed.
#[test]
fn a_stroke_keeps_only_the_tiles_it_changed() {
    let mut d = doc(2000, 2000);
    add_layer(&mut d);
    let id = d.paint_id().unwrap();
    d.stroke("Brush stroke", |s| {
        s.prepare(Rect::new(0, 0, 2000, 2000));
        for (r, c) in [(Rect::new(5, 5, 20, 20), [9u8, 9, 9, 255]), (Rect::new(1900, 1900, 1930, 1930), [7, 7, 7, 255])] { rect_in(s, r, c); }
    });
    // Two tiles of 128 px (the whole box would be 16 MB).
    assert!(d.history().bytes() < 300_000, "{} bytes", d.history().bytes());
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 10, 10)[3], 0);
    d.apply(Edit::Redo);
    assert_eq!(px(&d, id, 1920, 1920), [7, 7, 7, 255]);
    // Nothing painted: no step.
    let n = d.history().undo_len();
    assert!(!d.stroke("Brush stroke", |s| s.prepare(Rect::new(100, 100, 300, 300))).changed());
    assert_eq!(d.history().undo_len(), n);
}

#[test]
fn a_new_action_drops_redo_and_old_steps_go_over_budget() {
    let mut d = doc(32, 32);
    add_layer(&mut d);
    paint(&mut d, Rect::new(0, 0, 4, 4), [1, 2, 3, 255]);
    d.apply(Edit::Undo);
    paint(&mut d, Rect::new(8, 8, 12, 12), [4, 5, 6, 255]);
    assert!(!d.apply(Edit::Redo).changed());
    assert_eq!(d.history().undo_len(), 2, "the new layer, the stroke");
    let mut d = doc(256, 256);
    d.history = History::new(300 * 1024);
    add_layer(&mut d);
    for k in 0..6u64 {
        let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ k;
        d.stroke("Noise", |s| {
            s.prepare(Rect::of_size(256, 256));
            for y in 0..256 {
                let mut x0 = 0;
                while x0 < 256 {
                    let sp = s.span_mut(x0, y, (256 - x0) as usize);
                    for i in 0..sp.len() { x ^= x << 13; x ^= x >> 7; x ^= x << 17; sp.pixels[i] = [(x >> 24) as u8, (x >> 32) as u8, 1, 255]; sp.mask[i] = 255; }
                    x0 += sp.len() as i32;
                }
            }
        });
    }
    assert!(d.history().bytes() <= 300 * 1024, "{}", d.history().bytes());
    assert!(d.history().undo_len() < 6);
}

#[test]
fn clear_is_undoable_and_respects_lock() {
    let mut d = doc(32, 32);
    let id = add_layer(&mut d);
    paint(&mut d, Rect::of_size(32, 32), [9, 9, 9, 255]);
    assert!(d.apply(Edit::ClearActive).changed());
    assert_eq!(px(&d, id, 5, 5)[3], 0);
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 5, 5), [9, 9, 9, 255]);
    d.apply(Edit::SetLocked(id, true));
    assert!(d.apply(Edit::ClearActive).refused.is_some());
}

/// The Layers docker's edits are undone one at a time, newest first,
/// between the strokes; each says where the image may have changed.
#[test]
fn layer_edits_are_undone_in_order() {
    let mut d = doc(64, 64);
    let base = d.stack().active_id();
    paint(&mut d, Rect::new(0, 0, 8, 8), [1, 1, 1, 255]);
    let new = add_layer(&mut d);
    paint(&mut d, Rect::new(20, 20, 30, 30), [2, 2, 2, 255]);
    let c = d.apply(Edit::SetOpacity(new, 0.5));
    assert!(matches!(c.region.bounds(), Some(Some(r)) if r.contains_rect(&Rect::new(20, 20, 30, 30))));
    assert!(!d.apply(Edit::SetOpacity(new, 0.5)).changed(), "no change, no step");
    assert!(d.apply(Edit::SetClip(new, true)).tree);
    assert_eq!(d.history().undo_label(), Some("Clipping"));
    d.apply(Edit::Undo);
    assert!(!d.stack().node(new).unwrap().props().clip);
    d.apply(Edit::Undo);
    assert_eq!(d.stack().node(new).unwrap().opacity(), 1.0);
    d.apply(Edit::Undo);
    d.apply(Edit::Undo);
    assert!(d.stack().node(new).is_none(), "the new layer gone");
    d.apply(Edit::Undo);
    assert_eq!(px(&d, base, 1, 1)[3], 0, "then the stroke");
    for _ in 0..5 { d.apply(Edit::Redo); }
    assert!(d.stack().node(new).unwrap().props().clip);
    // A deletion is undone with the layer's pixels.
    d.apply(Edit::SetActive(new));
    assert!(d.apply(Edit::RemoveActive).changed());
    d.apply(Edit::Undo);
    assert_eq!(px(&d, new, 25, 25), [2, 2, 2, 255]);
    // An empty layer added changes no pixel.
    assert!(d.apply(Edit::AddLayer("empty".into())).region.is_nothing());
}

#[test]
fn filters_preview_cancel_apply_and_undo() {
    let mut d = doc(64, 64);
    let id = add_layer(&mut d);
    paint(&mut d, Rect::new(10, 10, 20, 20), [200, 100, 50, 255]);
    let inv = Filter::new(Kind::Invert);
    assert!(d.apply(Edit::FilterPreview(Some(inv.clone()))).changed());
    assert_eq!(px(&d, id, 15, 15), [55, 155, 205, 255]);
    assert!(d.filtering());
    assert_eq!(px(&d, id, 40, 40)[3], 0, "invisible stays invisible");
    d.apply(Edit::FilterCancel);
    assert_eq!(px(&d, id, 15, 15), [200, 100, 50, 255]);
    assert!(!d.filtering());
    let n = d.history().undo_len();
    d.apply(Edit::FilterPreview(Some(Filter::new(Kind::Sepia))));
    d.apply(Edit::FilterApply(inv.clone()));
    assert_eq!(px(&d, id, 15, 15), [55, 155, 205, 255]);
    assert_eq!(d.history().undo_len(), n + 1);
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 15, 15), [200, 100, 50, 255]);
    // A selection limits it; the Paper (its fill) changes too, and back.
    d.apply(Edit::Select(SelectOp::Shape { shape: Shape::Rect(Rect::new(0, 0, 15, 64)), mode: Mode::Replace, aa: false, feather: 0.0 }));
    let paper = d.stack().root[0].id();
    d.apply(Edit::SetActive(paper));
    assert!(d.apply(Edit::FilterApply(inv.clone())).refused.is_some(), "the Paper is locked");
    d.apply(Edit::SetLocked(paper, false));
    d.apply(Edit::FilterApply(inv.clone()));
    assert_eq!(px(&d, paper, 5, 5), [0, 0, 0, 255]);
    assert_eq!(px(&d, paper, 30, 5), [255; 4]);
    d.apply(Edit::Select(SelectOp::None));
    d.apply(Edit::FilterApply(inv));
    assert_eq!(px(&d, paper, -500, 5), [0, 0, 0, 255], "the fill everywhere");
    d.apply(Edit::Undo);
    assert_eq!(px(&d, paper, -500, 5), [255; 4]);
    assert_eq!(px(&d, paper, 5, 5), [0, 0, 0, 255]);
}

#[test]
fn selections_are_undone_and_reselected() {
    let mut d = doc(100, 100);
    let rect = |r| Edit::Select(SelectOp::Shape { shape: Shape::Rect(r), mode: Mode::Replace, aa: true, feather: 0.0 });
    assert!(d.apply(rect(Rect::new(10, 10, 50, 50))).selection);
    let add = Edit::Select(SelectOp::Shape { shape: Shape::Ellipse([40.0, 40.0, 90.0, 90.0]), mode: Mode::Add, aa: true, feather: 0.0 });
    assert!(d.apply(add.clone()).selection);
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(10, 10, 90, 90));
    assert!(d.apply(Edit::Select(SelectOp::None)).selection);
    assert_eq!(d.history().undo_label(), Some("Deselect"));
    assert!(d.apply(Edit::Select(SelectOp::Reselect)).selection);
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(10, 10, 90, 90));
    d.apply(Edit::Undo);
    assert!(d.selection().is_none());
    d.apply(Edit::Undo);
    d.apply(Edit::Undo);
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(10, 10, 50, 50));
    d.apply(Edit::Redo);
    let n = d.history().undo_len();
    assert!(!d.apply(add).changed());
    assert_eq!(d.history().undo_len(), n);
    assert!(d.apply(Edit::Select(SelectOp::Invert)).selection);
    assert!(d.selection().unwrap().contains(5, 5) && !d.selection().unwrap().contains(20, 20));
    // A stroke stays inside the selection.
    let id = add_layer(&mut d);
    d.apply(rect(Rect::new(0, 0, 10, 100)));
    paint(&mut d, Rect::new(0, 0, 50, 50), [3, 3, 3, 255]);
    assert_eq!(px(&d, id, 5, 5), [3, 3, 3, 255]);
    assert_eq!(px(&d, id, 20, 5)[3], 0);
    // Kept as a mask and loaded back.
    let m = d.apply(Edit::SelectionToMask("Kept".into())).made.unwrap();
    d.apply(Edit::Select(SelectOp::None));
    assert!(d.apply(Edit::MaskToSelection(m)).selection);
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(0, 0, 10, 100));
}

#[test]
fn the_wand_and_the_fill_pick_like_pixels() {
    let mut d = doc(64, 64);
    let id = add_layer(&mut d);
    for r in [Rect::new(4, 4, 20, 20), Rect::new(40, 40, 60, 60)] { paint(&mut d, r, [255, 0, 0, 255]); }
    let mut wand = Wand::default();
    let op = |wand, x, y, aa| Edit::Select(SelectOp::Wand { x, y, wand, mode: Mode::Replace, aa, feather: 0.0 });
    d.apply(op(wand, 10, 10, false));
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(4, 4, 20, 20));
    wand.contiguous = false;
    d.apply(op(wand, 10, 10, false));
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(4, 4, 60, 60));
    wand.fill.expand = 2;
    d.apply(op(wand, 10, 10, true));
    assert_eq!(d.selection().unwrap().coverage(10, 10), 255);
    assert!(d.selection().unwrap().coverage(2, 10) > 0);
    d.apply(Edit::Select(SelectOp::FromLayer(Mode::Replace)));
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(4, 4, 60, 60));
    d.apply(Edit::Select(SelectOp::None));
    // The fill: inside the first square only, undone.
    let f = d.apply(Edit::Fill { x: 10, y: 10, color: [0, 255, 0, 255], params: FillParams { refer: aurastudio_layers::Refer::Layer, expand: 0, ..FillParams::default() } });
    assert!(f.changed());
    assert_eq!(px(&d, id, 10, 10), [0, 255, 0, 255]);
    assert_eq!(px(&d, id, 50, 50), [255, 0, 0, 255]);
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 10, 10), [255, 0, 0, 255]);
    // Outside the frame: nothing, no step.
    let n = d.history().undo_len();
    assert!(!d.apply(Edit::Fill { x: 500, y: 500, color: [0; 4], params: FillParams::default() }).changed());
    assert_eq!(d.history().undo_len(), n);
}

#[test]
fn copy_paste_selected_pixels() {
    let mut d = doc(64, 64);
    add_layer(&mut d);
    paint(&mut d, Rect::of_size(64, 64), [9, 8, 7, 255]);
    d.apply(Edit::Select(SelectOp::Shape { shape: Shape::Polygon(vec![Point::new(10.0, 10.0), Point::new(30.0, 10.0), Point::new(10.0, 30.0)]), mode: Mode::Replace, aa: false, feather: 0.0 }));
    let clip = d.copy().unwrap();
    assert_eq!(clip.rect, Rect::new(10, 10, 29, 29));
    let done = d.apply(Edit::Paste(clip.clone()));
    let new = done.made.unwrap();
    assert_eq!(d.stack().active_id(), new);
    assert_eq!(px(&d, new, 12, 12), [9, 8, 7, 255]);
    assert_eq!(px(&d, new, 28, 28)[3], 0, "outside the lasso");
    // Partly off the frame: kept there (the canvas is unbounded).
    let off = Clip { rect: clip.rect.moved(-15, 50), ..clip.clone() };
    let o = d.apply(Edit::Paste(off)).made.unwrap();
    assert_eq!(px(&d, o, -3, 62), [9, 8, 7, 255]);
    d.apply(Edit::Undo);
    assert!(d.stack().node(new).is_some());
    d.apply(Edit::Undo);
    assert!(d.stack().node(new).is_none(), "undoing the paste takes its layer away");
    d.apply(Edit::Redo);
    assert_eq!(px(&d, new, 12, 12), [9, 8, 7, 255], "and redo brings it back, pixels too");
}

#[test]
fn transforms_move_apply_undo_cancel() {
    let mut d = doc(100, 100);
    let id = add_layer(&mut d);
    paint(&mut d, Rect::new(10, 10, 30, 30), [0, 0, 255, 255]);
    d.apply(Edit::TransformBegin(vec![id]));
    assert_eq!(d.transform_source(), Some(Rect::new(10, 10, 30, 30)));
    let moved = Warp { quad: corners(Rect::new(50, 60, 70, 80)), mesh: None };
    d.apply(Edit::TransformPreview { warp: moved.clone(), interp: Interp::Bicubic });
    assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 15, 15)[3], 0);
    d.apply(Edit::TransformCancel);
    assert_eq!(px(&d, id, 15, 15), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 55, 65)[3], 0);
    d.apply(Edit::TransformBegin(vec![id]));
    d.apply(Edit::TransformPreview { warp: Warp { quad: corners(Rect::new(0, 0, 5, 5)), mesh: None }, interp: Interp::Bilinear });
    let n = d.history().undo_len();
    d.apply(Edit::TransformApply { warp: moved.clone(), interp: Interp::Bicubic });
    assert!(!d.transforming());
    assert_eq!((d.history().undo_len(), d.history().undo_label()), (n + 1, Some("Transform")));
    assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 2, 2)[3], 0, "the old preview is gone");
    d.apply(Edit::Undo);
    assert_eq!(px(&d, id, 15, 15), [0, 0, 255, 255]);
    assert_eq!(px(&d, id, 55, 65)[3], 0);
    d.apply(Edit::Redo);
    assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
    assert!(d.apply(Edit::TransformBegin(vec![])).refused.is_some());
    let empty = add_layer(&mut d);
    assert_eq!(d.apply(Edit::TransformBegin(vec![empty])).refused, Some("Nothing to transform"));
    // Past the frame: kept there.
    d.apply(Edit::TransformBegin(vec![id]));
    d.apply(Edit::TransformApply { warp: Warp { quad: corners(Rect::new(-40, 10, -20, 30)), mesh: None }, interp: Interp::Bilinear });
    assert_eq!(px(&d, id, -30, 20), [0, 0, 255, 255]);
}

#[test]
fn a_selection_says_what_moves_and_goes_along() {
    let mut d = doc(100, 100);
    let a = add_layer(&mut d);
    paint(&mut d, Rect::of_size(100, 100), [255, 0, 0, 255]);
    let b = add_layer(&mut d);
    paint(&mut d, Rect::of_size(100, 100), [0, 255, 0, 255]);
    d.apply(Edit::Select(SelectOp::Shape { shape: Shape::Rect(Rect::new(0, 0, 20, 20)), mode: Mode::Replace, aa: false, feather: 0.0 }));
    d.apply(Edit::TransformBegin(vec![a, b]));
    assert_eq!(d.transform_source(), Some(Rect::new(0, 0, 20, 20)));
    let lift = d.transform_lift().unwrap();
    assert_eq!((lift.layers.len(), lift.layers[0].premul.len()), (2, 22 * 22));
    d.apply(Edit::TransformApply { warp: Warp { quad: corners(Rect::new(50, 50, 70, 70)), mesh: None }, interp: Interp::Bicubic });
    for (id, c) in [(a, [255, 0, 0, 255]), (b, [0, 255, 0, 255])] {
        assert_eq!(px(&d, id, 5, 5)[3], 0, "lifted out");
        assert_eq!(px(&d, id, 30, 5), c, "the rest stays");
        assert_eq!(px(&d, id, 60, 60), c);
    }
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(50, 50, 70, 70));
    d.apply(Edit::Undo);
    assert_eq!(d.selection().unwrap().bbox(), Rect::new(0, 0, 20, 20));
    assert_eq!(px(&d, a, 5, 5), [255, 0, 0, 255]);
    assert_eq!(px(&d, b, 5, 5), [0, 255, 0, 255]);
}

/// The live preview's pieces, put back together where nothing moved, give
/// the image as it was.
#[test]
fn the_lift_pieces_make_the_image() {
    let mut d = doc(100, 80);
    let id = add_layer(&mut d);
    paint(&mut d, Rect::new(10, 10, 60, 50), [0, 0, 255, 255]);
    add_layer(&mut d);
    paint(&mut d, Rect::new(0, 0, 100, 20), [255, 0, 0, 120]);
    let whole = aurastudio_compose::merge(d.stack(), d.stack().frame(), Default::default());
    d.apply(Edit::SetActive(id));
    d.apply(Edit::TransformBegin(vec![id]));
    let src = d.transform_source().unwrap();
    let lift = d.transform_lift().unwrap();
    assert_eq!(&lift.below[30 * 100 + 30][..3], &[255, 255, 255]);
    let above = lift.above.as_ref().expect("the top layer shows");
    let sw = src.w() as usize;
    for y in 0..80i32 {
        for x in 0..100i32 {
            let i = (y * 100 + x) as usize;
            let mut c: Vec<f32> = lift.below[i][..3].iter().map(|v| *v as f32).collect();
            if src.contains(x, y) {
                let p = lift.layers[0].premul[((y - src.y0 + 1) as usize) * (sw + 2) + (x - src.x0 + 1) as usize];
                let a = p[3] as f32 / 255.0;
                for k in 0..3 { c[k] = p[k] as f32 + c[k] * (1.0 - a); }
            }
            let o = above[i];
            let a = o[3] as f32 / 255.0;
            for k in 0..3 { assert!((o[k] as f32 * a + c[k] * (1.0 - a) - whole[i][k] as f32).abs() <= 2.0, "at {x},{y}"); }
        }
    }
    assert_eq!(px(&d, id, 30, 30), [0, 0, 255, 255]);
}

/// The first stroke on an empty frame makes a sheet: one step takes both
/// away; a stroke on it after is a step of its own; a sheet for nothing
/// goes.
#[test]
fn a_new_sheet_and_its_stroke_are_one_step() {
    let mut d = Document::new(DocId(1), "a", 32, 32, DocType::Animation);
    let a = d.stack().root.iter().find(|n| n.is_animation()).unwrap().id();
    let steps = d.history().undo_len();
    d.apply(Edit::SetActive(a));
    d.apply(Edit::SetFrame(3));
    let before = d.stack().sheets(a).len();
    let done = paint(&mut d, Rect::new(2, 2, 12, 12), [9, 9, 9, 255]);
    assert!(done.tree);
    assert_eq!(d.stack().sheets(a).len(), before + 1);
    assert_eq!(d.history().undo_len(), steps + 1, "one step");
    assert_eq!(d.history().undo_label(), Some("Brush stroke"));
    let cel = d.paint_id().unwrap();
    d.apply(Edit::Undo);
    assert_eq!(d.stack().sheets(a).len(), before, "the sheet went with it");
    d.apply(Edit::Redo);
    assert_eq!(px(&d, cel, 5, 5), [9, 9, 9, 255]);
    paint(&mut d, Rect::new(2, 2, 4, 4), [1, 1, 1, 255]);
    assert_eq!(d.history().undo_len(), steps + 2);
    d.apply(Edit::Undo);
    assert_eq!(d.stack().sheets(a).len(), before + 1);
    // A sheet for nothing goes, no step.
    d.apply(Edit::SetFrame(8));
    let n = d.history().undo_len();
    assert!(!d.stroke("Brush stroke", |s| s.prepare(Rect::new(0, 0, 4, 4))).changed());
    assert_eq!(d.stack().sheets(a).len(), before + 1);
    assert_eq!((d.history().undo_len(), d.history().redo_len()), (n, 0));
    assert!(!d.apply(Edit::Fill { x: 500, y: 500, color: [0; 4], params: FillParams::default() }).changed());
    assert_eq!(d.stack().sheets(a).len(), before + 1);
    // Frame changes are not undo steps, and settle the sheets shown.
    d.apply(Edit::SetFrame(1));
    assert_eq!(d.history().undo_len(), n);
    assert_eq!(d.timeline().frame(), 1);
}

#[test]
fn save_and_open_give_the_same_document() {
    let mut d = doc(96, 64);
    d.apply(Edit::SetDpi(600.0));
    d.apply(Edit::AssignProfile(aurastudio_color::Profile::display_p3()));
    let ink = add_layer(&mut d);
    paint(&mut d, Rect::new(10, 10, 40, 30), [200, 10, 10, 255]);
    paint(&mut d, Rect::new(30, 30, 90, 60), [0, 255, 0, 255]);
    d.apply(Edit::Undo);
    d.selection = Selection::rect(d.space(), Rect::new(1, 2, 30, 40));
    let snap = d.snapshot(None);
    let mut buf = std::io::Cursor::new(Vec::new());
    aurastudio_file::write(&snap, &mut buf).unwrap();
    let back = aurastudio_file::read(std::io::Cursor::new(buf.into_inner()), 16384).unwrap();
    let mut b = Document::from_snapshot(DocId(2), "t", back);
    assert!(!b.is_modified());
    assert_eq!((b.dpi(), b.profile().name()), (600.0, "Display P3"));
    assert_eq!(b.selection().unwrap().bbox(), Rect::new(1, 2, 30, 40));
    assert_eq!(px(&b, ink, 15, 15), [200, 10, 10, 255]);
    // The history came along: redo, then undo before the strokes.
    assert_eq!(b.history().redo_len(), 1);
    b.apply(Edit::Redo);
    assert_eq!(px(&b, ink, 50, 40), [0, 255, 0, 255]);
    b.apply(Edit::Undo);
    b.apply(Edit::Undo);
    assert_eq!(px(&b, ink, 15, 15)[3], 0);
    // A preview under way is not saved.
    d.apply(Edit::FilterPreview(Some(Filter::new(Kind::Invert))));
    let s = d.snapshot(None);
    assert_eq!(s.stack.image(ink).unwrap().pixel(15, 15), [200, 10, 10, 255]);
}

/// Rect-based steps of old files become patches and still undo.
#[test]
fn old_rect_steps_are_replayed_as_patches() {
    let mut d = doc(64, 64);
    let ink = add_layer(&mut d);
    paint(&mut d, Rect::new(0, 0, 64, 64), [5, 5, 5, 255]);
    let mut snap = d.snapshot(None);
    // An old step: the rect as it was before (transparent), LZ4.
    let r = Rect::new(8, 8, 24, 24);
    let raw = vec![0u8; r.area() as usize * 4];
    snap.undo = vec![aurastudio_file::Step { label: "Brush stroke".into(), layer: ink, rect: Some([8, 8, 24, 24]), data: lz4_flex::compress_prepend_size(&raw) }];
    snap.redo.clear();
    let mut b = Document::from_snapshot(DocId(2), "t", snap);
    assert_eq!(b.history().undo_len(), 1);
    b.apply(Edit::Undo);
    assert_eq!(px(&b, ink, 10, 10), [0; 4]);
    assert_eq!(px(&b, ink, 30, 30), [5, 5, 5, 255]);
    b.apply(Edit::Redo);
    assert_eq!(px(&b, ink, 10, 10), [5, 5, 5, 255]);
}

#[test]
fn documents_have_separate_histories() {
    let mut ws = Workspace::new();
    let a = ws.new_document("a", 32, 32, DocType::Illustration);
    let b = ws.new_document("b", 32, 32, DocType::Comics);
    {
        let mut a = a.lock().unwrap();
        add_layer(&mut a);
        paint(&mut a, Rect::new(0, 0, 4, 4), [1, 1, 1, 255]);
    }
    assert!(!b.lock().unwrap().apply(Edit::Undo).changed());
    assert!(a.lock().unwrap().apply(Edit::Undo).changed());
    assert_eq!(b.lock().unwrap().stack().tile, DocType::Comics.default_tile_size());
    let id = b.lock().unwrap().id();
    assert!(ws.close(id));
    assert_eq!(ws.len(), 1);
}

#[test]
fn resize_keeps_everything_and_drops_the_history() {
    let mut d = doc(64, 64);
    let id = add_layer(&mut d);
    paint(&mut d, Rect::new(0, 0, 4, 4), [1, 1, 1, 255]);
    let r = d.apply(Edit::Resize { width: 128, height: 64 });
    assert_eq!(r.region, Region::Whole);
    assert_eq!(d.history().undo_len(), 0);
    assert_eq!(px(&d, id, 32, 0), [1, 1, 1, 255]);
    let _ = Selection::all(d.space());
}

/// An opacity dragged or a name typed is one undo step.
#[test]
fn a_setting_changed_again_and_again_is_one_step() {
    let mut d = doc(32, 32);
    let id = add_layer(&mut d);
    let n = d.history().undo_len();
    for k in 1..=10 { d.apply(Edit::SetOpacity(id, 1.0 - k as f32 * 0.05)); }
    for name in ["a", "ab", "abc"] { d.apply(Edit::Rename(id, name.into())); }
    assert_eq!(d.history().undo_len(), n + 2);
    d.apply(Edit::Undo);
    assert_eq!(d.stack().node(id).unwrap().name(), "ink");
    d.apply(Edit::Undo);
    assert_eq!(d.stack().node(id).unwrap().opacity(), 1.0);
}

/// A stroke over many pen samples, the document let go in between: one
/// undo step, the same as painted at once; an edit of pixels meanwhile
/// ends it first.
#[test]
fn a_stroke_set_aside_between_samples_is_one_step() {
    let mut d = doc(64, 64);
    let id = add_layer(&mut d);
    let n = d.history().undo_len();
    assert!(d.begin_stroke("Brush stroke", None).refused.is_none());
    for k in 0..4 { assert!(d.paint(|s| { s.prepare(Rect::new(k * 10, 0, k * 10 + 8, 8)); rect_in(s, Rect::new(k * 10, 0, k * 10 + 8, 8), [1, 2, 3, 255]); }).is_some()); }
    assert!(d.stroking());
    d.end_stroke();
    assert_eq!(d.history().undo_len(), n + 1);
    assert_eq!(px(&d, id, 35, 4), [1, 2, 3, 255]);
    d.begin_stroke("Brush stroke", None);
    d.paint(|s| { s.prepare(Rect::new(0, 20, 8, 28)); rect_in(s, Rect::new(0, 20, 8, 28), [9, 9, 9, 255]); });
    d.apply(Edit::Undo);
    assert!(!d.stroking());
    assert_eq!(px(&d, id, 4, 24)[3], 0, "the open stroke ended, then was undone");
    // A stay-inside limit: only inside it.
    let lim = Selection::rect(d.space(), Rect::new(0, 40, 10, 50));
    d.begin_stroke("Brush stroke", lim);
    d.paint(|s| { s.prepare(Rect::new(0, 40, 30, 50)); rect_in(s, Rect::new(0, 40, 30, 50), [5, 5, 5, 255]); });
    d.end_stroke();
    assert_eq!(px(&d, id, 5, 45), [5, 5, 5, 255]);
    assert_eq!(px(&d, id, 20, 45)[3], 0);
}

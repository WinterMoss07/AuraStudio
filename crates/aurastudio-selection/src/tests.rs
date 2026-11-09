use aurastudio_core::{Point, Rect};
use aurastudio_raster::Image;

use crate::*;

fn sp(w: u32, h: u32) -> Space { Space { width: w, height: h, tile: 32 } }
fn pt(x: f64, y: f64) -> Point { Point::new(x, y) }
fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect { Rect::new(x0, y0, x1, y1) }

#[test]
fn rect_and_lasso_select_the_pixels_inside() {
    let s = Selection::rect(sp(100, 100), r(10, 20, 30, 40)).unwrap();
    assert!(s.contains(10, 20) && s.contains(29, 39) && !s.contains(30, 20) && !s.contains(9, 25));
    assert_eq!(s.bbox(), r(10, 20, 30, 40));
    assert!(s.is_rect());
    // Past the frame: still selected (the canvas is unbounded).
    let out = Selection::rect(sp(100, 100), r(200, -10, 300, 10)).unwrap();
    assert!(out.contains(250, -5));
    assert!(Selection::rect(sp(100, 100), r(5, 5, 5, 9)).is_none());

    // A triangle: its tip is narrow, its base wide.
    let t = Selection::polygon(sp(100, 100), &[pt(50.0, 10.0), pt(90.0, 90.0), pt(10.0, 90.0)], false).unwrap();
    assert!(t.contains(50, 50) && t.contains(15, 89));
    assert!(!t.contains(15, 15) && !t.contains(50, 95));
    let big = Selection::polygon(sp(100, 100), &[pt(-50.0, -50.0), pt(500.0, -50.0), pt(500.0, 500.0)], true).unwrap();
    assert!(big.contains(99, 99) && big.contains(400, 0) && big.bbox().x1 <= 500);
    // Bands of rows: a shape taller than one band is whole.
    let tall = Selection::polygon(sp(100, 100), &[pt(3.0, 1.0), pt(9.0, 1.0), pt(9.0, 300.0), pt(3.0, 300.0)], false).unwrap();
    assert_eq!((tall.bbox(), tall.mask(tall.bbox()).iter().all(|v| *v == 255)), (r(3, 1, 9, 300), true));
    // A stray point far away doesn't fill memory.
    let far = Selection::polygon(sp(100, 100), &[pt(0.0, 0.0), pt(1e9, 0.0), pt(0.0, 1e9)], false).unwrap();
    assert!(far.bbox().x1 <= 100 + 4096 && far.contains(50, 50));
    assert!(Selection::polygon(sp(100, 100), &[pt(1.0, 1.0), pt(2.0, 2.0)], false).is_none());
    assert!(Selection::polygon(sp(100, 100), &[pt(f64::NAN, 1.0), pt(2.0, 2.0), pt(3.0, 1.0)], false).is_none());
}

#[test]
fn anti_aliased_shapes_have_soft_edges() {
    let s = Selection::polygon(sp(20, 20), &[pt(2.0, 2.0), pt(10.5, 2.0), pt(10.5, 10.0), pt(2.0, 10.0)], true).unwrap();
    assert_eq!(s.coverage(5, 5), 255);
    assert!((120..=135).contains(&s.coverage(10, 5)), "{}", s.coverage(10, 5));
    assert_eq!(s.coverage(11, 5), 0);
    let e = Selection::ellipse(sp(200, 200), [20.0, 20.0, 180.0, 120.0], true).unwrap();
    assert_eq!(e.coverage(100, 70), 255);
    assert_eq!(e.coverage(25, 25), 0);
    let m = e.mask(e.bbox());
    assert!(m.iter().any(|v| *v > 0 && *v < 255));
    let area: f32 = m.iter().map(|v| *v as f32 / 255.0).sum();
    assert!((area - std::f32::consts::PI * 80.0 * 50.0).abs() < 20.0, "{area}");
}

#[test]
fn modes_add_subtract_intersect() {
    let s = sp(100, 100);
    let a = Selection::rect(s, r(0, 0, 60, 100));
    let b = Selection::rect(s, r(40, 0, 100, 100));
    assert_eq!(Selection::combine(a.as_ref(), b.clone(), Mode::Add).unwrap().bbox(), r(0, 0, 100, 100));
    assert_eq!(Selection::combine(a.as_ref(), b.clone(), Mode::Subtract).unwrap().bbox(), r(0, 0, 40, 100));
    let int = Selection::combine(a.as_ref(), b.clone(), Mode::Intersect).unwrap();
    assert_eq!(int.bbox(), r(40, 0, 60, 100));
    assert!(int.is_rect());
    assert!(Selection::combine(a.as_ref(), Selection::rect(s, r(70, 0, 90, 9)), Mode::Intersect).is_none());
    assert_eq!(Selection::combine(None, b.clone(), Mode::Add), b);
    assert!(Selection::combine(None, b, Mode::Subtract).is_none());
    let inv = a.unwrap().inverted().unwrap();
    assert_eq!(inv.bbox(), r(60, 0, 100, 100));
    assert!(Selection::all(s).inverted().is_none());
}

#[test]
fn grow_shrink_feather_border_and_move() {
    let s = Selection::rect(sp(200, 200), r(50, 50, 150, 150)).unwrap();
    let g = s.grown(10.0).unwrap();
    assert_eq!(g.bbox(), r(40, 40, 160, 160));
    assert_eq!(g.coverage(40, 100), 255);
    assert_eq!(g.coverage(41, 41), 0, "round corners");
    let k = s.shrunk(10.0).unwrap();
    assert_eq!(k.coverage(60, 100), 255);
    assert!(k.coverage(59, 100) < 255 && k.coverage(55, 100) == 0);
    // The frame's edge stays selected.
    let edge = Selection::rect(sp(200, 200), r(0, 0, 100, 200)).unwrap().shrunk(10.0).unwrap();
    assert_eq!(edge.coverage(0, 100), 255);
    assert_eq!(edge.coverage(95, 100), 0);
    let f = s.feathered(5.0).unwrap();
    assert!((100..=155).contains(&f.coverage(50, 100)), "{}", f.coverage(50, 100));
    assert_eq!(f.coverage(100, 100), 255);
    assert!(f.bbox().x0 < 50);
    let b = s.bordered(4.0).unwrap();
    assert_eq!(b.coverage(100, 100), 0);
    assert!(b.coverage(50, 100) > 200 && b.coverage(49, 100) > 200);
    // Moved: nothing lost past the frame.
    let m = s.translated(-60, 10).unwrap();
    assert_eq!(m.bbox(), r(-10, 60, 90, 160));
    assert!(m.is_rect());
    assert_eq!(s.translated(500, 0).unwrap().bbox(), r(550, 50, 650, 150));
}

#[test]
fn outline_follows_the_half_coverage() {
    let s = Selection::rect(sp(100, 100), r(2, 3, 10, 7)).unwrap();
    let o = s.outline(1);
    assert_eq!(o.len(), 4, "{o:?}");
    assert!(o.contains(&[pt(2.0, 3.0), pt(10.0, 3.0)]));
    assert!(o.contains(&[pt(10.0, 3.0), pt(10.0, 7.0)]));
    let hole = Selection::rect(sp(100, 100), r(0, 0, 50, 50)).unwrap();
    let hole = Selection::combine(Some(&hole), Selection::rect(sp(100, 100), r(20, 20, 30, 30)), Mode::Subtract).unwrap();
    assert_eq!(hole.outline(1).len(), 8);
    for seg in hole.outline(8) { for p in seg { assert!(p.x >= 0.0 && p.x <= 50.0 && p.y <= 50.0); } }
    // Past the frame, negative coordinates.
    let neg = Selection::rect(sp(100, 100), r(-20, -9, -4, 5)).unwrap();
    assert!(neg.outline(1).contains(&[pt(-20.0, -9.0), pt(-4.0, -9.0)]));
}

#[test]
fn masks_alpha_and_bytes_round_trip() {
    let s = sp(70, 70);
    let a = Selection::all(s);
    assert_eq!(a, Selection::rect(s, r(0, 0, 70, 70)).unwrap());
    assert_eq!(a, Selection::from_mask(s, r(0, 0, 70, 70), &vec![255; 70 * 70]).unwrap());
    assert_eq!(a.mask(r(60, 60, 80, 80)).iter().filter(|v| **v == 255).count(), 100);
    let soft = Selection::from_mask(s, r(0, 0, 4, 1), &[0, 128, 255, 0]).unwrap();
    assert_eq!(soft.bbox(), r(1, 0, 3, 1));
    let back = Selection::from_bytes(&soft.to_bytes(), 1 << 16).unwrap();
    assert_eq!(back, soft);
    assert!(Selection::from_bytes(&[1, 2, 3], 1 << 16).is_none());
    // A layer's alpha; a paper selects its frame.
    let mut img = Image::new(70, 70, 32, [0, 0, 0, 0]);
    img.fill_rect(r(10, 10, 20, 20), [9, 9, 9, 200]);
    let al = Selection::from_alpha(&img).unwrap();
    assert_eq!((al.bbox(), al.coverage(15, 15)), (r(10, 10, 20, 20), 200));
    assert_eq!(Selection::from_alpha(&Image::new(70, 70, 32, [255; 4])).unwrap(), a);
    let p = Selection::from_predicate(s, r(0, 0, 70, 70), |x, y| x == y).unwrap();
    assert_eq!(p.mask(p.bbox()).iter().filter(|v| **v == 255).count(), 70);
}

/// A rectangle grown, shrunk or bordered from its sides gives what the
/// distance map gives for the same pixels as a mask.
#[test]
fn rectangles_change_like_masks() {
    let space = sp(200, 120);
    for b in [r(50, 30, 150, 90), r(0, 0, 120, 120), r(0, 0, 200, 120)] {
        let rect = Selection::rect(space, b).unwrap();
        let mask = Selection::from_mask(space, b, &vec![255; b.area() as usize]).unwrap();
        assert!(rect.is_rect() && !mask.is_rect());
        let all = r(-20, -20, 220, 140);
        let same = |a: Option<Selection>, m: Option<Selection>, what: &str| {
            let (a, m) = (a.map(|s| s.mask(all)), m.map(|s| s.mask(all)));
            assert_eq!(a, m, "{what} of {b:?}");
        };
        for k in [1.0, 2.5, 7.0] {
            same(rect.grown(k), mask.grown(k), "grown");
            same(rect.shrunk(k), mask.shrunk(k), "shrunk");
            same(rect.bordered(k), mask.bordered(k), "bordered");
        }
    }
}

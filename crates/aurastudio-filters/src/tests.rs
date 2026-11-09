use super::*;
use aurastudio_core::Rgba8;

fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> Rgba8) -> Vec<Rgba8> {
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
}

fn with(kind: Kind, values: &[f32]) -> Filter { Filter { kind, values: values.to_vec(), color: [0, 0, 0] } }

#[test]
fn neutral_settings_change_nothing() {
    let src = img(16, 8, |x, y| [(x * 15) as u8, (y * 30) as u8, 77, 255]);
    for f in [with(Kind::BrightnessContrast, &[0.0, 0.0, 0.0]), Filter::new(Kind::Levels), Filter::new(Kind::HueSaturation),
              with(Kind::Invert, &[0.0]), with(Kind::Sepia, &[0.0]), with(Kind::Opacity, &[100.0]),
              with(Kind::GaussianBlur, &[0.0]), with(Kind::Thicken, &[0.0, 0.0]), with(Kind::UnsharpMask, &[1.0, 2.0, 255.0])] {
        let out = f.apply(&src, 16, 8);
        let worst = out.iter().flatten().zip(src.iter().flatten()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "{:?}: {worst}", f.kind);
    }
}

#[test]
fn blur_keeps_the_total_and_spreads() {
    let (w, h) = (64, 64);
    let src = img(w, h, |x, y| if (28..36).contains(&x) && (28..36).contains(&y) { [255, 0, 0, 255] } else { [0, 0, 0, 0] });
    for sigma in [0.8f32, 3.0, 8.0] {
        let out = with(Kind::GaussianBlur, &[sigma]).apply(&src, w, h);
        let total = |v: &[Rgba8]| v.iter().map(|p| p[3] as u64).sum::<u64>();
        let (a, b) = (total(&src) as f64, total(&out) as f64);
        assert!((a - b).abs() / a < 0.03, "σ {sigma}: alpha {a} -> {b}");
        assert!(out[27 * w + 32][3] > 0, "σ {sigma}: nothing spread");
        let p = &out[26 * w + 32];
        if p[3] > 10 { assert!(p[0] > 240 && p[1] < 10, "σ {sigma}: {p:?}"); }
    }
}

#[test]
fn thicken_and_thin_work_on_paper_and_on_line_art() {
    let (w, h) = (32, 32);
    let dark = |v: &[Rgba8], y: usize| { let p = &v[y * w + 5]; p[3] > 0 && p[0] < 128 };
    let rows = |v: &[Rgba8]| (0..h).filter(|&y| dark(v, y)).count();
    // A black line on a transparent layer.
    let art = img(w, h, |_, y| if y == 16 { [0, 0, 0, 255] } else { [255, 255, 255, 0] });
    assert_eq!(rows(&with(Kind::Thicken, &[2.0, 0.0]).apply(&art, w, h)), 5);
    // A black line on white paper.
    let paper = img(w, h, |_, y| if (12..20).contains(&y) { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
    assert_eq!(rows(&with(Kind::Thicken, &[2.0, 1.0]).apply(&paper, w, h)), 12);
    assert_eq!(rows(&with(Kind::Thin, &[2.0, 0.0]).apply(&paper, w, h)), 4);
    let thick = img(w, h, |_, y| if (12..20).contains(&y) { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
    assert_eq!(rows(&with(Kind::Thin, &[2.0, 0.0]).apply(&thick, w, h)), 4);
}

#[test]
fn threshold_posterize_and_levels() {
    let src = img(4, 1, |x, _| { let v = [10u8, 100, 160, 250][x]; [v, v, v, 255] });
    let t = with(Kind::Threshold, &[128.0]).apply(&src, 4, 1);
    assert_eq!(t.iter().map(|p| p[0]).collect::<Vec<_>>(), [0, 0, 255, 255]);
    let p = with(Kind::Posterize, &[2.0]).apply(&src, 4, 1);
    assert!(p.iter().all(|q| q[0] == 0 || q[0] == 255));
    // Red channel's white point at 128: red doubles, green stays.
    let mut l = Filter::new(Kind::Levels);
    l.values[idx::levels(1) + 2] = 128.0;
    let o = l.apply(&[[64, 64, 64, 255]], 1, 1)[0];
    assert!((o[0] as i32 - 128).abs() <= 1 && o[1] == 64, "{o:?}");
}

#[test]
fn drop_shadow_lands_away_from_the_light() {
    let (w, h) = (32, 32);
    let src = img(w, h, |x, y| if (8..16).contains(&x) && (8..16).contains(&y) { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
    // Light from 135° (top left), 6 px, hard: shadow below right.
    let f = Filter { kind: Kind::DropShadow, values: vec![135.0, 8.5, 0.0, 0.0, 100.0], color: [0, 0, 255] };
    let out = f.apply(&src, w, h);
    let at = |x: usize, y: usize| out[y * w + x];
    assert_eq!(at(10, 10), [255, 255, 255, 255], "source on top");
    assert_eq!(at(19, 19), [0, 0, 255, 255], "shadow below right");
    assert_eq!(at(4, 4)[3], 0);
}

#[test]
fn find_edges_outlines_line_art() {
    let (w, h) = (16, 16);
    let art = img(w, h, |x, _| if x == 8 { [0, 0, 0, 255] } else { [255, 255, 255, 0] });
    let out = Filter::new(Kind::FindEdges).apply(&art, w, h);
    let p = &out[8 * w + 7];
    assert!(p[3] > 200 && p[0] < 60, "an edge beside the line: {p:?}");
    assert_eq!(out[8 * w + 2][3], 0, "nothing far from it");
}

#[test]
fn histogram_counts_painted_pixels() {
    let src = img(4, 1, |x, _| [x as u8 * 80, 0, 0, if x == 3 { 0 } else { 255 }]);
    let h = Histogram::of(&src, |_| true);
    assert_eq!(h.count, 3);
    assert_eq!((h.chan[0][0], h.chan[0][80], h.chan[0][240]), (1, 1, 0));
    assert_eq!(h.rgb[0], 7);
}

#[test]
fn every_filter_runs_on_its_defaults_and_scales() {
    let (w, h) = (24, 20);
    let src = img(w, h, |x, y| [(x * 10) as u8, (y * 12) as u8, 128, if x > 3 { 255 } else { 0 }]);
    for (_, kinds) in GROUPS {
        for &k in *kinds {
            let f = Filter::new(k);
            assert_eq!(f.apply(&src, w, h).len(), src.len(), "{k:?}");
            assert_eq!(f.scaled(0.5).apply(&src, w, h).len(), src.len(), "{k:?}");
            for p in k.params() { assert!(p.index < f.values.len(), "{k:?} {}", p.name); }
        }
    }
}

// --- On surfaces ------------------------------------------------------

/// A transparent pixel's color doesn't show: all of them alike.
fn seen(p: Rgba8) -> Rgba8 { if p[3] == 0 { [0; 4] } else { p } }

use aurastudio_core::Rect;
use aurastudio_raster::{Image, Mask};

/// A layer with a painted patch (varied colors, holes, a uniform tile
/// and line art) on a transparent unbounded canvas, 32 px tiles.
fn layer() -> Image {
    let mut s = Image::new(160, 120, 32, [0, 0, 0, 0]);
    let r = Rect::new(20, 10, 100, 90);
    let px = img(80, 80, |x, y| {
        if (x / 9 + y / 7) % 4 == 0 { [0, 0, 0, 0] } else { [(x * 3) as u8, (y * 3) as u8, ((x + y) * 5) as u8, (128 + x) as u8] }
    });
    s.write(r, &px);
    s.fill_rect(Rect::new(128, 64, 160, 96), [10, 200, 30, 255]);
    let ink: Vec<u8> = (0..32 * 32).map(|i| if (i % 32) % 5 == 0 { 255 } else { (i % 7) as u8 * 30 }).collect();
    s.fill_mask(Rect::new(-32, 96, 0, 128), &ink, [20, 20, 20, 255]);
    s
}

fn every_filter() -> Vec<Filter> {
    let mut v: Vec<Filter> = Kind::all().map(Filter::new).collect();
    v.push(with(Kind::GaussianBlur, &[1.2]));
    v.push(with(Kind::Thicken, &[3.0, 1.0]));
    v.push(Filter { values: vec![200.0, 9.0, 40.0, 10.0, 80.0], ..Filter::new(Kind::DropShadow) });
    v
}

/// The layer filtered tile by tile equals the whole picture filtered at
/// once (wide enough that its edges are fill).
#[test]
fn on_a_layer_as_on_the_whole_picture() {
    for f in every_filter() {
        let mut s = layer();
        let before = s.clone();
        let reach = f.reach() as i32;
        let view = before.stored_bounds().unwrap().grown(2 * reach + 8);
        let want = f.apply(&before.read(view), view.w() as usize, view.h() as usize);
        f.apply_to(&mut s, None);
        let got: Vec<Rgba8> = s.read(view).into_iter().map(seen).collect();
        let want: Vec<Rgba8> = want.into_iter().map(seen).collect();
        let worst = got.iter().flatten().zip(want.iter().flatten()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
        assert!(worst == 0, "{:?} {:?}: off by {worst}", f.kind, f.values);
        // The uniform tile stayed one color (no pixels stored for it).
        if f.kind.is_point() { assert!(s.uniform_in(Rect::new(128, 64, 160, 96)).is_some(), "{:?}", f.kind); }
    }
}

#[test]
fn a_selection_keeps_the_rest_exactly() {
    let mut sel = Mask::new(160, 120, 32, 0);
    sel.fill_rect(Rect::new(40, 30, 70, 60), 255);
    sel.fill_rect(Rect::new(70, 30, 80, 60), 128);
    for f in every_filter() {
        let mut s = layer();
        let before = s.clone();
        let full = { let mut t = layer(); f.apply_to(&mut t, None); t };
        f.apply_to(&mut s, Some(&sel));
        let view = Rect::new(-40, 0, 170, 130);
        for y in view.y0..view.y1 {
            for x in view.x0..view.x1 {
                let (got, old, new) = (seen(s.pixel(x, y)), seen(before.pixel(x, y)), seen(full.pixel(x, y)));
                match sel.pixel(x, y) {
                    0 => assert_eq!(got, old, "{:?} outside at {x},{y}", f.kind),
                    255 => assert_eq!(got, new, "{:?} inside at {x},{y}", f.kind),
                    m => assert_eq!(got, seen(mix(old, new, m)), "{:?} edge at {x},{y}", f.kind),
                }
            }
        }
    }
}

#[test]
fn a_stopped_filter_says_so() {
    let f = with(Kind::GaussianBlur, &[3.0]);
    let mut s = layer();
    assert_eq!(f.apply_until(&mut s, None, &|| true), Err(crate::Stopped));
    let mut t = layer();
    assert_eq!(f.apply_until(&mut t, None, &|| false), Ok(f.apply_to(&mut layer(), None)));
}

#[test]
fn the_paper_changes_everywhere() {
    let mut s = Image::new(64, 64, 32, [255, 255, 255, 255]);
    s.fill_rect(Rect::new(0, 0, 8, 8), [0, 0, 0, 255]);
    let changed = Filter::new(Kind::Invert).apply_to(&mut s, None);
    assert_eq!(s.fill(), [0, 0, 0, 255]);
    assert_eq!(s.pixel(10_000, -500), [0, 0, 0, 255]);
    assert_eq!(s.pixel(3, 3), [255, 255, 255, 255]);
    assert!(changed.is_some_and(|r| r.contains_rect(&s.rect())));
    // A blur on a transparent canvas leaves the far fill alone.
    let mut t = Image::new(64, 64, 32, [0, 0, 0, 0]);
    assert_eq!(with(Kind::GaussianBlur, &[3.0]).apply_to(&mut t, None), None);
}

#[test]
fn mask_feather_and_spread_match_flat_planes() {
    let mut m = Mask::new(100, 100, 32, 0);
    m.fill_rect(Rect::new(20, 20, 60, 50), 255);
    m.fill_rect(Rect::new(70, 10, 75, 90), 180);
    let view = Rect::new(-40, -40, 140, 140);
    let flat = m.read(view);
    let (w, h) = (view.w() as usize, view.h() as usize);
    let mut a = m.clone();
    blur_mask(&mut a, 4.0);
    assert_eq!(a.read(view), blur::blur_u8(&flat, w, h, 4.0));
    let mut b = m.clone();
    spread_mask(&mut b, 3.0, true, true);
    let mut want = flat.clone();
    extreme::disc(&mut want, w, h, 3.0, u8::max);
    assert_eq!(b.read(view), want);
}

#[test]
fn histogram_of_a_layer() {
    let s = layer();
    let r = Rect::new(-40, 0, 170, 130);
    assert_eq!(Histogram::of_layer(&s, r, None), Histogram::of(&s.read(r), |_| true));
    let mut sel = Mask::new(160, 120, 32, 0);
    sel.fill_rect(Rect::new(30, 30, 90, 70), 200);
    let m = sel.read(r);
    assert_eq!(Histogram::of_layer(&s, r, Some(&sel)), Histogram::of(&s.read(r), |i| m[i] > 0));
}

#[test]
fn codes_are_stable_and_unique() {
    let codes: Vec<u8> = Kind::all().map(Kind::code).collect();
    assert_eq!(codes, (1..=16).collect::<Vec<u8>>());
    for k in Kind::all() { assert_eq!(Kind::from_code(k.code()), Some(k)); }
}

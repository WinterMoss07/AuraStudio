use aurastudio_core::{BlendMode, Rect, Rgba8};
use aurastudio_filters::{Filter, Kind};
use aurastudio_layers::{FolderBlend, Ghost, LayerStack, Node, Pick, Start};
use aurastudio_raster::Image;

use crate::*;

const W: i32 = 96;
const H: i32 = 64;

fn stack() -> LayerStack { LayerStack::new(W as u32, H as u32, 32, Start::Background) }
fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect { Rect::new(x0, y0, x1, y1) }

fn paint(s: &mut LayerStack, name: &str, rect: Rect, c: Rgba8) -> u64 {
    let id = s.add_layer(name);
    assert!(s.set_layer_rect(id, rect, &vec![c; rect.area() as usize]));
    id
}

fn px(s: &LayerStack, x: i32, y: i32) -> Rgba8 { merge(s, r(x, y, x + 1, y + 1), Opts::default())[0] }
fn close(a: Rgba8, b: Rgba8) -> bool { a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1) }
fn worst(a: &[Rgba8], b: &[Rgba8]) -> u8 { a.iter().flatten().zip(b.iter().flatten()).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0) }

#[test]
fn plain_merges() {
    assert_eq!(px(&stack(), 0, 0), [255; 4]);
    let mut s = stack();
    s.set_layer_rect(1, r(0, 0, W, H), &vec![[128, 128, 128, 255]; (W * H) as usize]);
    let l = paint(&mut s, "L", r(0, 0, W, H), [255, 0, 0, 255]);
    if let Some(Node::Layer(x)) = s.node_mut(l) { x.blend = BlendMode::Multiply; }
    assert_eq!(px(&s, 0, 0), [128, 0, 0, 255]);
    *s.node_mut(l).unwrap().visible_mut() = false;
    assert_eq!(px(&s, 0, 0), [128, 128, 128, 255]);
    // An isolated multiply folder; then pass-through at 50 %.
    let mut t = stack();
    t.set_layer_rect(1, r(0, 0, W, H), &vec![[128, 128, 128, 255]; (W * H) as usize]);
    let f = t.add_folder("F");
    paint(&mut t, "L", r(0, 0, W, H), [255, 0, 0, 255]);
    if let Some(Node::Folder(x)) = t.node_mut(f) { x.blend = FolderBlend::Isolated(BlendMode::Multiply); }
    assert_eq!(px(&t, 0, 0), [128, 0, 0, 255]);
    if let Some(Node::Folder(x)) = t.node_mut(f) { x.blend = FolderBlend::PassThrough; x.opacity = 0.5; }
    assert!(close(px(&t, 0, 0), [192, 64, 64, 255]), "{:?}", px(&t, 0, 0));
    // Past the frame: the canvas goes on.
    let mut u = stack();
    paint(&mut u, "far", r(-50, -50, -40, -40), [0, 0, 255, 255]);
    assert_eq!(px(&u, -45, -45), [0, 0, 255, 255]);
    // Transparent backdrop: what the layers cover.
    let mut v = LayerStack::new(8, 8, 32, Start::Background);
    let bg = v.layer_ids()[0];
    *v.node_mut(bg).unwrap().visible_mut() = false;
    paint(&mut v, "half", r(0, 0, 8, 8), [10, 20, 30, 128]);
    assert_eq!(merge(&v, r(0, 0, 1, 1), Opts { backdrop: Backdrop::Transparent, ..Default::default() })[0], [10, 20, 30, 128]);
}

#[test]
fn clipping_shows_only_over_the_base() {
    let mut s = stack();
    let base = paint(&mut s, "base", r(0, 0, 48, 64), [255, 0, 0, 255]);
    let c = paint(&mut s, "clipped", r(0, 0, 96, 64), [0, 0, 255, 255]);
    let d = paint(&mut s, "clipped 2", r(0, 0, 20, 64), [0, 255, 0, 255]);
    s.node_mut(c).unwrap().props_mut().clip = true;
    s.node_mut(d).unwrap().props_mut().clip = true;
    assert!(needs_general(&s));
    assert!(close(px(&s, 30, 5), [0, 0, 255, 255]));
    assert!(close(px(&s, 70, 5), [255; 4]));
    assert!(close(px(&s, 10, 5), [0, 255, 0, 255]));
    s.node_mut(base).unwrap().set_opacity(0.5);
    assert!(close(px(&s, 30, 5), [128, 128, 255, 255]), "{:?}", px(&s, 30, 5));
    *s.node_mut(base).unwrap().visible_mut() = false;
    assert!(close(px(&s, 30, 5), [255; 4]));
}

#[test]
fn filter_layers() {
    let mut s = stack();
    paint(&mut s, "gray", r(0, 0, 96, 64), [100, 100, 100, 255]);
    let f = s.add_filter_layer(Filter::new(Kind::Invert));
    assert!(close(px(&s, 5, 5), [155, 155, 155, 255]));
    s.node_mut(f).unwrap().set_opacity(0.5);
    assert!(close(px(&s, 5, 5), [128, 128, 128, 255]));
    s.node_mut(f).unwrap().set_opacity(1.0);
    s.set_active(f);
    paint(&mut s, "over", r(0, 0, 10, 10), [10, 20, 30, 255]);
    assert_eq!(px(&s, 5, 5), [10, 20, 30, 255]);
    // In an isolated folder: only its content.
    let mut t = stack();
    paint(&mut t, "outside", r(0, 0, 96, 64), [100, 100, 100, 255]);
    let g = t.add_folder("f");
    if let Some(Node::Folder(x)) = t.node_mut(g) { x.blend = FolderBlend::Isolated(BlendMode::Normal); }
    paint(&mut t, "inside", r(0, 0, 48, 64), [40, 40, 40, 255]);
    t.add_filter_layer(Filter::new(Kind::Invert));
    assert!(close(px(&t, 10, 5), [215, 215, 215, 255]), "{:?}", px(&t, 10, 5));
    assert!(close(px(&t, 70, 5), [100, 100, 100, 255]));
}

#[test]
fn a_blur_layer_gives_the_same_pixels_in_parts() {
    let mut s = stack();
    paint(&mut s, "dot", r(40, 24, 56, 40), [0, 0, 0, 255]);
    s.add_filter_layer(Filter { values: vec![6.0], ..Filter::new(Kind::GaussianBlur) });
    let whole = merge(&s, r(0, 0, W, H), Opts::default());
    for part in [r(30, 20, 50, 30), r(0, 0, 8, 8), r(50, 30, 96, 64)] {
        let got = merge(&s, part, Opts::default());
        for y in part.y0..part.y1 {
            for x in part.x0..part.x1 {
                assert_eq!(whole[(y * W + x) as usize], got[((y - part.y0) * part.w() as i32 + x - part.x0) as usize], "({x}, {y}) in {part:?}");
            }
        }
    }
    let p = px(&s, 37, 32);
    assert!(p[0] > 20 && p[0] < 235, "{p:?}");
}

#[test]
fn picks_leave_out_drafts_and_keep_references() {
    let mut s = stack();
    let ink = paint(&mut s, "ink", r(0, 0, 10, 64), [0, 0, 0, 255]);
    let sketch = paint(&mut s, "sketch", r(20, 0, 30, 64), [0, 0, 255, 255]);
    s.node_mut(sketch).unwrap().props_mut().draft = true;
    let at = |s: &LayerStack, x: i32, pick: Pick, t: bool| merge(s, r(x, 5, x + 1, 6), Opts { pick, backdrop: if t { Backdrop::Transparent } else { Backdrop::White }, ..Default::default() })[0];
    assert_eq!(at(&s, 25, Pick::ALL, false), [0, 0, 255, 255]);
    assert_eq!(at(&s, 25, Pick::EXPORT, false), [255; 4]);
    assert_eq!(at(&s, 5, Pick { reference: Some(0), ..Pick::ALL }, true)[3], 0);
    s.set_reference(ink, true);
    let refs = Pick { reference: s.reference_for(sketch), ..Pick::ALL };
    assert_eq!(at(&s, 5, refs, true), [0, 0, 0, 255]);
    assert_eq!(at(&s, 25, refs, true)[3], 0);
    assert_eq!(at(&s, 25, Pick { skip: sketch, ..Pick::ALL }, false), [255; 4]);
    let under = Pick { until: sketch, ..Pick::ALL };
    assert_eq!(at(&s, 25, under, false), [255; 4]);
    assert_eq!(at(&s, 5, under, false), [0, 0, 0, 255]);
    // The fill's view.
    let m = Merged { stack: &s, pick: Pick::EXPORT };
    use aurastudio_layers::FillSource;
    assert_eq!(m.pixels(r(25, 5, 26, 6))[0], [255; 4]);
}

/// Random stacks: every piece size gives the whole merge's pixels, the
/// occlusion and the folder cache change nothing.
#[test]
fn pieces_occlusion_and_cache_change_nothing() {
    let mut rng = aurastudio_core::Rng::new(11);
    for _ in 0..12 {
        let (w, h) = (rng.below(300) as i32 + 20, rng.below(200) as i32 + 20);
        let mut s = LayerStack::new(w as u32, h as u32, 16, Start::Paper);
        for _ in 0..rng.below(6) + 2 {
            if rng.chance(0.3) {
                let f = s.add_folder("f");
                if rng.chance(0.5) { if let Some(Node::Folder(x)) = s.node_mut(f) { x.blend = FolderBlend::Isolated(BlendMode::ALL[rng.below(13) as usize]); x.opacity = rng.range(0.3, 1.0); } }
            }
            let (x, y) = (rng.below(w as u64) as i32 - 10, rng.below(h as u64) as i32 - 10);
            let rect = r(x, y, x + rng.below(150) as i32 + 1, y + rng.below(150) as i32 + 1);
            let c = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8, if rng.chance(0.5) { 255 } else { rng.below(256) as u8 }];
            let id = paint(&mut s, "l", rect, c);
            if let Some(Node::Layer(l)) = s.node_mut(id) {
                l.blend = if rng.chance(0.6) { BlendMode::Normal } else { BlendMode::ALL[rng.below(13) as usize] };
                l.opacity = if rng.chance(0.5) { 1.0 } else { rng.range(0.2, 1.0) };
            }
        }
        let frame = r(-5, -5, w + 5, h + 5);
        let whole = merge(&s, frame, Opts::default());
        let cache = FolderCache::default();
        for _ in 0..2 {
            let cached = merge(&s, frame, Opts { cache: Some(&cache), ..Default::default() });
            assert_eq!(worst(&whole, &cached), 0, "the folder cache");
        }
        let part = r(rng.below(w as u64) as i32, rng.below(h as u64) as i32, w, h);
        let got = merge(&s, part, Opts::default());
        for y in part.y0..part.y1 {
            for x in part.x0..part.x1 {
                assert_eq!(whole[((y - frame.y0) * frame.w() as i32 + x - frame.x0) as usize], got[((y - part.y0) * part.w() as i32 + x - part.x0) as usize]);
            }
        }
    }
}

#[test]
fn occlusion_skips_what_an_opaque_layer_hides() {
    let mut s = stack();
    paint(&mut s, "under", r(0, 0, W, H), [1, 2, 3, 255]);
    let top = paint(&mut s, "cover", r(-200, -200, 400, 400), [9, 9, 9, 255]);
    assert_eq!(px(&s, 10, 10), [9, 9, 9, 255]);
    s.node_mut(top).unwrap().set_opacity(0.5);
    assert!(close(px(&s, 10, 10), [5, 6, 6, 255]));
}

fn noisy(w: i32, h: i32, seed: u32) -> Image {
    let mut img = Image::new(w as u32, h as u32, 16, [0; 4]);
    let px: Vec<Rgba8> = (0..w * h).map(|i| {
        let v = (i as u32).wrapping_mul(2654435761).wrapping_add(seed * 7919);
        if (i as u32 / 7 + seed).is_multiple_of(3) { [0; 4] } else { [(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8, ((v >> 4) as u8) | 0x20] }
    }).collect();
    img.write(r(0, 0, w, h), &px);
    img
}

/// The stroke's cached display matches the full merge (to rounding),
/// before and after the painted layer changes and after another layer
/// changes; with a non-normal layer above, the full merge is used.
#[test]
fn the_stroke_cache_matches_the_full_merge() {
    let (w, h) = (150, 90);
    for multiply_above in [false, true] {
        let mut s = LayerStack::new(w as u32, h as u32, 16, Start::Background);
        let mk = |s: &mut LayerStack, seed, blend, op: f32| {
            let id = s.add_layer("l");
            if let Some(Node::Layer(l)) = s.node_mut(id) { l.surface = noisy(w, h, seed); l.blend = blend; l.opacity = op; }
            id
        };
        mk(&mut s, 1, BlendMode::Multiply, 0.8);
        let f = s.add_folder("f");
        if let Some(Node::Folder(x)) = s.node_mut(f) { x.opacity = 0.7; }
        mk(&mut s, 2, BlendMode::Screen, 1.0);
        let painted = mk(&mut s, 3, BlendMode::Normal, 0.9);
        let other = mk(&mut s, 4, BlendMode::Normal, 0.5);
        s.set_active(f);
        let top = mk(&mut s, 6, if multiply_above { BlendMode::Multiply } else { BlendMode::Normal }, 0.4);
        let _ = top;
        let mut cache = StrokeCache::default();
        for rect in [r(0, 0, w, h), r(10, 5, 140, 80), r(63, 63, 70, 70)] {
            let full = merge(&s, rect, Opts { ghosts: true, ..Default::default() });
            let cached = cache.merge(&s, painted, rect);
            assert!(worst(&full, &cached) <= 2, "rect {rect:?}: off by {}", worst(&full, &cached));
        }
        assert!(s.set_layer_rect(painted, r(20, 20, 60, 50), &vec![[200, 30, 90, 255]; 1200]));
        let full = merge(&s, r(0, 0, w, h), Opts::default());
        let cached = cache.merge(&s, painted, r(0, 0, w, h));
        assert!(worst(&full, &cached) <= 2, "after painting: off by {}", worst(&full, &cached));
        if multiply_above { assert_eq!(full, cached, "the fallback is the full merge"); }
        // Another layer changes: its cells are rebuilt.
        assert!(s.set_layer_rect(other, r(100, 10, 140, 40), &vec![[0, 0, 255, 255]; 1200]));
        let full = merge(&s, r(0, 0, w, h), Opts::default());
        let cached = cache.merge(&s, painted, r(0, 0, w, h));
        assert!(worst(&full, &cached) <= 2, "after another layer changed: off by {}", worst(&full, &cached));
    }
}

#[test]
fn ghosts_show_on_screen_only() {
    let mut s = stack();
    let anim = s.add_folder("A");
    let sheet = s.add_folder("sheet");
    paint(&mut s, "drawing", r(10, 10, 20, 20), [0, 0, 0, 255]);
    *s.node_mut(sheet).unwrap().visible_mut() = false;
    s.set_ghosts(vec![(anim, vec![Ghost { sheet, tint: [1.0, 0.0, 0.0], opacity: 0.53 }])]);
    let screen = merge(&s, r(15, 15, 16, 16), Opts { ghosts: true, ..Default::default() })[0];
    assert!(close(screen, [255, 120, 120, 255]), "{screen:?}");
    assert_eq!(px(&s, 15, 15), [255; 4], "an export has none");
    let mut cache = StrokeCache::default();
    let bg = s.layer_ids()[0];
    let cached = cache.merge(&s, bg, r(0, 0, W, H));
    assert_eq!(cached, merge(&s, r(0, 0, W, H), Opts { ghosts: true, ..Default::default() }));
}

#[test]
fn split_preview_and_gpu_eligibility() {
    let (w, h) = (90, 70);
    let mut l = LayerStack::new(w, h, 32, Start::Paper);
    let low = paint(&mut l, "low", r(0, 0, 60, 70), [200, 40, 40, 255]);
    let folder = l.add_folder("f");
    let target = paint(&mut l, "target", r(20, 10, 80, 50), [30, 200, 60, 180]);
    l.set_active(folder);
    let top = paint(&mut l, "top", r(40, 0, 90, 30), [20, 20, 220, 100]);
    let whole = merge(&l, l.frame(), Opts::default());
    let (below, above) = merge_split(&l, target).unwrap();
    let above = above.expect("a layer shows over it");
    for ((b, a), m) in below.iter().zip(&above).zip(&whole) {
        let al = a[3] as f32 / 255.0;
        for c in 0..3 { let v = a[c] as f32 * al + b[c] as f32 * (1.0 - al); assert!((v - m[c] as f32).abs() <= 2.0, "{v} vs {}", m[c]); }
    }
    assert!(merge_split(&l, top).unwrap().1.is_none());
    let (pw, ph, p) = preview(&l, 45);
    assert_eq!((pw, ph), (45, 35));
    assert_eq!(p.len(), 45 * 35);
    assert!(gpu_mergeable(&l));
    if let Some(Node::Layer(x)) = l.node_mut(low) { x.blend = BlendMode::Multiply; }
    assert!(!gpu_mergeable(&l));
}

//! The filters fuzz: a random layer (tile size, fill, painted rects, one
//! color areas, line art), a random filter with random values, with or
//! without a random selection; applied tile by tile on the surface, it
//! must give what the whole picture filtered at once gives (the obvious
//! way), mixed by the selection.

use aurastudio_core::{Pixel, Rect, Rgba8, Rng};
use aurastudio_filters::{mix, Filter, Kind, Unit};
use aurastudio_raster::{Image, Mask};

fn rand_rect(r: &mut Rng) -> Rect {
    let (x0, y0) = (r.below(260) as i32 - 60, r.below(200) as i32 - 50);
    Rect::new(x0, y0, x0 + r.below(100) as i32 + 1, y0 + r.below(100) as i32 + 1)
}

fn rand_color(r: &mut Rng) -> Rgba8 {
    match r.below(4) { 0 => [0, 0, 0, 0], 1 => [255, 255, 255, 255], _ => [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8, r.below(256) as u8] }
}

/// A filter of a random kind with random values (sizes kept small).
fn rand_filter(r: &mut Rng) -> Filter {
    let kinds: Vec<Kind> = Kind::all().collect();
    let mut f = Filter::new(kinds[r.below(kinds.len() as u64) as usize]);
    for p in f.kind.params() {
        let max = if p.unit == Unit::Pixels { p.max.min(12.0) } else { p.max };
        f.values[p.index] = r.range(p.min, max);
    }
    // The values without a slider (Levels, Hue / Saturation, switches).
    match f.kind {
        Kind::Levels => for ch in 0..4 {
            let b = aurastudio_filters::idx::levels(ch);
            f.values[b] = r.range(0.0, 100.0);
            f.values[b + 1] = r.range(0.3, 3.0);
            f.values[b + 2] = r.range(150.0, 255.0);
        },
        Kind::HueSaturation => { f.values[0] = r.range(-180.0, 180.0); f.values[1] = r.range(-100.0, 100.0); f.values[4 + 7 * r.below(6) as usize] = r.range(-90.0, 90.0); }
        Kind::BlackWhite => f.values[aurastudio_filters::idx::TINT] = r.below(2) as f32,
        Kind::Thicken | Kind::Thin => f.values[aurastudio_filters::idx::ROUND] = r.below(2) as f32,
        _ => {}
    }
    f.color = [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8];
    f
}

fn seen(p: Rgba8) -> Rgba8 { if p[3] == 0 { [0; 4] } else { p } }

/// One random case from `seed`; panics on the first wrong pixel.
pub fn run(seed: u64) {
    let mut r = Rng::new(seed);
    let tile = [16, 32, 64][r.below(3) as usize];
    let mut s = Image::new(r.below(200) as u32 + 1, r.below(150) as u32 + 1, tile, rand_color(&mut r));
    for _ in 0..r.below(8) {
        let rect = rand_rect(&mut r);
        match r.below(3) {
            0 => s.fill_rect(rect, rand_color(&mut r)),
            1 => { let px: Vec<Rgba8> = (0..rect.area()).map(|_| rand_color(&mut r)).collect(); s.write(rect, &px); }
            _ => { let cov: Vec<u8> = (0..rect.area()).map(|_| r.below(256) as u8).collect(); s.fill_mask(rect, &cov, rand_color(&mut r)); }
        }
    }
    let sel = match r.below(3) {
        0 => None,
        k => {
            let mut m = Mask::new(s.width(), s.height(), tile, if k == 1 { 0 } else { 255 });
            for _ in 0..r.below(4) + 1 { m.fill_rect(rand_rect(&mut r), r.below(256) as u8); }
            Some(m)
        }
    };
    let f = rand_filter(&mut r);
    let before = s.clone();
    f.apply_to(&mut s, sel.as_ref());
    // Wide enough that the whole picture's edges are fill.
    let mut b = before.stored_bounds().unwrap_or(Rect::new(0, 0, 1, 1));
    if let Some(m) = &sel { if let Some(mb) = m.stored_bounds() { b = b.union(&mb); } }
    let view = b.union(&before.rect()).grown(2 * f.reach() as i32 + 8);
    let flat = f.apply(&before.read(view), view.w() as usize, view.h() as usize);
    // (Its outer `reach` read past the picture: no shadow cast from there.)
    let inner = view.grown(-(f.reach() as i32));
    for y in inner.y0..inner.y1 {
        for x in inner.x0..inner.x1 {
            let want = flat[((y - view.y0) * view.w() as i32 + x - view.x0) as usize];
            let want = match &sel { None => want, Some(m) => mix(before.pixel(x, y), want, m.pixel(x, y)) };
            let got = s.pixel(x, y);
            assert_eq!(seen(got), seen(want), "seed {seed}: {:?} {:?} at {x},{y} (tile {tile}, selection {})", f.kind, f.values, sel.is_some());
            assert!(got.visible() || !want.visible());
        }
    }
    // Far away: the fill's own result (mixed by the selection's fill).
    let far = f.prepare().on_uniform(before.fill());
    let far = match &sel { None => far, Some(m) => mix(before.fill(), far, m.fill()) };
    assert_eq!(seen(s.pixel(view.x1 + 5000, view.y0 - 3000)), seen(far), "seed {seed}: {:?} far fill", f.kind);
}

//! The raster fuzz: random operations on a surface and on a plain buffer
//! that does the same thing the obvious way; after every operation both
//! must hold the same pixels, and undo / redo must give each state back.

use aurastudio_core::blend::paint_pixel_u8;
use aurastudio_core::{BlendMode, PaintSurface, Pixel, Rect, Rgba8, Rng};
use aurastudio_raster::{Image, Mask, Patch};

/// The obvious model: every pixel in a `Vec`.
#[derive(Clone, PartialEq, Debug)]
struct Flat { w: u32, h: u32, px: Vec<Rgba8> }

impl Flat {
    fn rect(&self) -> Rect { Rect::of_size(self.w, self.h) }
    fn at(&mut self, x: i32, y: i32) -> &mut Rgba8 { &mut self.px[(y as u32 * self.w + x as u32) as usize] }
}

fn rand_rect(r: &mut Rng, w: u32, h: u32) -> Rect {
    let x0 = r.below(w as u64 + 40) as i32 - 20;
    let y0 = r.below(h as u64 + 40) as i32 - 20;
    Rect::new(x0, y0, x0 + r.below(90) as i32 + 1, y0 + r.below(90) as i32 + 1)
}

fn rand_color(r: &mut Rng) -> Rgba8 {
    match r.below(4) { 0 => [0, 0, 0, 255], 1 => [255, 255, 255, 0], _ => [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8, r.below(256) as u8] }
}

/// Run `ops` random operations from `seed`; panics on the first mismatch.
pub fn run(seed: u64, ops: u32) {
    let mut r = Rng::new(seed);
    let (w, h) = (r.below(300) as u32 + 1, r.below(200) as u32 + 1);
    let tile = [8, 16, 32, 64, 128][r.below(5) as usize];
    let fill = rand_color(&mut r);
    let mut s = Image::new(w, h, tile, fill);
    let mut f = Flat { w, h, px: vec![fill; (w * h) as usize] };
    let mut history: Vec<(Patch<Rgba8>, Flat)> = Vec::new();
    let mut limit: Option<Mask> = None;
    for op in 0..ops {
        let what = r.below(12);
        if std::env::var("FUZZ_TRACE").is_ok() { eprintln!("op {op}: kind {what}, tile {}, stored {:?}", s.tile_size(), s.stored()); }
        // (Edits other than strokes are not in this history: undoing an
        // older stroke past them is not a thing the app does.)
        if what <= 2 { history.clear(); }
        match what {
            0 => {
                let rect = rand_rect(&mut r, w, h);
                let data: Vec<Rgba8> = (0..rect.area()).map(|_| rand_color(&mut r)).collect();
                s.write(rect, &data);
                let inside = rect.intersect(&f.rect());
                for y in inside.y0..inside.y1 { for x in inside.x0..inside.x1 { *f.at(x, y) = data[((y - rect.y0) as u32 * rect.w() + (x - rect.x0) as u32) as usize]; } }
            }
            1 => {
                let (rect, c) = (rand_rect(&mut r, w, h), rand_color(&mut r));
                s.fill_rect(rect, c);
                let inside = rect.intersect(&f.rect());
                for y in inside.y0..inside.y1 { for x in inside.x0..inside.x1 { *f.at(x, y) = c; } }
            }
            2 => {
                let (rect, c) = (rand_rect(&mut r, w, h), rand_color(&mut r));
                let cov: Vec<u8> = (0..rect.area()).map(|_| [0u8, 255, r.below(256) as u8][r.below(3) as usize]).collect();
                s.fill_mask(rect, &cov, c);
                let inside = rect.intersect(&f.rect());
                for y in inside.y0..inside.y1 { for x in inside.x0..inside.x1 {
                    let k = cov[((y - rect.y0) as u32 * rect.w() + (x - rect.x0) as u32) as usize];
                    if k > 0 { let p = *f.at(x, y); *f.at(x, y) = p.over(c, k); }
                } }
            }
            3..=5 => {
                // A stroke of a few dabs, undoable.
                let before = f.clone();
                let color = rand_color(&mut r);
                let dabs: Vec<(Rect, u8)> = (0..r.below(6) + 1).map(|_| (rand_rect(&mut r, w, h), r.below(256) as u8)).collect();
                let lim = limit.as_ref();
                let patch = {
                    let mut st = s.begin_stroke(lim);
                    let mut mask = vec![0u8; (w * h) as usize];
                    for (rect, cov) in &dabs {
                        st.prepare(*rect);
                        let mut buf = Vec::new();
                        st.spans_mut(*rect, &mut buf);
                        for mut sp in buf {
                            for i in 0..sp.len() {
                                let m = sp.cover(i, *cov);
                                sp.pixels[i] = if m == 0 { sp.base[i] } else { paint_pixel_u8(sp.base[i], [color[0], color[1], color[2]], m, BlendMode::Normal) };
                            }
                        }
                        // The model: same rule on the flat buffer.
                        let inside = rect.intersect(&f.rect());
                        for y in inside.y0..inside.y1 { for x in inside.x0..inside.x1 {
                            let i = (y as u32 * w + x as u32) as usize;
                            let cap = lim.map_or(255, |l| l.pixel(x, y));
                            mask[i] = mask[i].max((*cov).min(cap));
                            if mask[i] > 0 { f.px[i] = paint_pixel_u8(before.px[i], [color[0], color[1], color[2]], mask[i], BlendMode::Normal); }
                        } }
                    }
                    st.end()
                };
                history.push((patch, before));
            }
            6 => {
                // Undo the last stroke, check, redo it, check, maybe drop it.
                if let Some((mut p, before)) = history.pop() {
                    let after = f.clone();
                    assert!(p.swap(&mut s), "seed {seed} op {op}: undo refused");
                    let got = s.read(s.rect());
                    if let Some(i) = got.iter().zip(&before.px).position(|(a, b)| a != b) { panic!("seed {seed} op {op}: undo differs at ({}, {}): {:?} vs {:?} (w {w}, h {h})", i as u32 % w, i as u32 / w, got[i], before.px[i]); }
                    assert!(p.swap(&mut s), "seed {seed} op {op}: redo refused");
                    assert_eq!(s.read(s.rect()), after.px, "seed {seed} op {op}: redo");
                    if r.chance(0.5) { p.pack(); assert!(p.swap(&mut s)); f = before; } else { history.push((p, before)); }
                }
            }
            7 => {
                let lim = Mask::new(w, h, [8, 16, 32][r.below(3) as usize], 0);
                let mut lim = lim;
                for _ in 0..3 { lim.fill_rect(rand_rect(&mut r, w, h), [0u8, 255, 128][r.below(3) as usize]); }
                limit = if r.chance(0.7) { Some(lim) } else { None };
                history.clear();
            }
            8 => { s.freeze(rand_rect(&mut r, w, h)); if r.chance(0.3) { s.thaw(); } }
            9 => {
                let back = Image::load(&s.save(), 10_000);
                assert!(back.as_ref().is_some_and(|b| *b == s), "seed {seed} op {op}: save / load");
                if let Some(b) = back { s = b; history.clear(); }
            }
            10 => {
                let t = [8, 16, 32, 64, 128][r.below(5) as usize];
                s = s.retiled(t);
                history.clear();
            }
            _ => {
                let (dx, dy) = (r.below(60) as i32 - 30, r.below(60) as i32 - 30);
                // (The model holds the frame only: what the surface keeps
                // past it is cut first, so a move brings nothing back in.)
                s.clear_outside(s.rect());
                let mut m = Image::new(w, h, s.tile_size(), fill);
                m.copy_from(&s, s.rect(), dx, dy);
                m.clear_outside(m.rect());
                let mut g = Flat { w, h, px: vec![fill; (w * h) as usize] };
                for y in 0..h as i32 { for x in 0..w as i32 {
                    let (sx, sy) = (x - dx, y - dy);
                    if sx >= 0 && sy >= 0 && (sx as u32) < w && (sy as u32) < h { *g.at(x, y) = f.px[(sy as u32 * w + sx as u32) as usize]; }
                } }
                s = m;
                f = g;
                history.clear();
            }
        }
        assert_eq!(s.read(s.rect()), f.px, "seed {seed} op {op} (kind {what}, tile {})", s.tile_size());
    }
}

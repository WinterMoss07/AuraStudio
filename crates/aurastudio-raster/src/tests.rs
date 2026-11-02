use std::sync::Arc;

use aurastudio_core::blend::paint_pixel_u8;
use aurastudio_core::{BlendMode, FlatSurface, PaintSurface, Rect, Rgba8, Span};

use crate::surface::Slot;
use crate::{legacy, Image, Mask, Patch, Surface};

const CLEAR: Rgba8 = [255, 255, 255, 0];

fn pattern(r: Rect) -> Vec<Rgba8> {
    (0..r.area()).map(|i| [(i * 7 % 251) as u8, (i % 13) as u8, 3, 255]).collect()
}

#[test]
fn empty_surfaces_hold_nothing_not_even_a_table() {
    let mut s = Image::new(6071, 8598, 16, CLEAR);
    assert_eq!(s.stored(), (0, 0));
    assert_eq!(s.slots.capacity(), 0);
    assert_eq!(s.pixel(100, 100), CLEAR);
    assert_eq!(s.pixel(-5, 3), CLEAR);
    s.write(Rect::new(70, 70, 71, 71), &[[1, 2, 3, 255]]);
    assert_eq!(s.stored(), (0, 1));
    s.clear();
    assert_eq!((s.stored(), s.slots.capacity()), ((0, 0), 0));
}

#[test]
fn tile_sizes_are_powers_of_two_in_range() {
    assert_eq!(Image::new(10, 10, 3, CLEAR).tile_size(), 8);
    assert_eq!(Image::new(10, 10, 100, CLEAR).tile_size(), 128);
    assert_eq!(Image::new(10, 10, 4096, CLEAR).tile_size(), 256);
}

#[test]
fn rects_round_trip_and_the_fill_is_freed() {
    for tile in [16, 32, 64, 128] {
        let mut s = Image::new(200, 150, tile, [0; 4]);
        let r = Rect::new(60, 60, 140, 70);
        let data = pattern(r);
        s.write(r, &data);
        assert_eq!(s.read(r), data, "tile {tile}");
        assert_eq!(s.pixel(59, 60), [0; 4]);
        s.write(r, &vec![[0; 4]; data.len()]);
        assert_eq!(s.stored(), (0, 0), "tile {tile}");
        // Reading past the edge gives the fill.
        assert_eq!(s.read(Rect::new(195, 145, 205, 155)).len(), 100);
    }
}

#[test]
fn one_color_tiles_store_no_pixels() {
    let mut s = Image::new(100, 100, 16, CLEAR);
    s.fill_rect(Rect::new(0, 0, 40, 40), [9, 9, 9, 255]);
    // 2 × 2 tiles covered, the edge ones (32..40) have pixels.
    assert_eq!(s.stored(), (4, 5));
    assert_eq!(s.pixel(39, 39), [9, 9, 9, 255]);
    assert_eq!(s.pixel(40, 39), CLEAR);
    // Writing one color over a whole tile makes it uniform.
    s.write(Rect::new(48, 48, 64, 64), &[[1, 1, 1, 255]; 256]);
    assert!(matches!(s.slot(3, 3), Slot::Uniform(_)));
    let mut rows = Vec::new();
    s.row(50, 40, 70, |x, run| rows.push((x, run.len(), matches!(run, crate::Run::Uniform(..)))));
    assert_eq!(rows, vec![(40, 8, true), (48, 16, true), (64, 6, true)]);
}

#[test]
fn copies_share_tiles_until_one_writes() {
    let mut a = Image::new(64, 64, 32, CLEAR);
    a.write(Rect::new(0, 0, 32, 32), &pattern(Rect::new(0, 0, 32, 32)));
    let b = a.clone();
    let (Slot::Full(pa), Slot::Full(pb)) = (a.slot(0, 0), b.slot(0, 0)) else { panic!() };
    assert!(Arc::ptr_eq(pa, pb));
    a.write(Rect::new(1, 1, 2, 2), &[[0, 0, 0, 255]]);
    let (Slot::Full(pa), Slot::Full(pb)) = (a.slot(0, 0), b.slot(0, 0)) else { panic!() };
    assert!(!Arc::ptr_eq(pa, pb));
    assert_ne!(a.pixel(1, 1), b.pixel(1, 1));
}

#[test]
fn bounds() {
    let mut s = Image::new(300, 300, 32, CLEAR);
    assert_eq!((s.stored_bounds(), s.visible_bounds(), s.pixel_bounds()), (None, None, None));
    s.write(Rect::new(40, 50, 41, 52), &[[1, 2, 3, 255], [1, 2, 3, 255]]);
    assert_eq!(s.pixel_bounds(), Some(Rect::new(40, 50, 41, 52)));
    assert_eq!(s.visible_bounds(), Some(Rect::new(32, 32, 64, 64)));
    // Erased to transparent but another color: stored, not visible.
    s.write(Rect::new(40, 50, 41, 52), &[[0, 0, 0, 0], [0, 0, 0, 0]]);
    assert!(s.stored_bounds().is_some() && s.visible_bounds().is_none());
    assert!(!s.any_visible_in(Rect::new(0, 0, 300, 300)));
    assert!(!s.is_fill_in(Rect::new(0, 0, 300, 300)) && s.is_fill_in(Rect::new(100, 100, 200, 200)));
    let paper = Image::new(10, 10, 16, [255; 4]);
    assert_eq!(paper.pixel_bounds(), Some(Rect::new(0, 0, 10, 10)));
}

#[test]
fn chunks_cover_the_rect_once_on_tile_edges() {
    let s = Image::new(1000, 700, 16, CLEAR);
    let r = Rect::new(-10, 5, 333, 700);
    let c = s.chunks(r, 128);
    let area: u64 = c.iter().map(|c| c.area()).sum();
    assert_eq!(area, r.intersect(&s.rect()).area());
    for (i, a) in c.iter().enumerate() {
        assert!(c[i + 1..].iter().all(|b| !a.overlaps(b)));
        assert!(a.w() <= 128 && a.h() <= 128);
    }
    assert!(s.chunks(Rect::new(2000, 0, 2100, 10), 64).is_empty());
}

#[test]
fn resize_moves_the_content_aligned_or_not() {
    for (dx, dy) in [(32, 64), (7, 5), (0, 0)] {
        let mut s = Image::new(100, 100, 32, CLEAR);
        s.write(Rect::new(10, 10, 11, 11), &[[5, 5, 5, 255]]);
        s.write(Rect::new(99, 99, 100, 100), &[[6, 6, 6, 255]]);
        let r = s.resized(300, 200, dx, dy);
        assert_eq!(r.pixel(10 + dx, 10 + dy), [5, 5, 5, 255], "{dx} {dy}");
        assert_eq!(r.pixel(99 + dx, 99 + dy), [6, 6, 6, 255]);
        // Past the old right edge: the fill, not leftovers.
        assert_eq!(r.pixel(100 + dx, 99 + dy), CLEAR);
        assert_eq!(r.resized(100, 100, -dx, -dy), s);
    }
}

#[test]
fn retiled_surfaces_hold_the_same_pixels() {
    let mut s = Image::new(333, 257, 64, CLEAR);
    s.write(Rect::new(20, 30, 300, 40), &pattern(Rect::new(20, 30, 300, 40)));
    s.fill_rect(Rect::new(0, 128, 333, 257), [1, 1, 1, 255]);
    for t in [16, 32, 128] {
        let r = s.retiled(t);
        assert_eq!(r.tile_size(), t);
        assert!(r == s, "tile {t}");
    }
}

#[test]
fn set_fill_keeps_stored_pixels() {
    let mut s = Image::new(64, 64, 16, CLEAR);
    s.fill_rect(Rect::new(0, 0, 16, 16), [255; 4]);
    s.set_fill([255; 4]);
    assert_eq!(s.stored(), (0, 0), "the white tile is the fill now");
    assert_eq!(s.pixel(40, 40), [255; 4]);
    s.set_fill(CLEAR);
    assert_eq!(s.pixel(5, 5), CLEAR);
}

/// A tiny engine following the rule of `PaintSurface`: coverage capped by
/// the limit, max with the mask, pixel made from the base.
fn dab(surf: &mut dyn PaintSurface, r: Rect, color: [u8; 3], cov: u8) {
    surf.prepare(r);
    let Some((x0, y0, x1, y1)) = r.clipped_u32(surf.width(), surf.height()) else { return };
    for y in y0..y1 {
        let mut x = x0;
        while x < x1 {
            let s = surf.span_mut(x, y, (x1 - x) as usize);
            let n = s.len();
            assert!(n > 0);
            paint(s, color, cov);
            x += n as u32;
        }
    }
}

fn paint(s: Span<'_>, color: [u8; 3], cov: u8) {
    for i in 0..s.len() {
        let c = s.limit.map_or(cov, |l| cov.min(l[i]));
        s.mask[i] = s.mask[i].max(c);
        if s.mask[i] == 0 { s.pixels[i * 4..i * 4 + 4].copy_from_slice(&s.base[i * 4..i * 4 + 4]); continue; }
        let b = [s.base[i * 4], s.base[i * 4 + 1], s.base[i * 4 + 2], s.base[i * 4 + 3]];
        s.pixels[i * 4..i * 4 + 4].copy_from_slice(&paint_pixel_u8(b, color, s.mask[i], BlendMode::Normal));
    }
}

fn flat_of(s: &Image) -> FlatSurface {
    let mut f = FlatSurface::new(s.width(), s.height(), CLEAR);
    f.pixels = s.read(s.rect()).as_flattened().to_vec();
    f.next_stroke();
    f
}

#[test]
fn a_stroke_paints_like_a_flat_buffer_and_undoes_by_tile() {
    for tile in [16, 32, 128] {
        let mut s = Image::new(200, 120, tile, CLEAR);
        s.write(Rect::new(30, 30, 90, 60), &pattern(Rect::new(30, 30, 90, 60)));
        let before = s.clone();
        let mut f = flat_of(&s);
        let dabs = [(Rect::new(10, 10, 60, 50), 120u8), (Rect::new(40, 20, 120, 70), 200), (Rect::new(-5, 100, 30, 130), 255)];
        let patch = {
            let mut st = s.begin_stroke(None);
            for (r, c) in dabs { dab(&mut st, r, [200, 40, 10], c); }
            st.end()
        };
        for (r, c) in dabs { dab(&mut f, r, [200, 40, 10], c); }
        assert_eq!(s.read(s.rect()).as_flattened(), &f.pixels[..], "tile {tile}");
        // The patch holds only touched tiles, and swaps back and forth.
        let mut p = patch;
        assert!(!p.is_empty());
        let after = s.clone();
        assert!(p.swap(&mut s));
        assert!(s == before, "undo, tile {tile}");
        assert!(p.swap(&mut s));
        assert!(s == after, "redo, tile {tile}");
    }
}

#[test]
fn spans_at_once_paint_the_same_as_one_by_one() {
    let mut a = Image::new(150, 90, 16, CLEAR);
    a.fill_rect(Rect::new(0, 0, 150, 40), [10, 200, 30, 255]);
    let mut b = a.clone();
    let r = Rect::new(5, 7, 140, 80);
    {
        let mut st = a.begin_stroke(None);
        dab(&mut st, r, [1, 2, 250], 150);
        st.end();
    }
    {
        let mut st = b.begin_stroke(None);
        let mut buf = Vec::new();
        assert!(st.spans_mut(r, &mut buf));
        let area: usize = buf.iter().map(|s| s.len()).sum();
        assert_eq!(area as u64, r.area());
        for s in buf { paint(s, [1, 2, 250], 150); }
        st.end();
    }
    assert!(a == b);
}

#[test]
fn the_limit_clips_the_stroke_at_the_source() {
    // Limit: left half fully selected, right half nothing, a soft band.
    let mut limit = Mask::new(120, 60, 32, 0);
    limit.fill_rect(Rect::new(0, 0, 60, 60), 255);
    limit.fill_rect(Rect::new(60, 0, 70, 60), 100);
    let mut s = Image::new(120, 60, 16, CLEAR);
    let r = Rect::new(0, 0, 120, 60);
    for at_once in [false, true] {
        s.clear();
        {
            let mut st = s.begin_stroke(Some(&limit));
            if at_once {
                let mut buf = Vec::new();
                st.spans_mut(r, &mut buf);
                for sp in buf { paint(sp, [0, 0, 0], 255); }
            } else {
                dab(&mut st, r, [0, 0, 0], 255);
            }
            st.end();
        }
        assert_eq!(s.pixel(10, 10)[3], 255, "selected");
        assert_eq!(s.pixel(65, 10)[3], 100, "soft edge");
        assert_eq!(s.pixel(100, 10)[3], 0, "outside");
        assert!(s.is_fill_in(Rect::new(80, 0, 120, 60)), "nothing stored outside ({at_once})");
    }
}

#[test]
fn a_stroke_back_to_one_color_stores_no_pixels() {
    let mut s = Image::new(64, 64, 16, CLEAR);
    let p = {
        let mut st = s.begin_stroke(None);
        dab(&mut st, Rect::new(0, 0, 64, 64), [5, 6, 7], 255);
        st.end()
    };
    assert_eq!(s.stored(), (16, 0));
    assert_eq!(p.len(), 16);
}

#[test]
fn patches_pack_and_save() {
    let mut s = Image::new(100, 100, 32, CLEAR);
    let snap = s.snapshot();
    s.write(Rect::new(0, 0, 50, 50), &pattern(Rect::new(0, 0, 50, 50)));
    s.fill_rect(Rect::new(64, 64, 96, 96), [1, 2, 3, 255]);
    let after = s.clone();
    let mut p = s.diff(&snap);
    assert_eq!(p.len(), 5);
    assert_eq!(p.rects(100, 100), vec![Rect::new(0, 0, 64, 32), Rect::new(0, 32, 64, 64), Rect::new(64, 64, 96, 96)]);
    p.pack();
    let bytes = p.to_bytes();
    let mut back = Patch::<Rgba8>::from_bytes(&bytes).expect("patch");
    assert!(back.swap(&mut s));
    assert_eq!(s.stored(), (0, 0));
    assert!(back.swap(&mut s));
    assert!(s == after);
    assert!(Patch::<Rgba8>::from_bytes(&bytes[..bytes.len() - 1]).is_none());
    assert!(Patch::<u8>::from_bytes(&bytes).is_none(), "another pixel kind");
    // Another tile size: refused, nothing changed.
    let mut other = Image::new(100, 100, 16, CLEAR);
    assert!(!back.swap(&mut other));
    assert_eq!(other.stored(), (0, 0));
}

#[test]
fn surfaces_save_and_load_and_damage_is_refused() {
    let mut s = Image::new(333, 200, 32, [255, 255, 255, 0]);
    s.write(Rect::new(10, 20, 150, 90), &pattern(Rect::new(10, 20, 150, 90)));
    s.fill_rect(Rect::new(160, 0, 333, 200), [3, 3, 3, 255]);
    let b = s.save();
    assert!(b.len() as u64 <= Image::saved_max(333, 200, 32));
    let back = Image::load(&b, 10_000).expect("load");
    assert!(back == s && back.tile_size() == 32 && back.stored() == s.stored());
    assert!(Image::load(&b[..b.len() - 2], 10_000).is_none());
    assert!(Image::load(&b, 100).is_none(), "too big");
    assert!(Mask::load(&b, 10_000).is_none(), "another pixel kind");
    let mut m = Mask::new(50, 50, 16, 0);
    m.fill_rect(Rect::new(3, 3, 40, 40), 200);
    assert!(Mask::load(&m.save(), 100).is_some_and(|x| x == m));
}

/// The 0.19 layout, written as 0.19 wrote it.
fn old_pack(px: &[Rgba8], w: u32, h: u32, fill: Rgba8) -> Vec<u8> {
    let (cols, rows) = (w.div_ceil(64), h.div_ceil(64));
    let mut tiles = Vec::new();
    for ty in 0..rows {
        for tx in 0..cols {
            let mut t = vec![fill; 64 * 64];
            let mut any = false;
            for y in 0..64 {
                for x in 0..64 {
                    let (gx, gy) = (tx * 64 + x, ty * 64 + y);
                    if gx < w && gy < h { t[(y * 64 + x) as usize] = px[(gy * w + gx) as usize]; any |= t[(y * 64 + x) as usize] != fill; }
                }
            }
            if any { tiles.push((tx, ty, t)); }
        }
    }
    let mut out = (tiles.len() as u32).to_le_bytes().to_vec();
    for (tx, ty, t) in tiles {
        let p = lz4_flex::compress_prepend_size(t.as_flattened());
        out.extend_from_slice(&tx.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(&p);
    }
    out
}

#[test]
fn old_layers_and_undo_steps_are_read() {
    let (w, h) = (150, 100);
    let mut s = Image::new(w, h, 64, CLEAR);
    s.write(Rect::new(5, 5, 140, 70), &pattern(Rect::new(5, 5, 140, 70)));
    let blob = old_pack(&s.read(s.rect()), w, h, CLEAR);
    for tile in [16, 64] {
        let back = legacy::load_layer(&blob, w, h, CLEAR, tile).expect("old layer");
        assert!(back == s && back.tile_size() == tile);
    }
    assert!(legacy::load_layer(&blob[..blob.len() - 1], w, h, CLEAR, 16).is_none());
    // An old undo step: one tile of pixels, one of fill.
    let px = vec![[9u8, 8, 7, 255]; 64 * 64];
    let data = lz4_flex::compress(px.as_flattened());
    let mut b = vec![0, 0, 0, 0, b'T', b'I', b'L', b'E'];
    for (tx, ty, d) in [(1u32, 0u32, data), (0, 1, Vec::new())] {
        b.extend_from_slice(&tx.to_le_bytes());
        b.extend_from_slice(&ty.to_le_bytes());
        b.extend_from_slice(&(d.len() as u32).to_le_bytes());
        b.extend_from_slice(&d);
    }
    assert!(legacy::is_old_patch(&b));
    let steps = legacy::read_old_patch(&b).expect("old patch");
    assert_eq!(steps[0].0, Rect::new(64, 0, 128, 64));
    assert_eq!(steps[0].1.as_ref().map(|p| p[0]), Some([9, 8, 7, 255]));
    assert!(steps[1].1.is_none());
}

#[test]
fn heap_bytes_follow_the_pixel_tiles() {
    use aurastudio_core::HeapSize;
    let mut s = Surface::<Rgba8>::new(256, 256, 16, CLEAR);
    assert_eq!(s.heap_bytes(), 0);
    s.write(Rect::new(0, 0, 1, 1), &[[1, 1, 1, 255]]);
    s.fill_rect(Rect::new(16, 0, 32, 16), [2, 2, 2, 255]);
    let table = s.slots.capacity() * std::mem::size_of::<Slot<Rgba8>>();
    assert_eq!(s.heap_bytes(), table + 16 * 16 * 4, "one pixel tile, one uniform");
}

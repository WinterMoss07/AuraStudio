use std::sync::Arc;

use aurastudio_core::blend::paint_pixel_u8;
use aurastudio_core::{BlendMode, FlatSurface, PaintSurface, Pixel, Rect, Rgba16, Rgba8, Span};

use crate::slot::Slot;
use crate::{legacy, Combine, DirtyTiles, Image, Mask, Patch, SaveCache, Stored, Surface};

const CLEAR: Rgba8 = [255, 255, 255, 0];

fn pattern(r: Rect) -> Vec<Rgba8> {
    (0..r.area()).map(|i| [(i * 7 % 251) as u8, (i % 13) as u8, 3, 255]).collect()
}

#[test]
fn empty_surfaces_hold_nothing_not_even_a_table() {
    let mut s = Image::new(6071, 8598, 16, CLEAR);
    assert_eq!(s.stored(), Stored::default());
    assert_eq!(s.slots.capacity(), 0);
    assert_eq!(s.pixel(100, 100), CLEAR);
    assert_eq!(s.pixel(-5, 3), CLEAR);
    s.write(Rect::new(70, 70, 71, 71), &[[1, 2, 3, 255]]);
    assert_eq!(s.stored().total(), 1);
    s.clear();
    assert_eq!((s.stored(), s.slots.capacity()), (Stored::default(), 0));
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
        assert_eq!(s.stored(), Stored::default(), "tile {tile}");
        // Reading past the edge gives the fill.
        assert_eq!(s.read(Rect::new(195, 145, 205, 155)).len(), 100);
    }
}

#[test]
fn one_color_tiles_store_no_pixels() {
    let mut s = Image::new(100, 100, 16, CLEAR);
    s.fill_rect(Rect::new(0, 0, 40, 40), [9, 9, 9, 255]);
    // 2 × 2 tiles covered; the edge ones (32..40) are one color with
    // coverage (alpha only), not pixels.
    assert_eq!(s.stored(), Stored { uniform: 4, alpha: 5, ..Default::default() });
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
    let (Slot::Full(pa), Slot::Full(pb)) = (a.slot(0, 0), b.slot(0, 0)) else { panic!("pixel tiles") };
    assert!(Arc::ptr_eq(pa, pb));
    a.write(Rect::new(1, 1, 2, 2), &[[0, 0, 0, 255]]);
    let (Slot::Full(pa), Slot::Full(pb)) = (a.slot(0, 0), b.slot(0, 0)) else { panic!("pixel tiles") };
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
    assert_eq!(s.stored(), Stored::default(), "the white tile is the fill now");
    assert_eq!(s.pixel(40, 40), [255; 4]);
    s.set_fill(CLEAR);
    assert_eq!(s.pixel(5, 5), CLEAR);
}

/// A tiny engine following the rule of `PaintSurface`: coverage capped by
/// the limit, max with the mask, pixel made from the base.
fn dab(surf: &mut dyn PaintSurface<Rgba8>, r: Rect, color: [u8; 3], cov: u8) {
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

fn paint(mut s: Span<'_>, color: [u8; 3], cov: u8) {
    for i in 0..s.len() {
        let m = s.cover(i, cov);
        s.pixels[i] = if m == 0 { s.base[i] } else { paint_pixel_u8(s.base[i], color, m, BlendMode::Normal) };
    }
}

fn flat_of(s: &Image) -> FlatSurface {
    let mut f = FlatSurface::new(s.width(), s.height(), CLEAR);
    f.pixels = s.read(s.rect());
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
        assert_eq!(s.read(s.rect()), f.pixels, "tile {tile}");
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
    assert_eq!(s.stored(), Stored { uniform: 16, ..Default::default() });
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
    assert_eq!(p.bounds(100, 100), Rect::new(0, 0, 96, 96));
    assert_eq!(p.rects(100, 100), vec![Rect::new(0, 0, 64, 32), Rect::new(0, 32, 64, 64), Rect::new(64, 64, 96, 96)]);
    p.pack();
    let bytes = p.to_bytes();
    let mut back = Patch::<Rgba8>::from_bytes(&bytes).expect("patch");
    assert!(back.swap(&mut s));
    assert_eq!(s.stored(), Stored::default());
    assert!(back.swap(&mut s));
    assert!(s == after);
    assert!(Patch::<Rgba8>::from_bytes(&bytes[..bytes.len() - 1]).is_none());
    assert!(Patch::<u8>::from_bytes(&bytes).is_none(), "another pixel kind");
    // Another tile size: refused, nothing changed.
    let mut other = Image::new(100, 100, 16, CLEAR);
    assert!(!back.swap(&mut other));
    assert_eq!(other.stored(), Stored::default());
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
    s.write(Rect::new(40, 0, 42, 1), &[[1, 1, 1, 255], [9, 9, 9, 255]]);
    let table = s.slots.capacity() * std::mem::size_of::<Slot<Rgba8>>();
    assert_eq!(s.heap_bytes(), table + 16 * 16 + 16 * 16 * 4, "one coverage tile, one uniform, one pixel tile");
}

#[test]
fn line_art_is_stored_as_coverage_and_read_as_runs() {
    let mut s = Image::new(64, 64, 16, CLEAR);
    // A soft black line: one color, varying alpha.
    {
        let mut st = s.begin_stroke(None);
        dab(&mut st, Rect::new(5, 5, 30, 8), [0, 0, 0], 120);
        dab(&mut st, Rect::new(5, 6, 30, 7), [0, 0, 0], 255);
        st.end();
    }
    let st = s.stored();
    assert_eq!((st.alpha, st.full), (2, 0), "{st:?}");
    let mut kinds = Vec::new();
    s.row(6, 0, 40, |_, run| kinds.push(matches!(run, crate::Run::Coverage { .. })));
    assert_eq!(kinds, vec![true, true, false]);
    assert_eq!(s.pixel(10, 6), [0, 0, 0, 255]);
    assert_eq!(s.pixel(10, 5)[3], 120);
    assert_eq!(s.pixel(40, 6), CLEAR, "clear pixels keep their value");
    // A second color makes it pixels again; erasing back makes it coverage.
    {
        let mut st = s.begin_stroke(None);
        dab(&mut st, Rect::new(10, 10, 12, 12), [255, 0, 0], 255);
        st.end();
    }
    assert_eq!(s.stored().full, 1);
    // Saved and loaded, undone and redone: the same pixels, the same forms.
    let back = Image::load(&s.save(), 1000).expect("load");
    assert!(back == s && back.stored() == s.stored());
}

#[test]
fn summaries_give_bounds_opacity_and_a_coarse_picture() {
    let mut s = Image::new(100, 50, 16, CLEAR);
    s.fill_rect(Rect::new(0, 0, 32, 16), [0, 0, 255, 255]);
    s.write(Rect::new(70, 20, 71, 21), &[[255, 0, 0, 128]]);
    assert_eq!(s.pixel_bounds(), Some(Rect::new(0, 0, 71, 21)));
    assert!(s.opaque_in(Rect::new(0, 0, 32, 16)) && !s.opaque_in(Rect::new(0, 0, 33, 16)));
    let g = s.summary_grid();
    assert_eq!((g.cols, g.rows, g.cell), (7, 4, 16));
    assert_eq!(g.avg[0], [0, 0, 255, 255]);
    assert_eq!(g.avg[6], [0, 0, 0, 0]);
    assert!(g.avg[7 + 70 / 16][3] > 0, "the half red pixel shows in its cell");
}

#[test]
fn fingerprints_follow_content_not_storage() {
    let mut a = Image::new(64, 64, 16, CLEAR);
    a.write(Rect::new(0, 0, 20, 20), &pattern(Rect::new(0, 0, 20, 20)));
    let b = Image::load(&a.save(), 100).expect("load");
    let r = Rect::new(0, 0, 32, 32);
    assert_eq!(a.fingerprint(r), b.fingerprint(r));
    let mut c = a.clone();
    c.write(Rect::new(3, 3, 4, 4), &[[0, 0, 0, 255]]);
    assert_ne!(a.fingerprint(r), c.fingerprint(r));
    assert_eq!(a.fingerprint(Rect::new(32, 32, 64, 64)), c.fingerprint(Rect::new(32, 32, 64, 64)));
}

#[test]
fn saving_again_reuses_the_unchanged_tiles() {
    let mut s = Image::new(256, 256, 32, CLEAR);
    s.write(Rect::new(0, 0, 256, 256), &pattern(Rect::new(0, 0, 256, 256)));
    let mut cache = SaveCache::default();
    let (first, st1) = s.save_with(&mut cache);
    assert_eq!((st1.tiles, st1.reused), (64, 0));
    s.write(Rect::new(40, 40, 41, 41), &[[0, 0, 0, 255]]);
    let (second, st2) = s.save_with(&mut cache);
    assert_eq!((st2.encoded, st2.reused), (1, 63));
    assert!(Image::load(&second, 1000).is_some_and(|b| b == s));
    assert_ne!(first, second);
}

#[test]
fn cold_tiles_read_the_same_and_warm_up_when_painted() {
    let mut s = Image::new(128, 128, 32, CLEAR);
    // Painted-like content: runs of a few colors (compresses).
    let px: Vec<Rgba8> = (0..128 * 128).map(|i: u32| [((i / 8) % 5) as u8 * 40, (i / 512) as u8, 7, 255]).collect();
    s.write(Rect::new(0, 0, 128, 128), &px);
    let before = s.clone();
    s.freeze(Rect::new(0, 0, 32, 32));
    assert_eq!(s.stored().cold, 15);
    assert!(s == before);
    use aurastudio_core::HeapSize;
    assert!(s.heap_bytes() < before.heap_bytes());
    s.write(Rect::new(100, 100, 101, 101), &[[1, 1, 1, 255]]);
    assert_eq!(s.stored().cold, 14);
    s.thaw();
    assert_eq!(s.stored().cold, 0);
}

#[test]
fn dirty_tiles_are_exact() {
    let mut d = DirtyTiles::new(1000, 1000, 16);
    for i in 0..50 { d.insert(Rect::from_xywh(i * 20, i * 20, 4, 4)); }
    assert_eq!(d.len(), 50);
    assert!(d.touches(Rect::new(20, 20, 21, 21)) && !d.touches(Rect::new(500, 0, 501, 1)));
    let area: u64 = d.rects(1000, 1000).iter().map(|r| r.area()).sum();
    assert_eq!(area, 50 * 16 * 16, "a diagonal stays a band of tiles");
    let mut e = DirtyTiles::new(1000, 1000, 16);
    e.insert(Rect::new(0, 0, 32, 16));
    d.union(&e);
    assert_eq!(d.len(), 51);
}

#[test]
fn the_fill_tool_blends_coverage_and_fully_covered_tiles_are_one_color() {
    let mut s = Image::new(64, 64, 16, CLEAR);
    let r = Rect::new(0, 0, 40, 16);
    let mut cov = vec![255u8; r.area() as usize];
    cov[39] = 128;
    s.fill_mask(r, &cov, [0, 200, 0, 255]);
    assert!(matches!(s.slot(0, 0), Slot::Uniform(_)));
    assert_eq!(s.pixel(39, 0)[3], 128);
    assert_eq!(s.pixel(41, 0), CLEAR);
}

#[test]
fn masks_combine_and_invert() {
    let mut a = Mask::new(64, 64, 16, 0);
    a.fill_rect(Rect::new(0, 0, 32, 64), 255);
    let mut b = Mask::new(64, 64, 16, 0);
    b.fill_rect(Rect::new(16, 0, 48, 64), 200);
    let mut add = a.clone();
    add.combine(&b, Combine::Max);
    assert_eq!((add.pixel(5, 5), add.pixel(40, 5), add.pixel(60, 5)), (255, 200, 0));
    let mut sub = a.clone();
    sub.combine(&b, Combine::Subtract);
    assert_eq!((sub.pixel(5, 5), sub.pixel(20, 5)), (255, 55));
    let mut inter = a.clone();
    inter.combine(&b, Combine::Min);
    assert_eq!((inter.pixel(5, 5), inter.pixel(20, 5)), (0, 200));
    // Another tile size combines too.
    let mut c = Mask::new(64, 64, 32, 0);
    c.combine(&b, Combine::Replace);
    assert!(c == b);
    let mut inv = a.clone();
    inv.invert();
    assert_eq!((inv.pixel(5, 5), inv.pixel(40, 5)), (0, 255));
    inv.invert();
    assert!(inv == a);
}

#[test]
fn copies_between_surfaces_share_aligned_tiles() {
    let mut src = Image::new(128, 128, 32, CLEAR);
    src.write(Rect::new(0, 0, 64, 64), &pattern(Rect::new(0, 0, 64, 64)));
    let mut dst = Image::new(256, 256, 32, CLEAR);
    dst.copy_from(&src, Rect::new(0, 0, 64, 64), 64, 32);
    assert_eq!(dst.read(Rect::new(64, 32, 128, 96)), src.read(Rect::new(0, 0, 64, 64)));
    let (Slot::Full(a), Slot::Full(b)) = (src.slot(0, 0), dst.slot(2, 1)) else { panic!("pixel tiles") };
    assert!(Arc::ptr_eq(a, b));
    let mut odd = Image::new(256, 256, 16, CLEAR);
    odd.copy_from(&src, Rect::new(3, 3, 60, 60), 7, -1);
    assert_eq!(odd.pixel(10, 2), src.pixel(3, 3));
}

#[test]
fn half_levels_average_in_premultiplied_space() {
    let mut s = Image::new(5, 3, 16, CLEAR);
    s.write(Rect::new(0, 0, 2, 1), &[[255, 0, 0, 255], [0, 0, 0, 0]]);
    s.fill_rect(Rect::new(2, 0, 5, 3), [0, 0, 255, 255]);
    let h = s.half();
    assert_eq!((h.width(), h.height()), (3, 2));
    // Red over transparent: red at half alpha, not a dark red.
    let p = h.pixel(0, 0);
    assert_eq!((p[0], p[2]), (255, 0));
    assert!((p[3] as i32 - 64).abs() <= 1, "{p:?}");
    assert_eq!(h.pixel(2, 1), [0, 0, 255, 255]);
}

#[test]
fn other_pixel_types_paint_and_undo_too() {
    let mut s = Surface::<Rgba16>::new(80, 40, 16, Rgba16([0; 4]));
    let red = Rgba16([65535, 0, 0, 65535]);
    let patch = {
        let mut st = s.begin_stroke(None);
        st.prepare(Rect::new(0, 0, 40, 20));
        let mut buf = Vec::new();
        st.spans_mut(Rect::new(0, 0, 40, 20), &mut buf);
        for mut sp in buf { for i in 0..sp.len() { let m = sp.cover(i, 255); sp.pixels[i] = sp.base[i].over(red, m); } }
        st.end()
    };
    assert_eq!(s.pixel(10, 10), red);
    assert_eq!(s.stored().uniform, 2, "covered tiles are one color");
    let mut p = patch;
    assert!(p.swap(&mut s));
    assert_eq!(s.stored(), Stored::default());
    assert!(Surface::<Rgba16>::load(&s.save(), 100).is_some());
}

#[test]
fn the_pool_gives_buffers_back() {
    let v = crate::pool::take_pixels::<Rgba8>(256, CLEAR);
    crate::pool::give_pixels(v);
    assert!(crate::pool::held_bytes() >= 1024);
    let again = crate::pool::take_pixels::<Rgba8>(256, [1, 2, 3, 4]);
    assert!(again.iter().all(|p| *p == [1, 2, 3, 4]), "a reused buffer is filled again");
    crate::pool::trim(std::time::Duration::ZERO);
    let m = crate::pool::take_mask(64);
    assert!(m.iter().all(|v| *v == 0));
}

#[test]
fn base_pixels_are_read_from_before_the_stroke() {
    let mut s = Image::new(32, 32, 16, CLEAR);
    s.fill_rect(Rect::new(0, 0, 32, 32), [9, 9, 9, 255]);
    let mut st = s.begin_stroke(None);
    dab(&mut st, Rect::new(0, 0, 8, 8), [200, 0, 0], 255);
    assert_eq!(st.base_pixel(3, 3), [9, 9, 9, 255]);
    assert_eq!(st.surface().pixel(3, 3), [200, 0, 0, 255]);
    assert_eq!(st.coverage(3, 3), 255);
    st.end();
}

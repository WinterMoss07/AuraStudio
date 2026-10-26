//! Layer thumbnails: the layer's pixels averaged down, box by box, so a
//! thin line shows as a faint line instead of vanishing or flickering
//! (picking one pixel in N does that).

use aurastudio_canvas::Rect;
use aurastudio_layers::Tiles;

/// Size of a thumbnail of a `w` × `h` layer that fits `max` pixels on its
/// longer side (never enlarged).
pub fn target_size(w: u32, h: u32, max: u32) -> (usize, usize) {
    let (w, h) = (w.max(1), h.max(1));
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * k).round() as u32).clamp(1, w);
    let th = ((h as f32 * k).round() as u32).clamp(1, h);
    (tw as usize, th as usize)
}

/// `tiles` reduced to fit `max` pixels on its longer side (never
/// enlarged): width, height and straight RGBA.
#[cfg(test)]
pub fn area_average(tiles: &Tiles, max: u32) -> (usize, usize, Vec<u8>) {
    let (tw, th) = target_size(tiles.width(), tiles.height(), max);
    let mut out = vec![0u8; tw * th * 4];
    area_average_into(tiles, tw, th, None, &mut out);
    (tw, th, out)
}

/// Where thumbnail pixel `i` of `n` starts and ends in a side of `size`
/// (at least one source pixel).
fn span(i: usize, n: usize, size: u32) -> (u32, u32) {
    let at = |i: usize| (i as u64 * size as u64 / n as u64) as u32;
    let start = at(i);
    (start, at(i + 1).max(start + 1).min(size))
}

/// The thumbnail `out` (`tw` × `th`, straight RGBA) of `tiles`: all of it,
/// or (`rect`: x0, y0, x1, y1 of the layer) only the pixels whose box
/// touches `rect`, the others left as they are. What it writes is exactly
/// what the full computation gives, so a partial update after a change
/// inside `rect` equals a new thumbnail.
pub fn area_average_into(tiles: &Tiles, tw: usize, th: usize, rect: Option<Rect>, out: &mut [u8]) {
    assert_eq!(out.len(), tw * th * 4, "the thumbnail buffer has the thumbnail's size");
    let (w, h) = (tiles.width().max(1), tiles.height().max(1));
    // The thumbnail pixels to do.
    let (tx0, tx1, ty0, ty1) = match rect {
        None => (0, tw, 0, th),
        Some(r) => {
            let (x0, y0, x1, y1) = (r.0.min(w), r.1.min(h), r.2.min(w), r.3.min(h));
            if x1 <= x0 || y1 <= y0 { return; }
            // Conservative by one pixel each side (a box may start a pixel early).
            let lo = |v: u32, n: usize, size: u32| ((v as u64 * n as u64 / size as u64) as usize).saturating_sub(1);
            let hi = |v: u32, n: usize, size: u32| (((v as u64 * n as u64).div_ceil(size as u64)) as usize + 1).min(n);
            (lo(x0, tw, w), hi(x1, tw, w), lo(y0, th, h), hi(y1, th, h))
        }
    };
    for ty in ty0..ty1 {
        let (y0, y1) = span(ty, th, h);
        for tx in tx0..tx1 {
            let (x0, x1) = span(tx, tw, w);
            // Premultiplied sums: r, g, b, a (u64: a 8K box of 255s stays
            // far below the limit).
            let mut s = [0u64; 4];
            for y in y0..y1 {
                tiles.for_row(y, x0, x1, |_, n, px| match px {
                    Some(px) => for p in px.chunks_exact(4) { add(&mut s, p, 1); },
                    None => add(&mut s, &tiles.fill(), n as u64),
                });
            }
            let n = ((y1 - y0) as u64) * ((x1 - x0) as u64);
            let a = s[3];
            let o = (ty * tw + tx) * 4;
            if a == 0 { out[o..o + 4].copy_from_slice(&[0, 0, 0, 0]); continue; }
            let c = |v: u64| ((v + a / 2) / a).min(255) as u8;
            out[o..o + 4].copy_from_slice(&[c(s[0]), c(s[1]), c(s[2]), ((a + n / 2) / n).min(255) as u8]);
        }
    }
}

/// One pixel `p` (straight RGBA) counted `n` times, weighted by alpha.
fn add(s: &mut [u64; 4], p: &[u8], n: u64) {
    let a = p[3] as u64;
    s[0] += p[0] as u64 * a * n;
    s[1] += p[1] as u64 * a * n;
    s[2] += p[2] as u64 * a * n;
    s[3] += a * n;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thin_line_shows_as_a_faint_line_not_nothing() {
        // 256 × 256, one black column 1 px wide, reduced 4×.
        let (w, h) = (256u32, 256u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h { let i = ((y * w + 101) * 4) as usize; rgba[i + 3] = 255; }
        let tiles = Tiles::from_rgba(w, h, &rgba, [0; 4]);
        let (tw, th, px) = area_average(&tiles, 64);
        assert_eq!((tw, th), (64, 64));
        // Column 101 / 4 = 25 holds a quarter alpha, black; the rest nothing.
        let at = |x: usize, y: usize| &px[(y * tw + x) * 4..][..4];
        assert_eq!(at(25, 10), &[0, 0, 0, 64]);
        assert_eq!(at(24, 10)[3], 0);
        assert_eq!(at(26, 10)[3], 0);
    }

    #[test]
    fn colors_are_weighted_by_alpha_and_the_fill_counts() {
        // Half the box red opaque, half transparent: red at half alpha, not dark red.
        let mut rgba = vec![0u8; 4 * 2 * 4];
        for y in 0..2 { for x in 0..2 { let i = (y * 4 + x) * 4; rgba[i..i + 4].copy_from_slice(&[200, 0, 0, 255]); } }
        let tiles = Tiles::from_rgba(4, 2, &rgba, [0; 4]);
        let (tw, th, px) = area_average(&tiles, 1);
        assert_eq!((tw, th), (1, 1));
        assert_eq!(&px[..], &[200, 0, 0, 128]);
        // An all-fill layer is its fill, whatever the size.
        let paper = Tiles::new(1000, 700, [255, 255, 255, 255]);
        let (tw, th, px) = area_average(&paper, 64);
        assert_eq!(th, 45);
        assert!(px.chunks_exact(4).all(|p| p == [255, 255, 255, 255]) && tw == 64);
    }

    /// Deterministic noise.
    fn noise(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    /// A partial update after a change inside the rectangle equals a new
    /// thumbnail, to the byte, wherever the rectangle is.
    #[test]
    fn a_partial_update_equals_a_full_recompute() {
        let (w, h) = (700u32, 500u32);
        let mut seed = 5u32;
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| if i % 4 == 3 { if noise(&mut seed).is_multiple_of(3) { 0 } else { 255 } } else { (noise(&mut seed) & 255) as u8 }).collect();
        for max in [64u32, 56, 41] {
            let mut tiles = Tiles::from_rgba(w, h, &rgba, [0; 4]);
            let (tw, th, mut thumb) = area_average(&tiles, max);
            for round in 0..24 {
                // A rectangle anywhere, edges and corners included.
                let (x0, y0) = (noise(&mut seed) % w, noise(&mut seed) % h);
                let (x1, y1) = ((x0 + 1 + noise(&mut seed) % 300).min(w), (y0 + 1 + noise(&mut seed) % 200).min(h));
                let (x0, y0) = if round == 0 { (0, 0) } else if round == 1 { (w - 5, h - 5) } else { (x0, y0) };
                let (x1, y1) = if round == 0 { (3, 3) } else if round == 1 { (w, h) } else { (x1, y1) };
                let data: Vec<u8> = (0..(x1 - x0) * (y1 - y0) * 4).map(|_| (noise(&mut seed) & 255) as u8).collect();
                tiles.write_rect((x0, y0, x1, y1), &data);
                area_average_into(&tiles, tw, th, Some((x0, y0, x1, y1)), &mut thumb);
                let (_, _, full) = area_average(&tiles, max);
                assert_eq!(thumb, full, "max {max}, round {round}, rect {:?}", (x0, y0, x1, y1));
            }
        }
    }

    #[test]
    fn a_rectangle_off_the_layer_changes_nothing() {
        let tiles = Tiles::new(100, 100, [9, 9, 9, 255]);
        let mut out = vec![7u8; 10 * 10 * 4];
        area_average_into(&tiles, 10, 10, Some((200, 200, 300, 300)), &mut out);
        area_average_into(&tiles, 10, 10, Some((5, 5, 5, 9)), &mut out);
        assert!(out.iter().all(|b| *b == 7));
    }

    #[test]
    fn thumbnails_follow_the_screen_scale() {
        // The docker draws them 28 points wide: 56 pixels at 200 %, 42 at 150 %.
        assert_eq!(target_size(2000, 3000, (28.0f32 * 2.0).round() as u32), (37, 56));
        assert_eq!(target_size(2000, 3000, (28.0f32 * 1.5).round() as u32), (28, 42));
        assert_eq!(target_size(30, 20, 56), (30, 20));
    }

    #[test]
    fn a_small_layer_is_not_enlarged() {
        let t = Tiles::new(30, 20, [0; 4]);
        let (tw, th, px) = area_average(&t, 64);
        assert_eq!((tw, th, px.len()), (30, 20, 30 * 20 * 4));
    }

    /// 300 × 300 touched on a 2000 × 3000 layer: the update is cheap (release only).
    #[test]
    #[ignore]
    fn partial_update_speed() {
        let (w, h) = (2000u32, 3000u32);
        let rgba: Vec<u8> = (0..w as usize * h as usize * 4).map(|i| (i * 7 % 251) as u8 | 1).collect();
        let tiles = Tiles::from_rgba(w, h, &rgba, [0; 4]);
        let (tw, th) = target_size(w, h, 56);
        let mut out = vec![0u8; tw * th * 4];
        let t = std::time::Instant::now();
        for _ in 0..20 { area_average_into(&tiles, tw, th, Some((500, 900, 800, 1200)), &mut out); }
        let each = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
        println!("partial {each:.2} ms");
        assert!(each < 1.5, "{each:.2} ms");
    }
}

//! Layer thumbnails: the layer's pixels averaged down, box by box, so a
//! thin line shows as a faint line instead of vanishing or flickering
//! (picking one pixel in N does that).

use aurastudio_layers::Tiles;

/// `tiles` reduced to fit `max` pixels on its longer side (never
/// enlarged): width, height and straight RGBA.
pub fn area_average(tiles: &Tiles, max: u32) -> (usize, usize, Vec<u8>) {
    let (w, h) = (tiles.width().max(1), tiles.height().max(1));
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * k).round() as u32).clamp(1, w);
    let th = ((h as f32 * k).round() as u32).clamp(1, h);
    // Premultiplied sums per thumbnail pixel: r, g, b, a (u64: a 8K box
    // of 255s stays far below the limit).
    let mut sums = vec![[0u64; 4]; (tw * th) as usize];
    let edge = |i: u32, n: u32, size: u32| (i as u64 * size as u64 / n as u64) as u32;
    for ty in 0..th {
        let (y0, y1) = (edge(ty, th, h), edge(ty + 1, th, h).max(edge(ty, th, h) + 1).min(h));
        for tx in 0..tw {
            let (x0, x1) = (edge(tx, tw, w), edge(tx + 1, tw, w).max(edge(tx, tw, w) + 1).min(w));
            let s = &mut sums[(ty * tw + tx) as usize];
            for y in y0..y1 {
                tiles.for_row(y, x0, x1, |_, n, px| match px {
                    Some(px) => for p in px.chunks_exact(4) { add(s, p, 1); },
                    None => add(s, &tiles.fill(), n as u64),
                });
            }
        }
    }
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let (y0, y1) = (edge(ty, th, h), edge(ty + 1, th, h).max(edge(ty, th, h) + 1).min(h));
        for tx in 0..tw {
            let (x0, x1) = (edge(tx, tw, w), edge(tx + 1, tw, w).max(edge(tx, tw, w) + 1).min(w));
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let s = sums[(ty * tw + tx) as usize];
            let a = s[3];
            if a == 0 { out.extend_from_slice(&[0, 0, 0, 0]); continue; }
            let c = |v: u64| ((v + a / 2) / a).min(255) as u8;
            out.extend_from_slice(&[c(s[0]), c(s[1]), c(s[2]), ((a + n / 2) / n).min(255) as u8]);
        }
    }
    (tw as usize, th as usize, out)
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

    #[test]
    fn a_small_layer_is_not_enlarged() {
        let t = Tiles::new(30, 20, [0; 4]);
        let (tw, th, px) = area_average(&t, 64);
        assert_eq!((tw, th, px.len()), (30, 20, 30 * 20 * 4));
    }
}

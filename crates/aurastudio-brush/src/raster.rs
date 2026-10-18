//! Row helpers shared by the brush engines: rows are processed in chunks,
//! first the new coverage of a whole chunk (plain arithmetic the compiler
//! vectorizes), then the layer pixels only where it changed.

use aurastudio_canvas::{blend::paint_pixel_u8, BlendMode};

/// Pixels per chunk of a row.
pub const CHUNK: usize = 64;
/// 0, 1, 2... as floats (x offsets inside a chunk, without int→float
/// conversions in the vectorized loop).
pub const IOTA: [f32; CHUNK] = {
    let mut a = [0.0; CHUNK];
    let mut i = 0;
    while i < CHUNK { a[i] = i as f32; i += 1; }
    a
};

/// Smallest value of a mask run (vectorized 16 at a time).
pub fn min_u8(v: &[u8]) -> u8 {
    let mut chunks = v.chunks_exact(16);
    let mut m = [u8::MAX; 16];
    for c in &mut chunks {
        for (a, &b) in m.iter_mut().zip(c) { *a = (*a).min(b); }
    }
    let rest = chunks.remainder().iter().fold(u8::MAX, |a, &b| a.min(b));
    m.iter().fold(rest, |a, &b| a.min(b))
}

/// First and one-past-last index where `new` differs from `old`.
pub fn changed_span(old: &[u8], new: &[u8]) -> Option<(usize, usize)> {
    let c0 = old.iter().zip(new).position(|(a, b)| a != b)?;
    let c1 = old.len() - old.iter().zip(new).rev().position(|(a, b)| a != b).unwrap_or(0);
    Some((c0, c1))
}

/// Layer pixels = brush color over the pre-stroke pixels at the given
/// coverages. Recomputing an unchanged pixel gives the same value, so whole
/// runs are written without per-pixel tests; normal blend over an opaque or
/// empty run is plain arithmetic the compiler vectorizes.
pub fn write_pixels(out: &mut [u8], base: &[u8], cov: &[u8], color: [u8; 3], blend: BlendMode) {
    if blend == BlendMode::Normal {
        if base.chunks_exact(4).all(|b| b[3] == 255) {
            // Byte by byte over the whole run (RGBA alike: the alpha byte
            // gives 255 again), with each pixel's coverage repeated 4 times.
            let mut a4 = [0u8; CHUNK * 4];
            let a4 = &mut a4[..out.len()];
            for (d, &a) in a4.chunks_exact_mut(4).zip(cov) { d.fill(a); }
            let mut c4 = [255u8; CHUNK * 4];
            for d in c4.chunks_exact_mut(4) { d[..3].copy_from_slice(&color); }
            for (((o, &b), &a), &c) in out.iter_mut().zip(base).zip(a4.iter()).zip(c4.iter()) {
                let (a, c) = (a as u16, c as u16);
                *o = ((c * a + b as u16 * (255 - a) + 127) / 255) as u8;
            }
            return;
        }
        if base.chunks_exact(4).all(|b| b[3] == 0) {
            for (o, &a) in out.chunks_exact_mut(4).zip(cov) {
                o.copy_from_slice(&[color[0], color[1], color[2], a]);
            }
            return;
        }
    }
    for ((o, b), &a) in out.chunks_exact_mut(4).zip(base.chunks_exact(4)).zip(cov) {
        o.copy_from_slice(&paint_pixel_u8([b[0], b[1], b[2], b[3]], color, a, blend));
    }
}


/// Horizontal extent [x0, x1] of the capsule (segment `a`-`b`, radius
/// `reach`) on the row at height `y`, or None if the row misses it.
///
/// The capsule is convex, so its row section is one interval: the union
/// of the two end circles' sections and the swept band's section.
pub fn capsule_row_span(a: (f32, f32), b: (f32, f32), reach: f32, y: f32) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut add = |l: f32, h: f32| { lo = lo.min(l); hi = hi.max(h); };

    for c in [a, b] {
        let dy = y - c.1;
        let h2 = reach * reach - dy * dy;
        if h2 > 0.0 {
            let h = h2.sqrt();
            add(c.0 - h, c.0 + h);
        }
    }

    // Band: points whose projection falls inside the segment and whose
    // distance to its line is under `reach`.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len > 1e-3 && dx.abs() > 1e-6 {
        let (ux, uy) = (dx / len, dy / len);
        // Along the segment: 0 <= (x - a.x) * ux + (y - a.y) * uy <= len.
        let s0 = (0.0 - (y - a.1) * uy) / ux + a.0;
        let s1 = (len - (y - a.1) * uy) / ux + a.0;
        let (t_lo, t_hi) = (s0.min(s1), s0.max(s1));
        // Across: |(x - a.x) * -uy + (y - a.y) * ux| < reach.
        let (b_lo, b_hi) = if uy.abs() > 1e-6 {
            let c0 = ((y - a.1) * ux - reach) / uy + a.0;
            let c1 = ((y - a.1) * ux + reach) / uy + a.0;
            (c0.min(c1), c0.max(c1))
        } else if ((y - a.1) * ux).abs() < reach {
            (f32::NEG_INFINITY, f32::INFINITY)
        } else {
            (1.0, 0.0)
        };
        let (l, h) = (t_lo.max(b_lo), t_hi.min(b_hi));
        if l <= h { add(l, h); }
    } else if len > 1e-3 {
        // Vertical segment: the band is a column between the end circles.
        let (ymin, ymax) = (a.1.min(b.1), a.1.max(b.1));
        if y >= ymin && y <= ymax { add(a.0 - reach, a.0 + reach); }
    }

    (lo <= hi).then_some((lo, hi))
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The row-span shortcut must never skip a pixel the full scan paints:
    /// compare with a brute-force reference on segments in all directions.
    #[test]
    fn row_spans_cover_every_painted_pixel() {
        let reach = 9.5;
        for (a, b) in [((40.0, 60.0), (90.0, 70.0)), ((64.0, 20.0), (64.0, 100.0)), ((30.0, 90.0), (95.0, 30.0)),
                       ((50.0, 50.0), (50.3, 50.1)), ((20.0, 64.0), (110.0, 64.0))] {
            for py in 0..128 {
                let y = py as f32 + 0.5;
                let span = capsule_row_span(a, b, reach, y);
                for px in 0..128 {
                    let x = px as f32 + 0.5;
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let (ex, ey) = (x - (a.0 + dx * t), y - (a.1 + dy * t));
                    if ex * ex + ey * ey < reach * reach {
                        let (lo, hi) = span.unwrap_or_else(|| panic!("row {py} missed for {a:?}-{b:?}"));
                        assert!(x >= lo.floor() && x <= hi.ceil() + 1.0, "px {px},{py} outside {lo}..{hi}");
                    }
                }
            }
        }
    }
}

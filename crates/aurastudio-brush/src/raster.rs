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

/// Eraser: the pre-stroke pixels with their alpha lowered by the
/// coverage (color kept, so a half-erased pixel keeps its hue).
pub fn erase_pixels(out: &mut [u8], base: &[u8], cov: &[u8]) {
    for ((o, b), &a) in out.chunks_exact_mut(4).zip(base.chunks_exact(4)).zip(cov) {
        let alpha = (b[3] as u16 * (255 - a as u16) + 127) / 255;
        o.copy_from_slice(&[b[0], b[1], b[2], alpha as u8]);
    }
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


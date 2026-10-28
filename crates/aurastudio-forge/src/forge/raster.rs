//! Row helpers shared by the brush engines: rows are processed in chunks,
//! first the new coverage of a whole chunk (plain arithmetic the compiler
//! vectorizes), then the layer pixels only where it changed.

use aurastudio_core::{blend::paint_pixel_u8, BlendMode};

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

/// `v` (0 ≤ v < 65536) rounded to the nearest integer. Branch-free, so the
/// loops using it vectorize: Rust's saturating `as` casts compile to
/// scalar code per lane (most of a soft dab's time).
#[inline(always)]
pub fn round_small(v: f32) -> u16 {
    // Adding 2^23 puts the rounded integer in the low mantissa bits.
    (v + 8_388_608.0).to_bits() as u16
}

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

/// Brush color over the pre-stroke pixels at these coverages (whole runs,
/// vectorized).
pub fn write_pixels(out: &mut [u8], base: &[u8], cov: &[u8], color: [u8; 3], blend: BlendMode) {
    if blend == BlendMode::Normal {
        // Opaque run (AND) or empty run (OR), in one pass.
        let (mut and, mut or) = (u32::MAX, 0u32);
        for b in base.chunks_exact(4) {
            let w = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            and &= w;
            or |= w;
        }
        if and >> 24 == 255 { return over_opaque(out, base, cov, color); }
        if or >> 24 == 0 { return over_empty(out, cov, color); }
        // Mixed (over the edge of earlier paint): by stretches of opaque,
        // empty and partly covered pixels, the first two still vectorized.
        let class = |i: usize| match base[i * 4 + 3] { 255 => 0u8, 0 => 1, _ => 2 };
        let mut i = 0;
        while i < cov.len() {
            let k = class(i);
            let mut j = i + 1;
            while j < cov.len() && class(j) == k { j += 1; }
            let (o, b, c) = (&mut out[i * 4..j * 4], &base[i * 4..j * 4], &cov[i..j]);
            match k {
                0 => over_opaque(o, b, c, color),
                1 => over_empty(o, c, color),
                _ => over_partial(o, b, c, color),
            }
            i = j;
        }
        return;
    }
    for ((o, b), &a) in out.chunks_exact_mut(4).zip(base.chunks_exact(4)).zip(cov) {
        o.copy_from_slice(&paint_pixel_u8([b[0], b[1], b[2], b[3]], color, a, blend));
    }
}

/// Normal blending over opaque pixels (paint_pixel_u8's formula, byte by
/// byte: the alpha byte stays 255). At most CHUNK pixels.
fn over_opaque(out: &mut [u8], base: &[u8], cov: &[u8], color: [u8; 3]) {
    let mut a4 = [0u8; CHUNK * 4];
    let a4 = &mut a4[..out.len()];
    for (d, &a) in a4.chunks_exact_mut(4).zip(cov) {
        d.copy_from_slice(&(a as u32 * 0x0101_0101).to_le_bytes());
    }
    let mut c4 = [0u8; CHUNK * 4];
    let c = [color[0], color[1], color[2], 255];
    for d in c4.chunks_exact_mut(4) { d.copy_from_slice(&c); }
    for (((o, &b), &a), &c) in out.iter_mut().zip(base).zip(a4.iter()).zip(c4.iter()) {
        let (a, c) = (a as u16, c as u16);
        *o = ((c * a + b as u16 * (255 - a) + 127) / 255) as u8;
    }
}

/// Normal blending over partly covered pixels: blend_pixel_u8's float
/// math, operation for operation (the same bytes), in plain loops the
/// compiler vectorizes. At most CHUNK pixels.
fn over_partial(out: &mut [u8], base: &[u8], cov: &[u8], color: [u8; 3]) {
    let n = cov.len();
    let cs = [color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0];
    let mut a_s = [0.0f32; CHUNK];
    let mut a_b = [0.0f32; CHUNK];
    for (s, &c) in a_s[..n].iter_mut().zip(cov) { *s = (c as f32 / 255.0).clamp(0.0, 1.0); }
    for (b, p) in a_b[..n].iter_mut().zip(base.chunks_exact(4)) { *b = p[3] as f32 / 255.0; }
    // Alpha: a_s + a_b (1 - a_s), then its reciprocal.
    let mut inv = [0.0f32; CHUNK];
    let mut a8 = [0u8; CHUNK];
    for i in 0..n {
        let a = (a_s[i] + a_b[i] * (1.0 - a_s[i])).clamp(0.0, 1.0);
        inv[i] = 1.0 / a;
        a8[i] = (a * 255.0 + 0.5) as u8;
    }
    for c in 0..3 {
        let mut v = [0u8; CHUNK];
        for i in 0..n {
            let p = base[i * 4 + c] as f32 / 255.0 * a_b[i];
            let d = cs[c] * a_s[i] + p * (1.0 - a_s[i]);
            v[i] = ((d * inv[i]).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        for (o, &v) in out.chunks_exact_mut(4).zip(&v[..n]) { o[c] = v; }
    }
    for (o, &a) in out.chunks_exact_mut(4).zip(&a8[..n]) { o[3] = a; }
}

/// Normal blending over empty pixels: the color at the coverage.
fn over_empty(out: &mut [u8], cov: &[u8], color: [u8; 3]) {
    for (o, &a) in out.chunks_exact_mut(4).zip(cov) {
        o.copy_from_slice(&[color[0], color[1], color[2], a]);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The vectorized blend over partly covered pixels gives exactly
    /// paint_pixel_u8's bytes (every alpha and coverage, varied colors).
    #[test]
    fn partial_blend_matches_paint_pixel() {
        let mut v = 1u32;
        let mut next = || { v = v.wrapping_mul(1664525).wrapping_add(1013904223); (v >> 24) as u8 };
        for ab in 1..=254u8 {
            for cov0 in (0..=255u16).step_by(CHUNK) {
                let n = CHUNK.min(256 - cov0 as usize);
                let color = [next(), next(), next()];
                let base: Vec<u8> = (0..n).flat_map(|_| [next(), next(), next(), ab]).collect();
                let cov: Vec<u8> = (0..n).map(|i| (cov0 as usize + i) as u8).collect();
                let mut out = vec![0u8; n * 4];
                over_partial(&mut out, &base, &cov, color);
                for i in 0..n {
                    let b = &base[i * 4..i * 4 + 4];
                    let want = paint_pixel_u8([b[0], b[1], b[2], b[3]], color, cov[i], BlendMode::Normal);
                    assert_eq!(&out[i * 4..i * 4 + 4], &want, "alpha {ab} cov {}", cov[i]);
                }
            }
        }
    }
}

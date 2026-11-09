//! The one Gaussian blur (W3C feGaussianBlur: three box blurs from σ = 2,
//! the Gaussian itself below), on 16-bit planes: Gaussian blur, Unsharp
//! Mask, Drop Shadow and the selection's feather all go through it.

use aurastudio_core::{math, Rgba8};
use rayon::prelude::*;

/// 16-bit premultiplied planes (65535 = 1.0), one per channel (16 bits so
/// that three box passes don't band a soft gradient).
pub type Planes = [Vec<u16>; 4];

pub fn premultiply(src: &[Rgba8]) -> Planes {
    let mut planes: Planes = Default::default();
    for p in &mut planes { p.reserve_exact(src.len()); }
    for px in src {
        let a = px[3] as u32;
        for c in 0..3 { planes[c].push(((px[c] as u32 * a * 257 + 127) / 255) as u16); }
        planes[3].push((a * 257) as u16);
    }
    planes
}

/// Back to straight 8-bit (rounded once, by `core::math`); nearly
/// transparent pixels (alpha < 0.5 level) become transparent black.
pub fn unpremultiply(planes: &Planes) -> Vec<Rgba8> {
    let [r, g, b, a] = planes;
    a.iter().enumerate().map(|(i, &a)| {
        if a < 129 { return [0; 4]; }
        let a = a as f32;
        let c = |p: &Vec<u16>| math::to_u8((p[i] as f32 / a).min(1.0));
        [c(r), c(g), c(b), math::to_u8(a / 65535.0)]
    }).collect()
}

/// How far (px) a blur of `sigma` reads around a pixel.
pub fn reach(sigma: f32) -> u32 { if sigma <= 0.0 { 0 } else { (3.0 * sigma).ceil() as u32 + 2 } }

/// Blur one `w` × `h` plane in place, edges repeated.
pub fn gaussian(px: &mut [u16], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 || w == 0 || h == 0 || px.len() != w * h { return; }
    if sigma >= 2.0 {
        let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
        let boxes = if d % 2 == 1 { [(d / 2, d / 2); 3] } else { [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)] };
        px.par_chunks_exact_mut(w).for_each_init(|| vec![0u16; w], |tmp, row| {
            for &(l, r) in &boxes { box_row(row, tmp, l, r); }
        });
        let mut prev = vec![0u16; px.len()];
        for &(l, r) in &boxes { box_columns(px, &mut prev, w, h, l, r); }
        return;
    }
    let k = kernel(sigma);
    let r = k.len() / 2;
    px.par_chunks_exact_mut(w).for_each_init(|| vec![0u16; w], |tmp, row| {
        tmp.copy_from_slice(row);
        for (i, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() { s += kv * tmp[(i + j).saturating_sub(r).min(w - 1)] as f32; }
            *o = (s + 0.5) as u16;
        }
    });
    // Columns: whole rows at a time (row-major, no transpose), on all cores.
    let prev = px.to_vec();
    px.par_chunks_exact_mut(w).enumerate().for_each_init(|| vec![0f32; w], |acc, (y, out)| {
        acc.fill(0.0);
        for (j, kv) in k.iter().enumerate() {
            let sy = (y + j).saturating_sub(r).min(h - 1);
            for (a, &v) in acc.iter_mut().zip(&prev[sy * w..(sy + 1) * w]) { *a += kv * v as f32; }
        }
        for (o, a) in out.iter_mut().zip(acc.iter()) { *o = (a + 0.5) as u16; }
    });
}

/// The normalized Gaussian, ±3σ.
pub fn kernel(sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as usize;
    let k: Vec<f32> = (0..=2 * r).map(|i| { let x = i as f32 - r as f32; (-x * x / (2.0 * sigma * sigma)).exp() }).collect();
    let total: f32 = k.iter().sum();
    k.iter().map(|v| v / total).collect()
}

fn box_row(row: &mut [u16], tmp: &mut [u16], l: usize, r: usize) {
    let w = row.len();
    tmp.copy_from_slice(row);
    let inv = 1.0 / (l + 1 + r) as f32;
    let at = |x: isize| tmp[x.clamp(0, w as isize - 1) as usize] as u32;
    let mut sum: u32 = (-(l as isize)..=r as isize).map(at).sum();
    for (i, o) in row.iter_mut().enumerate() {
        *o = (sum as f32 * inv + 0.5) as u16;
        sum = sum + at(i as isize + r as isize + 1) - at(i as isize - l as isize);
    }
}

/// A box along the columns: a running sum per column, swept down the
/// rows (whole rows at a time).
fn box_columns(px: &mut [u16], prev: &mut [u16], w: usize, h: usize, l: usize, r: usize) {
    prev.copy_from_slice(px);
    let prev = &*prev;
    let inv = 1.0 / (l + 1 + r) as f32;
    let row = |y: isize| { let y = y.clamp(0, h as isize - 1) as usize; y * w..(y + 1) * w };
    // Bands of rows on all cores, each with its own running sums.
    let band = h.div_ceil(rayon::current_num_threads().max(1) * 2).max(32);
    px.par_chunks_mut(w * band).enumerate().for_each(|(b, out)| {
        let y0 = (b * band) as isize;
        let mut sum = vec![0u32; w];
        for j in y0 - l as isize..=y0 + r as isize {
            for (s, &v) in sum.iter_mut().zip(&prev[row(j)]) { *s += v as u32; }
        }
        for (k, o) in out.chunks_exact_mut(w).enumerate() {
            let y = y0 + k as isize;
            for (o, &s) in o.iter_mut().zip(&sum) { *o = (s as f32 * inv + 0.5) as u16; }
            let (add, sub) = (row(y + r as isize + 1), row(y - l as isize));
            for ((s, &a), &b) in sum.iter_mut().zip(&prev[add]).zip(&prev[sub]) { *s = *s + a as u32 - b as u32; }
        }
    });
}

/// Gaussian blur of straight RGBA (premultiplied while blurring: no dark
/// fringe around a line); each plane's rows on all cores.
pub fn blur_rgba(src: &[Rgba8], w: usize, h: usize, sigma: f32) -> Vec<Rgba8> {
    if sigma <= 0.0 { return src.to_vec(); }
    let mut planes = premultiply(src);
    planes.par_iter_mut().for_each(|p| gaussian(p, w, h, sigma));
    unpremultiply(&planes)
}

/// Gaussian blur of one 8-bit plane (a mask, a selection's feather).
pub fn blur_u8(src: &[u8], w: usize, h: usize, sigma: f32) -> Vec<u8> {
    if sigma <= 0.0 { return src.to_vec(); }
    let mut p: Vec<u16> = src.iter().map(|&v| v as u16 * 257).collect();
    gaussian(&mut p, w, h, sigma);
    p.iter().map(|&v| ((v as u32 + 128) / 257) as u8).collect()
}

//! Filters reading neighbors: Unsharp Mask, Thicken / Thin lines, Find
//! Edges, Emboss, Drop Shadow (after PhotoCraft, MIT, see NOTICE). Edges
//! of the image repeated; rows shared out between the cores.

use aurastudio_core::{math, Rgba8};
use rayon::prelude::*;

use crate::{blur, extreme};

/// Rows of a `w`-wide image, in parallel, with their y.
fn par_rows(out: &mut [Rgba8], w: usize, f: impl Fn(usize, &mut [Rgba8]) + Sync + Send) {
    if w == 0 { return; }
    out.par_chunks_mut(w).enumerate().with_min_len(16).for_each(|(y, row)| f(y, row));
}

/// Unsharp Mask: each color plus `amount` × its difference from a blurred
/// copy, where that difference reaches `threshold` levels. Straight
/// colors (the blur is premultiplied: no dark fringe at a line's edge),
/// alpha unchanged.
pub fn unsharp(src: &[Rgba8], w: usize, h: usize, amount: f32, sigma: f32, threshold: f32) -> Vec<Rgba8> {
    let blurred = blur::blur_rgba(src, w, h, sigma);
    let mut out = src.to_vec();
    out.par_iter_mut().zip(blurred.par_iter()).with_min_len(4096).for_each(|(o, b)| {
        if o[3] == 0 || b[3] == 0 { return; }
        for c in 0..3 {
            let d = o[c] as f32 - b[c] as f32;
            if d.abs() + 1e-3 >= threshold { o[c] = (o[c] as f32 + amount * d).round().clamp(0.0, 255.0) as u8; }
        }
    });
    out
}

/// Thicken or thin dark lines (Minimum / Maximum), square or round, on
/// paper or line art.
pub fn lines(src: &[Rgba8], w: usize, h: usize, radius: f32, round: bool, grow: bool) -> Vec<Rgba8> {
    if radius < 0.5 || w == 0 || h == 0 { return src.to_vec(); }
    // A transparent pixel's color doesn't count: lightest for a min,
    // darkest for a max.
    let hidden = if grow { 255 } else { 0 };
    let mut planes: [Vec<u8>; 4] = Default::default();
    for p in &mut planes { p.reserve_exact(src.len()); }
    for px in src {
        for c in 0..3 { planes[c].push(if px[3] == 0 { hidden } else { px[c] }); }
        planes[3].push(px[3]);
    }
    planes.par_iter_mut().enumerate().for_each(|(c, p)| {
        let max = (c == 3) == grow;
        let op: fn(u8, u8) -> u8 = if max { u8::max } else { u8::min };
        if round { extreme::disc(p, w, h, radius, op) } else { extreme::square(p, w, h, radius.round() as usize, op) }
    });
    let [r, g, b, a] = &planes;
    (0..w * h).map(|i| if a[i] == 0 { [0; 4] } else { [r[i], g[i], b[i], a[i]] }).collect()
}

/// The colors over white, 0..1 (a transparent layer's lines then read as
/// dark, the empty parts as white).
fn over_white(src: &[Rgba8]) -> Vec<[f32; 3]> {
    src.par_iter().with_min_len(4096).map(|p| {
        let a = math::from_u8(p[3]);
        std::array::from_fn(|c| math::from_u8(p[c]) * a + 1.0 - a)
    }).collect()
}

/// Find Edges on one pixel: `e` the edge per color (0..1), `edge` the
/// strongest. Also what a uniform area becomes (no edge: towards white).
pub fn edge_pixel(p: Rgba8, e: [f32; 3], edge: f32, strength: f32) -> Rgba8 {
    let a = math::from_u8(p[3]);
    // Where the layer is transparent the result is the edge itself.
    let na = a.max(edge * strength);
    if na <= 0.0 { return p; }
    let mut o = p;
    for k in 0..3 {
        let base = if a > 0.0 { math::from_u8(p[k]) } else { 1.0 };
        o[k] = math::to_u8((base + ((1.0 - e[k]) - base) * strength).clamp(0.0, 1.0));
    }
    o[3] = math::to_u8(na.min(1.0));
    o
}

/// Find Edges: the Sobel gradient of each color, dark edges on white (a
/// 60-level step darkens by 240, as Photoshop's), mixed with the source
/// by `strength`. On a transparent layer the edges around the lines show.
pub fn find_edges(src: &[Rgba8], w: usize, h: usize, strength: f32) -> Vec<Rgba8> {
    let c = over_white(src);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        let at = |x: isize, dy: isize| &c[((y as isize + dy).clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for (x, p) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let mut edge = 0f32;
            let mut e = [0f32; 3];
            for (k, ek) in e.iter_mut().enumerate() {
                let g = |dx: isize, dy: isize| at(xi + dx, dy)[k];
                let gx = (g(1, -1) + 2.0 * g(1, 0) + g(1, 1)) - (g(-1, -1) + 2.0 * g(-1, 0) + g(-1, 1));
                let gy = (g(-1, 1) + 2.0 * g(0, 1) + g(1, 1)) - (g(-1, -1) + 2.0 * g(0, -1) + g(1, -1));
                *ek = (gx * gx + gy * gy).sqrt().clamp(0.0, 1.0);
                edge = edge.max(*ek);
            }
            *p = edge_pixel(*p, e, edge, strength);
        }
    });
    out
}

/// Emboss: gray relief, 0.5 + amount × (the luminance `height` px toward
/// the light − away from it); the light comes from `angle` degrees.
pub fn emboss(src: &[Rgba8], w: usize, h: usize, angle: f32, height: f32, amount: f32) -> Vec<Rgba8> {
    let lum: Vec<f32> = over_white(src).iter().map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).collect();
    let (s, c) = angle.to_radians().sin_cos();
    let hgt = height.max(1.0);
    let (dx, dy) = ((c * hgt).round() as isize, (-s * hgt).round() as isize);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        let at = |x: isize, y: isize| lum[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for (x, p) in row.iter_mut().enumerate() {
            if p[3] == 0 { continue; }
            let (xi, yi) = (x as isize, y as isize);
            let g = (0.5 + amount * (at(xi + dx, yi + dy) - at(xi - dx, yi - dy))).clamp(0.0, 1.0);
            p[..3].fill(math::to_u8(g));
        }
    });
    out
}

/// Where a shadow lands: `distance` px away from the light at `angle`.
pub fn shadow_offset(angle: f32, distance: f32) -> (f32, f32) {
    let a = angle.to_radians();
    ((-a.cos() * distance).round(), (a.sin() * distance).round())
}

/// The shadow under one pixel: `sa` the shadow's alpha there (0..1).
pub fn shadow_pixel(p: Rgba8, sa: f32, color: [u8; 3]) -> Rgba8 {
    let a = math::from_u8(p[3]);
    let oa = a + sa * (1.0 - a);
    if oa <= 0.0 { return [0; 4]; }
    let mut o = [0u8; 4];
    for k in 0..3 { o[k] = ((p[k] as f32 * a + color[k] as f32 * sa * (1.0 - a)) / oa).round().clamp(0.0, 255.0) as u8; }
    o[3] = math::to_u8(oa.min(1.0));
    o
}

/// Drop shadow (Photoshop's layer style): the layer's alpha moved,
/// spread (grown by size × spread, the rest blurred), filled with `color`
/// at `opacity`, under the layer. Past the image's edge there is no
/// alpha to cast a shadow.
#[allow(clippy::too_many_arguments)]
pub fn drop_shadow(src: &[Rgba8], w: usize, h: usize, angle: f32, distance: f32, spread: f32, size: f32, opacity: f32, color: [u8; 3]) -> Vec<Rgba8> {
    let mut alpha: Vec<u16> = src.iter().map(|p| p[3] as u16 * 257).collect();
    let (grow, soft) = ((size * spread).round() as usize, size * (1.0 - spread));
    extreme::square(&mut alpha, w, h, grow, u16::max);
    blur::gaussian(&mut alpha, w, h, 0.4 * soft);
    let (dx, dy) = shadow_offset(angle, distance);
    let (dx, dy) = (dx as isize, dy as isize);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        for (x, p) in row.iter_mut().enumerate() {
            let (sx, sy) = (x as isize - dx, y as isize - dy);
            let inside = sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize;
            let sa = if inside { alpha[sy as usize * w + sx as usize] as f32 / 65535.0 * opacity } else { 0.0 };
            *p = shadow_pixel(*p, sa, color);
        }
    });
    out
}

//! Filters reading neighbors: Gaussian blur (W3C's three box blurs),
//! Unsharp Mask, Minimum / Maximum, Find Edges, Emboss, Drop Shadow
//! (after PhotoCraft, MIT, see NOTICE).

/// 16-bit premultiplied planes (65535 = 1.0), one per channel.
pub type Planes = [Vec<u16>; 4];

pub fn premultiply(src: &[u8]) -> Planes {
    let n = src.len() / 4;
    let mut planes: Planes = Default::default();
    for p in &mut planes { p.reserve_exact(n); }
    for px in src.chunks_exact(4) {
        let a = px[3] as u32;
        for c in 0..3 { planes[c].push(((px[c] as u32 * a * 257 + 127) / 255) as u16); }
        planes[3].push((a * 257) as u16);
    }
    planes
}

pub fn unpremultiply(planes: &Planes) -> Vec<u8> {
    let n = planes[3].len();
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let a = planes[3][i] as f32;
        if a < 129.0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let k = 255.0 / a;
        for p in &planes[..3] { out.push((p[i] as f32 * k + 0.5).min(255.0) as u8); }
        out.push((a / 257.0 + 0.5) as u8);
    }
    out
}

/// `f` on each plane, each on a thread of its own.
pub fn each_plane<T: Send>(planes: &mut [Vec<T>; 4], f: impl Fn(usize, &mut Vec<T>) + Sync) {
    std::thread::scope(|s| {
        for (i, p) in planes.iter_mut().enumerate() {
            let f = &f;
            s.spawn(move || f(i, p));
        }
    });
}

/// `f(y, row)` on every row of a `w`-wide RGBA image, rows shared out
/// between the cores.
pub fn par_rows(out: &mut [u8], w: usize, f: impl Fn(usize, &mut [u8]) + Sync) {
    let h = out.len() / (w * 4).max(1);
    if h == 0 { return; }
    let n = std::thread::available_parallelism().map_or(1, |n| n.get()).min(h);
    let per = h.div_ceil(n);
    std::thread::scope(|s| {
        for (k, chunk) in out.chunks_mut(per * w * 4).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, row) in chunk.chunks_exact_mut(w * 4).enumerate() { f(k * per + j, row); }
            });
        }
    });
}

pub fn blur_reach(sigma: f32) -> u32 { (3.0 * sigma).ceil() as u32 + 2 }

/// feGaussianBlur on one channel: three box blurs from σ = 2 (as the
/// spec), the Gaussian itself below.
pub fn gaussian_blur(px: &mut [u16], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 || w == 0 || h == 0 { return; }
    if sigma >= 2.0 {
        let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
        let boxes = if d % 2 == 1 { [(d / 2, d / 2); 3] } else { [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)] };
        let mut tmp = vec![0u16; w];
        for row in px.chunks_exact_mut(w) {
            for &(l, r) in &boxes { box_row(row, &mut tmp, l, r); }
        }
        let mut prev = vec![0u16; px.len()];
        for &(l, r) in &boxes { box_columns(px, &mut prev, w, h, l, r); }
        return;
    }
    let k = gaussian_kernel(sigma);
    let r = k.len() / 2;
    let mut tmp = vec![0u16; w];
    for row in px.chunks_exact_mut(w) {
        tmp.copy_from_slice(row);
        for (i, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() { s += kv * tmp[(i + j).saturating_sub(r).min(w - 1)] as f32; }
            *o = (s + 0.5) as u16;
        }
    }
    let prev = px.to_vec();
    let mut acc = vec![0f32; w];
    for y in 0..h {
        acc.fill(0.0);
        for (j, kv) in k.iter().enumerate() {
            let sy = (y + j).saturating_sub(r).min(h - 1);
            for (a, &v) in acc.iter_mut().zip(&prev[sy * w..(sy + 1) * w]) { *a += kv * v as f32; }
        }
        for (o, a) in px[y * w..(y + 1) * w].iter_mut().zip(&acc) { *o = (a + 0.5) as u16; }
    }
}

/// The normalized Gaussian, ±3σ.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
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

fn box_columns(px: &mut [u16], prev: &mut [u16], w: usize, h: usize, l: usize, r: usize) {
    prev.copy_from_slice(px);
    let inv = 1.0 / (l + 1 + r) as f32;
    let row = |y: isize| { let y = y.clamp(0, h as isize - 1) as usize; y * w..(y + 1) * w };
    let mut sum = vec![0u32; w];
    for j in -(l as isize)..=r as isize {
        for (s, &v) in sum.iter_mut().zip(&prev[row(j)]) { *s += v as u32; }
    }
    for y in 0..h {
        for (o, &s) in px[y * w..(y + 1) * w].iter_mut().zip(&sum) { *o = (s as f32 * inv + 0.5) as u16; }
        let (add, sub) = (row(y as isize + r as isize + 1), row(y as isize - l as isize));
        for ((s, &a), &b) in sum.iter_mut().zip(&prev[add]).zip(&prev[sub]) { *s = *s + a as u32 - b as u32; }
    }
}

pub fn blur_rgba(src: &[u8], w: usize, h: usize, sigma: f32) -> Vec<u8> {
    let mut planes = premultiply(src);
    each_plane(&mut planes, |_, p| gaussian_blur(p, w, h, sigma));
    unpremultiply(&planes)
}

/// Unsharp Mask: each color plus `amount` × its difference from a blurred
/// copy, where that difference reaches `threshold` levels. Straight
/// colors (the blur is premultiplied: no dark fringe at a line's edge),
/// alpha unchanged.
pub fn unsharp(src: &[u8], w: usize, h: usize, amount: f32, sigma: f32, threshold: f32) -> Vec<u8> {
    let blurred = blur_rgba(src, w, h, sigma);
    let (k, t) = (amount, threshold);
    let mut out = src.to_vec();
    for (o, b) in out.chunks_exact_mut(4).zip(blurred.chunks_exact(4)) {
        if o[3] == 0 || b[3] == 0 { continue; }
        for c in 0..3 {
            let d = o[c] as f32 - b[c] as f32;
            if d.abs() + 1e-3 >= t { o[c] = (o[c] as f32 + k * d).round().clamp(0.0, 255.0) as u8; }
        }
    }
    out
}

/// Thicken or thin dark lines (Minimum / Maximum), square or round, on
/// paper or line art.
pub fn lines(src: &[u8], w: usize, h: usize, radius: f32, round: bool, grow: bool) -> Vec<u8> {
    if radius < 0.5 || w == 0 || h == 0 { return src.to_vec(); }
    // A transparent pixel's color doesn't count: lightest for a min,
    // darkest for a max.
    let hidden = if grow { 255 } else { 0 };
    let mut planes: [Vec<u8>; 4] = Default::default();
    for px in src.chunks_exact(4) {
        for c in 0..3 { planes[c].push(if px[3] == 0 { hidden } else { px[c] }); }
        planes[3].push(px[3]);
    }
    each_plane(&mut planes, |c, p| {
        let max = (c == 3) == grow;
        let op: fn(u8, u8) -> u8 = if max { u8::max } else { u8::min };
        if round { extreme_disc(p, w, h, radius, op) } else { extreme_square(p, w, h, radius.round() as usize, op) }
    });
    let mut out = Vec::with_capacity(src.len());
    for i in 0..w * h {
        if planes[3][i] == 0 { out.extend_from_slice(&[0, 0, 0, 0]); } else { out.extend_from_slice(&[planes[0][i], planes[1][i], planes[2][i], planes[3][i]]); }
    }
    out
}

/// Max / min over a (2r + 1)² square: rows, then the rows of the
/// transposed plane (van Herk / Gil-Werman: constant work per value).
pub fn extreme_square<T: Copy + Default>(px: &mut [T], w: usize, h: usize, r: usize, op: fn(T, T) -> T) {
    if r == 0 { return; }
    let mut buf = Vec::new();
    for row in px.chunks_exact_mut(w) { extreme_line(row, r, r, op, &mut buf); }
    let mut t = transpose(px, w, h);
    for row in t.chunks_exact_mut(h) { extreme_line(row, r, r, op, &mut buf); }
    px.copy_from_slice(&transpose(&t, h, w));
}

/// Max / min over a disc of `radius`: each row of the disc is a window
/// along a row (corners and straight edges move by the same distance).
fn extreme_disc(px: &mut [u8], w: usize, h: usize, radius: f32, op: fn(u8, u8) -> u8) {
    let ri = radius.floor() as usize;
    let src = px.to_vec();
    let mut buf = Vec::new();
    let mut ext = vec![0u8; src.len()];
    // Rows dy and -dy share their window: each is worked out once.
    for d in 0..=ri {
        let hw = ((radius * radius - (d * d) as f32).max(0.0).sqrt() + 1e-4).floor() as usize;
        ext.copy_from_slice(&src);
        for row in ext.chunks_exact_mut(w) { extreme_line(row, hw, hw, op, &mut buf); }
        for y in 0..h {
            let row = &mut px[y * w..(y + 1) * w];
            for sy in [y.saturating_sub(d), (y + d).min(h - 1)] {
                let line = &ext[sy * w..(sy + 1) * w];
                for (o, &v) in row.iter_mut().zip(line) { *o = op(*o, v); }
            }
        }
    }
}

fn transpose<T: Copy + Default>(src: &[T], w: usize, h: usize) -> Vec<T> {
    let mut out = vec![T::default(); src.len()];
    const B: usize = 64;
    for by in (0..h).step_by(B) {
        for bx in (0..w).step_by(B) {
            for y in by..(by + B).min(h) {
                for x in bx..(bx + B).min(w) { out[x * h + y] = src[y * w + x]; }
            }
        }
    }
    out
}

/// Max (or min) over the window [i − l, i + r] along a line, edges
/// repeated: running values from each block's start (g) and end (hh).
fn extreme_line<T: Copy + Default>(line: &mut [T], l: usize, r: usize, op: fn(T, T) -> T, buf: &mut Vec<T>) {
    let n = line.len();
    let k = l + r + 1;
    if k <= 1 || n == 0 { return; }
    let len = (n + l + r).div_ceil(k) * k;
    buf.clear();
    buf.resize(len * 3, T::default());
    let (v, rest) = buf.split_at_mut(len);
    let (g, hh) = rest.split_at_mut(len);
    for (i, x) in v.iter_mut().enumerate() { *x = line[i.saturating_sub(l).min(n - 1)]; }
    for b in (0..len).step_by(k) {
        g[b] = v[b];
        for i in b + 1..b + k { g[i] = op(g[i - 1], v[i]); }
        hh[b + k - 1] = v[b + k - 1];
        for i in (b..b + k - 1).rev() { hh[i] = op(hh[i + 1], v[i]); }
    }
    for (i, o) in line.iter_mut().enumerate() { *o = op(hh[i], g[i + k - 1]); }
}

/// The colors over white, 0..1 (a transparent layer's lines then read as
/// dark, the empty parts as white).
fn over_white(src: &[u8]) -> Vec<[f32; 3]> {
    src.chunks_exact(4).map(|p| {
        let a = p[3] as f32 / 255.0;
        std::array::from_fn(|c| (p[c] as f32 / 255.0) * a + 1.0 - a)
    }).collect()
}

/// Find Edges: the Sobel gradient of each color, dark edges on white (a
/// 60-level step darkens by 240, as Photoshop's), mixed with the source
/// by `strength`. On a transparent layer the edges around the lines show.
pub fn find_edges(src: &[u8], w: usize, h: usize, strength: f32) -> Vec<u8> {
    let c = over_white(src);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        let at = |x: isize, dy: isize| &c[((y as isize + dy).clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for x in 0..w {
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
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3] as f32 / 255.0;
            // Where the layer is transparent the result is the edge itself.
            let na = a.max(edge * strength);
            if na <= 0.0 { continue; }
            for k in 0..3 {
                let base = if a > 0.0 { p[k] as f32 / 255.0 } else { 1.0 };
                let v = base + ((1.0 - e[k]) - base) * strength;
                p[k] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            p[3] = (na * 255.0).round() as u8;
        }
    });
    out
}

/// Emboss: gray relief, 0.5 + amount × (the luminance `height` px toward
/// the light − away from it); the light comes from `angle` degrees.
pub fn emboss(src: &[u8], w: usize, h: usize, angle: f32, height: f32, amount: f32) -> Vec<u8> {
    let lum: Vec<f32> = over_white(src).iter().map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]).collect();
    let (s, c) = angle.to_radians().sin_cos();
    let hgt = height.max(1.0);
    let (dx, dy) = ((c * hgt).round() as isize, (-s * hgt).round() as isize);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        let at = |x: isize, y: isize| lum[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for x in 0..w {
            let p = &mut row[x * 4..x * 4 + 4];
            if p[3] == 0 { continue; }
            let (xi, yi) = (x as isize, y as isize);
            let g = (0.5 + amount * (at(xi + dx, yi + dy) - at(xi - dx, yi - dy))).clamp(0.0, 1.0);
            let v = (g * 255.0).round() as u8;
            p[..3].fill(v);
        }
    });
    out
}

/// Where a shadow lands: `distance` px away from the light at `angle`.
pub fn shadow_offset(angle: f32, distance: f32) -> (f32, f32) {
    let a = angle.to_radians();
    ((-a.cos() * distance).round(), (a.sin() * distance).round())
}

/// Drop shadow (Photoshop's layer style): the layer's alpha moved,
/// spread (grown by size × spread, the rest blurred), filled with `color`
/// at `opacity`, under the layer.
#[allow(clippy::too_many_arguments)]
pub fn drop_shadow(src: &[u8], w: usize, h: usize, angle: f32, distance: f32, spread: f32, size: f32, opacity: f32, color: [u8; 3]) -> Vec<u8> {
    let mut alpha: Vec<u16> = src.chunks_exact(4).map(|p| p[3] as u16 * 257).collect();
    let (grow, soft) = ((size * spread).round() as usize, size * (1.0 - spread));
    extreme_square(&mut alpha, w, h, grow, u16::max);
    gaussian_blur(&mut alpha, w, h, 0.4 * soft);
    let (dx, dy) = shadow_offset(angle, distance);
    let (dx, dy) = (dx as isize, dy as isize);
    let mut out = src.to_vec();
    par_rows(&mut out, w, |y, row| {
        for x in 0..w {
            let (sx, sy) = (x as isize - dx, y as isize - dy);
            let sa = if sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize { alpha[sy as usize * w + sx as usize] as f32 / 65535.0 * opacity } else { 0.0 };
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3] as f32 / 255.0;
            let oa = a + sa * (1.0 - a);
            if oa <= 0.0 { p.copy_from_slice(&[0, 0, 0, 0]); continue; }
            for k in 0..3 { p[k] = ((p[k] as f32 * a + color[k] as f32 * sa * (1.0 - a)) / oa).round().clamp(0.0, 255.0) as u8; }
            p[3] = (oa * 255.0).round() as u8;
        }
    });
    out
}

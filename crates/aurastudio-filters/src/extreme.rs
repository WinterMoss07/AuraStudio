//! Max / min over a square or a disc (van Herk / Gil-Werman: constant
//! work per value whatever the radius): Thicken / Thin lines, the drop
//! shadow's spread, a selection's grow and shrink.

/// Max / min over a (2r + 1)² square, edges repeated: along the rows,
/// then along the columns whole rows at a time (row-major, no transpose).
pub fn square<T: Copy + Default>(px: &mut [T], w: usize, h: usize, r: usize, op: fn(T, T) -> T) {
    if r == 0 || w == 0 || h == 0 || px.len() != w * h { return; }
    let mut buf = Vec::new();
    for row in px.chunks_exact_mut(w) { line(row, r, r, op, &mut buf); }
    columns(px, w, h, r, op);
}

/// Max / min over a disc of `radius`: each row of the disc is a window
/// along a row (rows dy and −dy share theirs).
pub fn disc<T: Copy + Default>(px: &mut [T], w: usize, h: usize, radius: f32, op: fn(T, T) -> T) {
    if radius < 0.5 || w == 0 || h == 0 || px.len() != w * h { return; }
    let ri = radius.floor() as usize;
    let src = px.to_vec();
    let mut buf = Vec::new();
    let mut ext = vec![T::default(); src.len()];
    for d in 0..=ri {
        let hw = ((radius * radius - (d * d) as f32).max(0.0).sqrt() + 1e-4).floor() as usize;
        ext.copy_from_slice(&src);
        for row in ext.chunks_exact_mut(w) { line(row, hw, hw, op, &mut buf); }
        for y in 0..h {
            let row = &mut px[y * w..(y + 1) * w];
            for sy in [y.saturating_sub(d), (y + d).min(h - 1)] {
                for (o, &v) in row.iter_mut().zip(&ext[sy * w..(sy + 1) * w]) { *o = op(*o, v); }
            }
        }
    }
}

/// Max (or min) over the window [i − l, i + r] along a line, edges
/// repeated: running values from each block's start (g) and end (hh).
fn line<T: Copy + Default>(line: &mut [T], l: usize, r: usize, op: fn(T, T) -> T, buf: &mut Vec<T>) {
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

/// The same along the columns, window ±r: the line algorithm with whole
/// rows as its values (each step a row-long loop the compiler vectorizes).
fn columns<T: Copy + Default>(px: &mut [T], w: usize, h: usize, r: usize, op: fn(T, T) -> T) {
    let k = 2 * r + 1;
    let len = (h + 2 * r).div_ceil(k) * k;
    let v = |i: usize| { let y = i.saturating_sub(r).min(h - 1); y * w..(y + 1) * w };
    let src = px.to_vec();
    let mut g = vec![T::default(); len * w];
    let mut hh = vec![T::default(); len * w];
    for b in (0..len).step_by(k) {
        g[b * w..(b + 1) * w].copy_from_slice(&src[v(b)]);
        for i in b + 1..b + k {
            let (done, cur) = g.split_at_mut(i * w);
            for ((o, &p), &s) in cur[..w].iter_mut().zip(&done[(i - 1) * w..]).zip(&src[v(i)]) { *o = op(p, s); }
        }
        let e = b + k - 1;
        hh[e * w..(e + 1) * w].copy_from_slice(&src[v(e)]);
        for i in (b..e).rev() {
            let (cur, after) = hh.split_at_mut((i + 1) * w);
            for ((o, &n), &s) in cur[i * w..].iter_mut().zip(&after[..w]).zip(&src[v(i)]) { *o = op(n, s); }
        }
    }
    for y in 0..h {
        let (a, b) = (&hh[y * w..(y + 1) * w], &g[(y + k - 1) * w..(y + k) * w]);
        for ((o, &x), &z) in px[y * w..(y + 1) * w].iter_mut().zip(a).zip(b) { *o = op(x, z); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The square against the obvious double loop.
    #[test]
    fn square_matches_brute_force() {
        let (w, h) = (23usize, 17usize);
        let src: Vec<u8> = (0..w * h).map(|i| ((i * 7919) % 251) as u8).collect();
        for r in [1usize, 2, 5, 9] {
            let mut got = src.clone();
            square(&mut got, w, h, r, u8::max);
            for y in 0..h {
                for x in 0..w {
                    let mut m = 0u8;
                    for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                        for xx in x.saturating_sub(r)..=(x + r).min(w - 1) { m = m.max(src[yy * w + xx]); }
                    }
                    assert_eq!(got[y * w + x], m, "r {r} at {x},{y}");
                }
            }
        }
    }
}

//! Drawing a transformed source over destination pixels: bands of rows on
//! all cores; a quad by its inverse map, a bent grid by fine triangles.

use aurastudio_core::blend::{to_premul, to_straight_u8};
use aurastudio_core::{Point, Rect, Rgba8};
use rayon::prelude::*;

use crate::homography::Homography;
use crate::mesh::{Mesh, Warp};
use crate::source::{Interp, Source};

/// Premultiplied `s` over straight `d` (Normal), rounded once.
#[inline]
fn over(d: &mut Rgba8, s: [f32; 4]) {
    // Opaque, or over nothing: the sample as it is.
    if s[3] >= 0.9995 || d[3] == 0 { *d = to_straight_u8(s); return; }
    let b = to_premul(*d);
    let k = 1.0 - s[3];
    *d = to_straight_u8(std::array::from_fn(|c| s[c] + b[c] * k));
}

/// Paint `src` transformed by `warp` over `dst` (straight RGBA rows of
/// `rect`, canvas pixels).
pub fn render(src: &Source, warp: &Warp, interp: Interp, rect: Rect, dst: &mut [Rgba8]) {
    if rect.is_empty() || dst.len() != rect.area() as usize || src.rect().is_empty() { return; }
    aurastudio_core::zone!("transform render");
    match warp.bent() {
        Some(m) => render_mesh(src, warp, m, interp, rect, dst),
        None => render_quad(src, warp, interp, rect, dst),
    }
}

fn render_quad(src: &Source, warp: &Warp, interp: Interp, rect: Rect, dst: &mut [Rgba8]) {
    let s = src.rect();
    let Some(h) = Homography::rect_to_quad(s, warp.quad) else { return };
    let Some(inv) = h.inverse() else { return };
    let m = inv.0;
    // Move, scale, turn, skew: no division per pixel.
    let affine = inv.is_affine();
    let rw = rect.w() as usize;
    let (sx0, sy0, sx1, sy1) = (s.x0 as f64 - 2.0, s.y0 as f64 - 2.0, s.x1 as f64 + 2.0, s.y1 as f64 + 2.0);
    const B: usize = 16;
    dst.par_chunks_mut(rw * B).enumerate().for_each(|(bi, band)| {
        let rows = band.len() / rw;
        let yb = (rect.y0 as i64 + (bi * B) as i64) as f64;
        for bx in (0..rw).step_by(B) {
            let bw = B.min(rw - bx);
            let x0 = (rect.x0 as i64 + bx as i64) as f64;
            // A block whose corners all map off the source on one side shows
            // nothing of it (the map keeps straight lines straight).
            let cs = [(x0, yb), (x0 + bw as f64, yb), (x0, yb + rows as f64), (x0 + bw as f64, yb + rows as f64)].map(|(x, y)| inv.apply(Point::new(x, y)));
            if cs.iter().all(|c| c.x < sx0) || cs.iter().all(|c| c.x > sx1) || cs.iter().all(|c| c.y < sy0) || cs.iter().all(|c| c.y > sy1) { continue; }
            // The scale and footprint of this block, at its center.
            let c = Point::new(x0 + bw as f64 / 2.0, yb + rows as f64 / 2.0);
            let o = inv.apply(c);
            let (ju, jv) = (inv.apply(c + Point::new(1.0, 0.0)) - o, inv.apply(c + Point::new(0.0, 1.0)) - o);
            let scale = ju.cross(jv).abs().sqrt();
            let foot = (ju.x.abs() + jv.x.abs(), ju.y.abs() + jv.y.abs());
            let lod = src.level(scale, interp);
            for y in 0..rows {
                // Along the row the homogeneous point moves by fixed steps.
                let (px, py) = (x0 + 0.5, yb + y as f64 + 0.5);
                let (mut hx, mut hy, mut hw) = (m[0] * px + m[1] * py + m[2], m[3] * px + m[4] * py + m[5], m[6] * px + m[7] * py + m[8]);
                for x in bx..bx + bw {
                    let (u, v) = if affine { (hx, hy) } else { let r = 1.0 / hw; (hx * r, hy * r) };
                    hx += m[0];
                    hy += m[3];
                    hw += m[6];
                    if !(u.is_finite() && v.is_finite()) { continue; }
                    if let Some(c) = src.at(u, v, &lod, foot, interp) { over(&mut band[y * rw + x], c); }
                }
            }
        }
    });
}

/// A grid point: where it lands, where it comes from in the source.
#[derive(Clone, Copy)]
struct Vert { dst: Point, src: Point }

fn render_mesh(src: &Source, warp: &Warp, m: &Mesh, interp: Interp, rect: Rect, dst: &mut [Rgba8]) {
    let s = src.rect();
    let Some(h) = Homography::square_to_quad(warp.quad) else { return };
    let (sw, sh) = (s.w() as f64, s.h() as f64);
    let Some(b) = warp.bounds(s) else { return };
    let rw = rect.w() as usize;
    // Fine triangles: about 10 destination pixels a side (a part far
    // past what is drawn needs no detail).
    let shown = rect.w().max(rect.h()) as f64 * 4.0;
    let side = (b[2] - b[0]).max(b[3] - b[1]).min(shown);
    let n = ((side / 10.0).ceil() as usize).clamp(m.cols.max(m.rows) * 4, 512).max(2);
    let (nu, nv) = (n, (n as f64 * sh / sw).ceil().clamp(2.0, 768.0) as usize);
    let mut verts = Vec::with_capacity((nu + 1) * (nv + 1));
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f64 / nu as f64, j as f64 / nv as f64);
            verts.push(Vert { dst: h.apply(m.eval(Point::new(u, v))), src: Point::new(s.x0 as f64 + u * sw, s.y0 as f64 + v * sh) });
        }
    }
    let at = |i: usize, j: usize| verts[j * (nu + 1) + i];
    let mut tris = Vec::with_capacity(nu * nv * 2);
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    const B: usize = 32;
    // Triangles by the bands they cross, as one flat index (offsets into
    // a single list).
    let nb = (rect.h() as usize).div_ceil(B);
    let span = |t: &[Vert; 3]| -> Option<(usize, usize)> {
        let (y0, y1) = (t.iter().map(|v| v.dst.y).fold(f64::MAX, f64::min), t.iter().map(|v| v.dst.y).fold(f64::MIN, f64::max));
        let (x0, x1) = (t.iter().map(|v| v.dst.x).fold(f64::MAX, f64::min), t.iter().map(|v| v.dst.x).fold(f64::MIN, f64::max));
        if !(y0.is_finite() && y1.is_finite()) { return None; }
        if x1 < rect.x0 as f64 || x0 > rect.x1 as f64 || y1 < rect.y0 as f64 || y0 > rect.y1 as f64 { return None; }
        let a = ((y0 - rect.y0 as f64).floor().max(0.0) as usize / B).min(nb);
        let z = ((y1 - rect.y0 as f64).ceil().max(0.0) as usize / B + 1).min(nb);
        Some((a, z))
    };
    let mut start = vec![0usize; nb + 1];
    let spans: Vec<Option<(usize, usize)>> = tris.iter().map(span).collect();
    for (a, z) in spans.iter().flatten() { for b in *a..*z { start[b + 1] += 1; } }
    for b in 0..nb { start[b + 1] += start[b]; }
    let mut index = vec![0usize; start[nb]];
    let mut fill = start.clone();
    for (k, sp) in spans.iter().enumerate() {
        if let Some((a, z)) = sp { for b in *a..*z { index[fill[b]] = k; fill[b] += 1; } }
    }
    dst.par_chunks_mut(rw * B).enumerate().for_each_init(Vec::new, |samples: &mut Vec<Option<[f32; 4]>>, (bi, band)| {
        let rows = band.len() / rw;
        // Samples first (a fold covers a pixel twice: the later wins), then
        // over the pixels.
        samples.clear();
        samples.resize(rw * rows, None);
        let by0 = (rect.y0 as i64 + (bi * B) as i64) as f64;
        for &k in &index[start[bi]..start[bi + 1]] {
            let [t0, t1, t2] = tris[k];
            let (p0, p1, p2) = (t0.dst, t1.dst, t2.dst);
            let det = (p1 - p0).cross(p2 - p0);
            if det.abs() < 1e-9 { continue; }
            // Destination → source, affine on the triangle.
            let ix = [(p2.y - p0.y) / det, -(p2.x - p0.x) / det, -(p1.y - p0.y) / det, (p1.x - p0.x) / det];
            let (e1, e2) = (t1.src - t0.src, t2.src - t0.src);
            let du = (e1.x * ix[0] + e2.x * ix[2], e1.x * ix[1] + e2.x * ix[3]);
            let dv = (e1.y * ix[0] + e2.y * ix[2], e1.y * ix[1] + e2.y * ix[3]);
            let scale = (du.0 * dv.1 - du.1 * dv.0).abs().sqrt();
            let foot = (du.0.abs() + du.1.abs(), dv.0.abs() + dv.1.abs());
            let lod = src.level(scale, interp);
            let (minx, maxx) = (p0.x.min(p1.x).min(p2.x), p0.x.max(p1.x).max(p2.x));
            let (miny, maxy) = (p0.y.min(p1.y).min(p2.y), p0.y.max(p1.y).max(p2.y));
            let ya = ((miny - by0 - 0.5).ceil().max(0.0) as usize).min(rows);
            let yb = ((maxy - by0 - 0.5).floor() as i64 + 1).clamp(0, rows as i64) as usize;
            let xa = ((minx - rect.x0 as f64 - 0.5).ceil().max(0.0) as usize).min(rw);
            let xb = ((maxx - rect.x0 as f64 - 0.5).floor() as i64 + 1).clamp(0, rw as i64) as usize;
            for y in ya..yb {
                let py = by0 + y as f64 + 0.5;
                for x in xa..xb {
                    let px = rect.x0 as f64 + x as f64 + 0.5;
                    let (ex, ey) = (px - p0.x, py - p0.y);
                    let l1 = ix[0] * ex + ix[1] * ey;
                    let l2 = ix[2] * ex + ix[3] * ey;
                    if l1 < -1e-9 || l2 < -1e-9 || l1 + l2 > 1.0 + 1e-9 { continue; }
                    let p = t0.src + e1 * l1 + e2 * l2;
                    if let Some(c) = src.at(p.x, p.y, &lod, foot, interp) { samples[y * rw + x] = Some(c); }
                }
            }
        }
        for (d, s) in band.iter_mut().zip(samples.iter()) {
            if let Some(c) = s { over(d, *c); }
        }
    });
}

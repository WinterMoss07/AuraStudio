//! Ways to make a selection: a rectangle, a coverage buffer, a closed
//! outline (lasso, polygon), an ellipse, a predicate, a layer's alpha.

use aurastudio_core::{math, Point, Rect};
use aurastudio_raster::{Image, Mask};

use crate::{finite, Selection, Space};

/// How far past the frame a drawn shape may reach (the canvas is
/// unbounded, a stray point at 10⁹ must not fill memory or take minutes).
const REACH: i32 = 4096;

/// Rows rasterized at once (then written to the mask).
const BAND: usize = 64;

impl Selection {
    /// A rectangle; `None` if it is empty.
    pub fn rect(space: Space, r: Rect) -> Option<Self> {
        let r = r.intersect(&space.frame().grown(REACH));
        if r.is_empty() { return None; }
        let mut m = space.mask();
        m.fill_rect(r, 255);
        Self::finish(m, true)
    }

    /// `mask` (rows of `r`, one byte per pixel) as a selection. `None` if
    /// nothing is selected.
    pub fn from_mask(space: Space, r: Rect, mask: &[u8]) -> Option<Self> {
        if r.is_empty() || mask.len() != r.area() as usize { return None; }
        let mut m = space.mask();
        m.write(r, mask);
        Self::of(m)
    }

    /// Inside a closed outline (lasso, polygon), even-odd. Anti-aliased:
    /// each pixel by how much of it is inside (exact across, 4 rows per
    /// pixel down); else a pixel counts when its center is inside.
    pub fn polygon(space: Space, points: &[Point], aa: bool) -> Option<Self> {
        if points.len() < 3 || !finite(points) { return None; }
        let (mut fx0, mut fy0, mut fx1, mut fy1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in points { fx0 = fx0.min(p.x); fy0 = fy0.min(p.y); fx1 = fx1.max(p.x); fy1 = fy1.max(p.y); }
        let r = Rect::around(fx0, fy0, fx1, fy1).intersect(&space.frame().grown(REACH));
        if r.is_empty() { return None; }
        let (w, h) = (r.w() as usize, r.h() as usize);
        let mut m = space.mask();
        let mut acc = vec![0f32; w * BAND];
        let sub = if aa { 4 } else { 1 };
        let pts: Vec<(f32, f32)> = points.iter().map(|p| (p.x as f32, p.y as f32)).collect();
        // Edges by their top, crossing the rows they span.
        let mut edges: Vec<((f32, f32), (f32, f32))> = (0..pts.len()).filter_map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            match a.1.total_cmp(&b.1) {
                std::cmp::Ordering::Less => Some((a, b)),
                std::cmp::Ordering::Greater => Some((b, a)),
                _ => None,
            }
        }).collect();
        edges.sort_by(|p, q| p.0 .1.total_cmp(&q.0 .1));
        let mut active: Vec<usize> = Vec::new();
        let mut next = 0;
        let mut xs: Vec<f32> = Vec::new();
        for py in 0..h {
            let band_row = py % BAND;
            if band_row == 0 { acc.fill(0.0); }
            let row = &mut acc[band_row * w..(band_row + 1) * w];
            for s in 0..sub {
                let y = r.y0 as f32 + py as f32 + (s as f32 + 0.5) / sub as f32;
                while next < edges.len() && edges[next].0 .1 <= y { active.push(next); next += 1; }
                active.retain(|&e| edges[e].1 .1 > y);
                if active.is_empty() { continue; }
                xs.clear();
                xs.extend(active.iter().map(|&e| { let (a, b) = edges[e]; a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0) }));
                xs.sort_by(f32::total_cmp);
                for span in xs.chunks_exact(2) {
                    let (x0, x1) = ((span[0] - r.x0 as f32).max(0.0), (span[1] - r.x0 as f32).min(w as f32));
                    if x1 <= x0 { continue; }
                    let (c0, c1) = (x0.floor() as usize, (x1.ceil() as usize).min(w));
                    for (cx, v) in row.iter_mut().enumerate().take(c1).skip(c0) {
                        let c = if aa {
                            ((cx + 1) as f32).min(x1) - (cx as f32).max(x0)
                        } else {
                            let m = cx as f32 + 0.5;
                            if m >= x0 && m < x1 { 1.0 } else { 0.0 }
                        };
                        *v += c.max(0.0) / sub as f32;
                    }
                }
            }
            if band_row == BAND - 1 || py == h - 1 {
                let rows = band_row + 1;
                let y0 = r.y0 + (py - band_row) as i32;
                let cov: Vec<u8> = acc[..rows * w].iter().map(|v| math::to_u8(v.clamp(0.0, 1.0))).collect();
                m.write(Rect::new(r.x0, y0, r.x1, y0 + rows as i32), &cov);
            }
        }
        Self::of(m)
    }

    /// An ellipse in the box `[x0, y0, x1, y1]` (canvas pixels).
    pub fn ellipse(space: Space, b: [f64; 4], aa: bool) -> Option<Self> {
        let (cx, cy, rx, ry) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, (b[2] - b[0]).abs() / 2.0, (b[3] - b[1]).abs() / 2.0);
        if !(rx > 0.0 && ry > 0.0 && cx.is_finite() && cy.is_finite()) { return None; }
        // Fine enough that the flat sides stay under a tenth of a pixel.
        let n = ((std::f64::consts::TAU * (rx.max(ry) / 0.2).sqrt()).ceil() as usize).clamp(16, 4096);
        let pts: Vec<Point> = (0..n).map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            Point::new(cx + rx * a.cos(), cy + ry * a.sin())
        }).collect();
        Self::polygon(space, &pts, aa)
    }

    /// The pixels of `bbox` for which `inside` holds (the brushes' "stay
    /// inside the lines": a fill area).
    pub fn from_predicate(space: Space, bbox: Rect, inside: impl Fn(i32, i32) -> bool) -> Option<Self> {
        if bbox.is_empty() { return None; }
        aurastudio_core::zone!("selection from predicate");
        let mut m = space.mask();
        for c in m.chunks(bbox, 256) {
            let data: Vec<u8> = (c.y0..c.y1).flat_map(|y| (c.x0..c.x1).map(move |x| (x, y))).map(|(x, y)| if inside(x, y) { 255 } else { 0 }).collect();
            m.write(c, &data);
        }
        Self::of(m)
    }

    /// The painted pixels of a layer, by their alpha (Krita's "select
    /// opaque"); a paper (visible fill) selects its frame.
    pub fn from_alpha(img: &Image) -> Option<Self> {
        let r = img.pixel_bounds()?;
        let mut m: Mask = Mask::new(img.width(), img.height(), img.tile_size(), 0);
        for c in img.chunks(r, 256) {
            match img.uniform_in(c) {
                Some(p) => m.fill_rect(c, p[3]),
                None => m.write(c, &img.read(c).iter().map(|p| p[3]).collect::<Vec<u8>>()),
            }
        }
        Self::of(m)
    }
}

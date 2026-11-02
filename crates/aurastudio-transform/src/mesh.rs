//! The grid bending the source before the quad, and the whole warp.

use aurastudio_core::{Point, Rect};

use crate::homography::{corners, Homography, Quad};

/// A grid bending the source before the quad (CSP's mesh): `cols` ×
/// `rows` cells, control points in the source's unit square. A point
/// moves only the four cells around it: the points around stay where they
/// are, and so does what is under them (each point's move fades out over
/// its cells, smoothly: 3t² - 2t³).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub cols: usize,
    pub rows: usize,
    /// (cols + 1) × (rows + 1), row by row.
    pub pts:  Vec<Point>,
}

impl Mesh {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.clamp(1, 16), rows.clamp(1, 16));
        let pts = (0..=rows).flat_map(|j| (0..=cols).map(move |i| Point::new(i as f64 / cols as f64, j as f64 / rows as f64))).collect();
        Self { cols, rows, pts }
    }

    pub fn is_identity(&self) -> bool { *self == Mesh::new(self.cols, self.rows) }

    pub fn at(&self, i: usize, j: usize) -> Point { self.pts[j * (self.cols + 1) + i] }

    /// How far point (i, j) moved from its place.
    pub(crate) fn moved(&self, i: usize, j: usize) -> Point {
        self.at(i, j) - Point::new(i as f64 / self.cols as f64, j as f64 / self.rows as f64)
    }

    /// Where unit point `p` of the source goes (still unit space). Past
    /// the edges, as the nearest edge moved.
    pub fn eval(&self, p: Point) -> Point {
        let (x, y) = (p.x * self.cols as f64, p.y * self.rows as f64);
        let i = (x.floor() as isize).clamp(0, self.cols as isize - 1) as usize;
        let j = (y.floor() as isize).clamp(0, self.rows as isize - 1) as usize;
        let smooth = |t: f64| { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) };
        let (su, sv) = (smooth(x - i as f64), smooth(y - j as f64));
        let mut out = p;
        for (di, dj, w) in [(0, 0, (1.0 - su) * (1.0 - sv)), (1, 0, su * (1.0 - sv)), (0, 1, (1.0 - su) * sv), (1, 1, su * sv)] {
            out = out + self.moved(i + di, j + dj) * w;
        }
        out
    }
}

/// A whole transform of the source rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    /// Where the source's corners go, clockwise from top-left.
    pub quad: Quad,
    pub mesh: Option<Mesh>,
}

/// A warp ready to map many points of one source rectangle (its
/// homography worked out once).
pub struct WarpMap<'a> {
    h:    Homography,
    src:  Rect,
    mesh: Option<&'a Mesh>,
}

impl WarpMap<'_> {
    /// Source canvas point → destination.
    #[inline]
    pub fn map(&self, p: Point) -> Point {
        let (sw, sh) = (self.src.w() as f64, self.src.h() as f64);
        let mut u = Point::new((p.x - self.src.x0 as f64) / sw, (p.y - self.src.y0 as f64) / sh);
        if let Some(m) = self.mesh { u = m.eval(u); }
        self.h.apply(u)
    }
}

impl Warp {
    pub fn identity(src: Rect) -> Self { Self { quad: corners(src), mesh: None } }

    /// The mesh, if it bends anything.
    pub fn bent(&self) -> Option<&Mesh> { self.mesh.as_ref().filter(|m| !m.is_identity()) }

    /// For mapping points of `src`; `None` if the quad is degenerate.
    pub fn mapper(&self, src: Rect) -> Option<WarpMap<'_>> {
        if src.is_empty() { return None; }
        Some(WarpMap { h: Homography::square_to_quad(self.quad)?, src, mesh: self.mesh.as_ref() })
    }

    /// Source canvas point → destination, `None` if degenerate.
    pub fn map(&self, src: Rect, p: Point) -> Option<Point> { Some(self.mapper(src)?.map(p)) }

    /// Bounds of the result (a pixel more for the filter), as floats.
    pub fn bounds(&self, src: Rect) -> Option<[f64; 4]> {
        let m = self.mapper(src)?;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut add = |p: Point| { x0 = x0.min(p.x); y0 = y0.min(p.y); x1 = x1.max(p.x); y1 = y1.max(p.y); };
        let n = if self.mesh.is_some() { 32 } else { 1 };
        let (sw, sh) = (src.w() as f64, src.h() as f64);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            for (u, v) in [(t, 0.0), (t, 1.0), (0.0, t), (1.0, t)] {
                add(m.map(Point::new(src.x0 as f64 + u * sw, src.y0 as f64 + v * sh)));
            }
        }
        // A bent grid: each cell's points stay in the cell's box grown by
        // its corners' moves (the quad maps a box to a convex quad).
        if let Some(g) = &self.mesh {
            for j in 0..g.rows {
                for i in 0..g.cols {
                    let ds = [g.moved(i, j), g.moved(i + 1, j), g.moved(i, j + 1), g.moved(i + 1, j + 1)];
                    let lo = Point::new(ds.iter().map(|d| d.x).fold(f64::MAX, f64::min), ds.iter().map(|d| d.y).fold(f64::MAX, f64::min));
                    let hi = Point::new(ds.iter().map(|d| d.x).fold(f64::MIN, f64::max), ds.iter().map(|d| d.y).fold(f64::MIN, f64::max));
                    let (u0, v0) = (i as f64 / g.cols as f64 + lo.x, j as f64 / g.rows as f64 + lo.y);
                    let (u1, v1) = ((i + 1) as f64 / g.cols as f64 + hi.x, (j + 1) as f64 / g.rows as f64 + hi.y);
                    for (u, v) in [(u0, v0), (u1, v0), (u0, v1), (u1, v1)] { add(m.h.apply(Point::new(u, v))); }
                }
            }
        }
        let ok = [x0, y0, x1, y1].iter().all(|v| v.is_finite());
        ok.then_some([x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0])
    }

    /// The whole pixels the result may touch (unbounded canvas: not
    /// clipped; intersect with what is shown or stored).
    pub fn dest_rect(&self, src: Rect) -> Option<Rect> {
        let [x0, y0, x1, y1] = self.bounds(src)?;
        let lim = i32::MAX as f64 / 2.0;
        let c = |v: f64| v.clamp(-lim, lim) as i32;
        let r = Rect::new(c(x0.floor()), c(y0.floor()), c(x1.ceil()), c(y1.ceil()));
        (!r.is_empty()).then_some(r)
    }
}

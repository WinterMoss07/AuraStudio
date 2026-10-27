//! Selections: 8-bit coverage in 64 px tiles (empty and full tiles cost
//! nothing), the shapes and commands that make them, and their outline.

use std::sync::atomic::{AtomicU64, Ordering};

use aurastudio_core::{Rect, TILE};

const T: usize = TILE as usize;
const TN: usize = T * T;

/// How a new selection meets the one there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

#[derive(Clone, Debug, PartialEq)]
enum Cov {
    Empty,
    Full,
    /// Past the canvas edge: always 0.
    Part(Box<[u8; TN]>),
}

/// A boundary piece of the outline, canvas pixels (horizontal or vertical).
pub type Segment = [(f32, f32); 2];

#[derive(Clone, Debug)]
pub struct Selection {
    width:   u32,
    height:  u32,
    cols:    u32,
    tiles:   Vec<Cov>,
    /// Smallest rectangle holding every selected pixel.
    bbox:    Rect,
    /// Exactly its bounding box, fully selected.
    is_rect: bool,
    /// New for every new content (the outline cache's key).
    stamp:   u64,
}

impl PartialEq for Selection {
    fn eq(&self, o: &Self) -> bool {
        (self.width, self.height, self.bbox) == (o.width, o.height, o.bbox) && self.tiles == o.tiles
    }
}

fn next_stamp() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Straight blend of `a` toward `b` by `t` (0..255).
fn mix(a: u8, b: u8, t: u8) -> u8 {
    ((a as u32 * (255 - t as u32) + b as u32 * t as u32 + 127) / 255) as u8
}

impl Selection {
    fn empty(width: u32, height: u32) -> Self {
        let cols = width.div_ceil(TILE);
        let n = (cols * height.div_ceil(TILE)) as usize;
        Self { width, height, cols, tiles: vec![Cov::Empty; n], bbox: (0, 0, 0, 0), is_rect: false, stamp: next_stamp() }
    }

    /// Pixels of tile `i` inside the canvas.
    fn extent(&self, i: usize) -> (usize, usize) {
        let (tx, ty) = (i as u32 % self.cols, i as u32 / self.cols);
        (((self.width - tx * TILE).min(TILE)) as usize, ((self.height - ty * TILE).min(TILE)) as usize)
    }

    /// A partial tile becomes empty / full when it is.
    fn normalize(&mut self, i: usize) {
        let (tw, th) = self.extent(i);
        let Cov::Part(p) = &mut self.tiles[i] else { return };
        let (mut any, mut all) = (false, true);
        for y in 0..T {
            for x in 0..T {
                let v = &mut p[y * T + x];
                if x >= tw || y >= th { *v = 0; continue; }
                any |= *v > 0;
                all &= *v == 255;
            }
        }
        if !any { self.tiles[i] = Cov::Empty; } else if all { self.tiles[i] = Cov::Full; }
    }

    /// Bounds of the selected pixels, from the tiles.
    fn update_bbox(&mut self) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, t) in self.tiles.iter().enumerate() {
            let (tx, ty) = (i as u32 % self.cols * TILE, i as u32 / self.cols * TILE);
            let (tw, th) = self.extent(i);
            let r = match t {
                Cov::Empty => continue,
                Cov::Full => (0, 0, tw, th),
                Cov::Part(p) => {
                    let (mut a, mut b, mut c, mut d) = (usize::MAX, usize::MAX, 0, 0);
                    for y in 0..th {
                        for x in 0..tw {
                            if p[y * T + x] > 0 { a = a.min(x); b = b.min(y); c = c.max(x + 1); d = d.max(y + 1); }
                        }
                    }
                    if a == usize::MAX { continue; }
                    (a, b, c, d)
                }
            };
            x0 = x0.min(tx + r.0 as u32);
            y0 = y0.min(ty + r.1 as u32);
            x1 = x1.max(tx + r.2 as u32);
            y1 = y1.max(ty + r.3 as u32);
        }
        self.bbox = if x0 == u32::MAX { (0, 0, 0, 0) } else { (x0, y0, x1, y1) };
    }

    /// Finish a selection made or changed: None if nothing is selected.
    fn done(mut self) -> Option<Self> {
        for i in 0..self.tiles.len() { self.normalize(i); }
        self.update_bbox();
        self.stamp = next_stamp();
        (self.bbox.2 > self.bbox.0).then_some(self)
    }

    /// The whole canvas.
    pub fn all(width: u32, height: u32) -> Self {
        let mut s = Self::empty(width.max(1), height.max(1));
        s.tiles.fill(Cov::Full);
        s.bbox = (0, 0, s.width, s.height);
        s.is_rect = true;
        s
    }

    /// A rectangle (clipped to the canvas); None if nothing is left.
    pub fn rect(width: u32, height: u32, r: Rect) -> Option<Self> {
        let (x0, y0, x1, y1) = (r.0.min(width), r.1.min(height), r.2.min(width), r.3.min(height));
        if x0 >= x1 || y0 >= y1 { return None; }
        let mut s = Self::empty(width, height);
        for ty in y0 / TILE..y1.div_ceil(TILE) {
            for tx in x0 / TILE..x1.div_ceil(TILE) {
                let (bx, by) = (tx * TILE, ty * TILE);
                let i = (ty * s.cols + tx) as usize;
                let (tw, th) = s.extent(i);
                let (a, b) = (x0.max(bx) - bx, y0.max(by) - by);
                let (c, d) = (x1.min(bx + tw as u32) - bx, y1.min(by + th as u32) - by);
                if a == 0 && b == 0 && c as usize == tw && d as usize == th {
                    s.tiles[i] = Cov::Full;
                    continue;
                }
                let mut p = Box::new([0u8; TN]);
                for y in b..d { p[y as usize * T + a as usize..y as usize * T + c as usize].fill(255); }
                s.tiles[i] = Cov::Part(p);
            }
        }
        s.bbox = (x0, y0, x1, y1);
        s.is_rect = true;
        Some(s)
    }

    /// `mask` (rows of `r`, one byte per pixel) as a selection, clipped to
    /// the canvas. None if nothing is selected.
    pub fn from_mask(width: u32, height: u32, r: Rect, mask: &[u8]) -> Option<Self> {
        let rw = r.2.saturating_sub(r.0) as usize;
        if rw == 0 || mask.len() != rw * r.3.saturating_sub(r.1) as usize { return None; }
        let (x0, y0, x1, y1) = (r.0.min(width), r.1.min(height), r.2.min(width), r.3.min(height));
        if x0 >= x1 || y0 >= y1 { return None; }
        let mut s = Self::empty(width, height);
        for ty in y0 / TILE..y1.div_ceil(TILE) {
            for tx in x0 / TILE..x1.div_ceil(TILE) {
                let (bx, by) = (tx * TILE, ty * TILE);
                let mut p = Box::new([0u8; TN]);
                let mut any = false;
                for y in y0.max(by)..y1.min(by + TILE) {
                    let row = (y - r.1) as usize * rw;
                    for x in x0.max(bx)..x1.min(bx + TILE) {
                        let v = mask[row + (x - r.0) as usize];
                        p[((y - by) * TILE + x - bx) as usize] = v;
                        any |= v > 0;
                    }
                }
                if any { s.tiles[(ty * s.cols + tx) as usize] = Cov::Part(p); }
            }
        }
        s.done()
    }

    /// Inside a closed outline (lasso, polygon), even-odd. Anti-aliased:
    /// each pixel by how much of it is inside (exact across, 4 rows per
    /// pixel down); else a pixel counts when its center is inside.
    pub fn polygon(width: u32, height: u32, points: &[(f32, f32)], aa: bool) -> Option<Self> {
        if points.len() < 3 || !points.iter().all(|p| p.0.is_finite() && p.1.is_finite()) { return None; }
        let (mut fx0, mut fy0, mut fx1, mut fy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in points { fx0 = fx0.min(p.0); fy0 = fy0.min(p.1); fx1 = fx1.max(p.0); fy1 = fy1.max(p.1); }
        let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        let r = (clamp(fx0.floor(), width), clamp(fy0.floor(), height), clamp(fx1.ceil(), width), clamp(fy1.ceil(), height));
        if r.0 >= r.2 || r.1 >= r.3 { return None; }
        let (w, h) = ((r.2 - r.0) as usize, (r.3 - r.1) as usize);
        let mut acc = vec![0f32; w * h];
        let sub = if aa { 4 } else { 1 };
        // Edges by their top, crossing the rows they span.
        let mut edges: Vec<((f32, f32), (f32, f32))> = (0..points.len()).filter_map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
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
            let row = &mut acc[py * w..(py + 1) * w];
            for s in 0..sub {
                let y = r.1 as f32 + py as f32 + (s as f32 + 0.5) / sub as f32;
                while next < edges.len() && edges[next].0 .1 <= y { active.push(next); next += 1; }
                active.retain(|&e| edges[e].1 .1 > y);
                if active.is_empty() { continue; }
                xs.clear();
                xs.extend(active.iter().map(|&e| { let (a, b) = edges[e]; a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0) }));
                xs.sort_by(f32::total_cmp);
                for span in xs.chunks_exact(2) {
                    let (x0, x1) = ((span[0] - r.0 as f32).max(0.0), (span[1] - r.0 as f32).min(w as f32));
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
        }
        let mask: Vec<u8> = acc.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        Self::from_mask(width, height, r, &mask)
    }

    /// An ellipse in the box `(x0, y0, x1, y1)` (canvas pixels).
    pub fn ellipse(width: u32, height: u32, b: (f32, f32, f32, f32), aa: bool) -> Option<Self> {
        let (cx, cy, rx, ry) = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0, (b.2 - b.0).abs() / 2.0, (b.3 - b.1).abs() / 2.0);
        if !(rx > 0.0 && ry > 0.0 && cx.is_finite() && cy.is_finite()) { return None; }
        // Fine enough that the flat sides stay under a tenth of a pixel.
        let n = ((std::f32::consts::TAU * (rx.max(ry) / 0.2).sqrt()).ceil() as usize).clamp(16, 4096);
        let pts: Vec<(f32, f32)> = (0..n).map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        }).collect();
        Self::polygon(width, height, &pts, aa)
    }

    /// The pixels of a fill area (the brushes' "stay inside the lines").
    pub fn from_area(width: u32, height: u32, area: &crate::fill::Area) -> Option<Self> {
        let b = area.bbox;
        let (x1, y1) = (b.2.min(width), b.3.min(height));
        if x1 <= b.0 || y1 <= b.1 { return None; }
        let r = (b.0, b.1, x1, y1);
        let mut mask = vec![0u8; ((x1 - b.0) * (y1 - b.1)) as usize];
        let mut i = 0;
        for y in b.1..y1 {
            for x in b.0..x1 {
                if area.contains(x, y) { mask[i] = 255; }
                i += 1;
            }
        }
        Self::from_mask(width, height, r, &mask)
    }

    /// The opaque pixels of a layer, by their alpha (Krita's "select
    /// opaque").
    pub fn from_alpha(tiles: &crate::Tiles) -> Option<Self> {
        let (w, h) = (tiles.width(), tiles.height());
        let r = if tiles.fill()[3] > 0 { (0, 0, w, h) } else { tiles.visible_bounds()? };
        let px = tiles.read_rect(r);
        let mask: Vec<u8> = px.chunks_exact(4).map(|p| p[3]).collect();
        Self::from_mask(w, h, r, &mask)
    }

    pub fn coverage(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height { return 0; }
        match &self.tiles[((y / TILE) * self.cols + x / TILE) as usize] {
            Cov::Empty => 0,
            Cov::Full => 255,
            Cov::Part(p) => p[((y % TILE) * TILE + x % TILE) as usize],
        }
    }

    /// Selected at all (soft edges included).
    pub fn contains(&self, x: u32, y: u32) -> bool { self.coverage(x, y) > 0 }

    /// Inside the outline (at least half selected).
    pub fn inside(&self, x: u32, y: u32) -> bool { self.coverage(x, y) >= 128 }

    /// Coverage of `r` (clipped to the canvas: 0 past it), one byte per
    /// pixel, rows of `r`.
    pub fn mask(&self, r: Rect) -> Vec<u8> {
        let (rw, rh) = (r.2.saturating_sub(r.0) as usize, r.3.saturating_sub(r.1) as usize);
        let mut out = vec![0u8; rw * rh];
        let (x0, y0, x1, y1) = (r.0.min(self.width), r.1.min(self.height), r.2.min(self.width), r.3.min(self.height));
        for ty in y0 / TILE..y1.div_ceil(TILE) {
            for tx in x0 / TILE..x1.div_ceil(TILE) {
                let t = &self.tiles[(ty * self.cols + tx) as usize];
                let (bx, by) = (tx * TILE, ty * TILE);
                let (a, c) = (x0.max(bx), x1.min(bx + TILE));
                for y in y0.max(by)..y1.min(by + TILE) {
                    let o = &mut out[(y - r.1) as usize * rw + (a - r.0) as usize..][..(c - a) as usize];
                    match t {
                        Cov::Empty => {}
                        Cov::Full => o.fill(255),
                        Cov::Part(p) => o.copy_from_slice(&p[((y - by) * TILE + a - bx) as usize..][..(c - a) as usize]),
                    }
                }
            }
        }
        out
    }

    pub fn bbox(&self) -> Rect { self.bbox }

    pub fn is_rect(&self) -> bool { self.is_rect }

    pub fn size(&self) -> (u32, u32) { (self.width, self.height) }

    /// Changes with every new content (cache key).
    pub fn stamp(&self) -> u64 { self.stamp }

    /// Memory held (partial tiles).
    pub fn bytes(&self) -> usize {
        self.tiles.iter().filter(|t| matches!(t, Cov::Part(_))).count() * TN + self.tiles.len() * 16
    }

    /// `new` met with `old` (None: nothing selected).
    pub fn combine(old: Option<&Selection>, new: Option<Selection>, mode: Mode) -> Option<Selection> {
        match (mode, old, new) {
            (Mode::Replace, _, n) => n,
            (Mode::Add, None, n) => n,
            (Mode::Add | Mode::Subtract, Some(o), None) => Some(o.clone()),
            (Mode::Subtract | Mode::Intersect, None, _) | (Mode::Intersect, _, None) => None,
            (_, Some(o), Some(n)) if o.size() != n.size() => Some(n),
            (m, Some(o), Some(n)) => {
                let f = |a: u8, b: u8| match m {
                    Mode::Add => a.max(b),
                    Mode::Subtract => a.saturating_sub(b),
                    _ => a.min(b),
                };
                let mut s = Self::empty(o.width, o.height);
                for (i, (a, b)) in o.tiles.iter().zip(&n.tiles).enumerate() {
                    s.tiles[i] = match (m, a, b) {
                        (_, Cov::Empty, Cov::Empty) => Cov::Empty,
                        (Mode::Add, Cov::Full, _) | (Mode::Add, _, Cov::Full) => Cov::Full,
                        (Mode::Add, x, Cov::Empty) | (Mode::Add, Cov::Empty, x) => x.clone(),
                        (Mode::Subtract, _, Cov::Full) | (Mode::Subtract, Cov::Empty, _) => Cov::Empty,
                        (Mode::Subtract, x, Cov::Empty) => x.clone(),
                        (Mode::Intersect, Cov::Empty, _) | (Mode::Intersect, _, Cov::Empty) => Cov::Empty,
                        (Mode::Intersect, x, Cov::Full) | (Mode::Intersect, Cov::Full, x) => x.clone(),
                        _ => {
                            let v = |c: &Cov, k: usize| match c { Cov::Empty => 0, Cov::Full => 255, Cov::Part(p) => p[k] };
                            let mut p = Box::new([0u8; TN]);
                            for (k, out) in p.iter_mut().enumerate() { *out = f(v(a, k), v(b, k)); }
                            Cov::Part(p)
                        }
                    };
                }
                s.done()
            }
        }
    }

    /// Everything else (None: all was selected).
    pub fn inverted(&self) -> Option<Self> {
        let mut s = self.clone();
        for t in &mut s.tiles {
            *t = match t {
                Cov::Empty => Cov::Full,
                Cov::Full => Cov::Empty,
                Cov::Part(p) => { let mut q = p.clone(); for v in q.iter_mut() { *v = 255 - *v; } Cov::Part(q) }
            };
        }
        s.is_rect = false;
        s.done()
    }

    /// Moved by whole pixels (what goes past the canvas is lost).
    pub fn translated(&self, dx: i64, dy: i64) -> Option<Self> {
        let b = self.bbox;
        let m = self.mask(b);
        let shift = |v: u32, d: i64| v as i64 + d;
        let (x0, y0, x1, y1) = (shift(b.0, dx), shift(b.1, dy), shift(b.2, dx), shift(b.3, dy));
        let (cx0, cy0) = (x0.clamp(0, self.width as i64), y0.clamp(0, self.height as i64));
        let (cx1, cy1) = (x1.clamp(0, self.width as i64), y1.clamp(0, self.height as i64));
        if cx0 >= cx1 || cy0 >= cy1 { return None; }
        let (bw, cw) = ((b.2 - b.0) as usize, (cx1 - cx0) as usize);
        let mut out = vec![0u8; cw * (cy1 - cy0) as usize];
        for y in cy0..cy1 {
            let src = (y - y0) as usize * bw + (cx0 - x0) as usize;
            out[(y - cy0) as usize * cw..][..cw].copy_from_slice(&m[src..src + cw]);
        }
        let mut s = Self::from_mask(self.width, self.height, (cx0 as u32, cy0 as u32, cx1 as u32, cy1 as u32), &out)?;
        // A rectangle moved and clipped is still one.
        s.is_rect = self.is_rect;
        Some(s)
    }

    /// The bounding box grown by `m`, clipped to the canvas.
    fn around(&self, m: u32) -> Rect {
        let b = self.bbox;
        (b.0.saturating_sub(m), b.1.saturating_sub(m), (b.2 + m).min(self.width), (b.3 + m).min(self.height))
    }

    /// Grown by `r` pixels, round (Photoshop's Expand, Krita's Grow).
    pub fn grown(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let area = self.around(r.ceil() as u32 + 1);
        let (w, h) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        let m = self.mask(area);
        let d = distance(&m.iter().map(|v| *v >= 128).collect::<Vec<_>>(), w, h);
        let out: Vec<u8> = m.iter().zip(d).map(|(v, d)| (*v).max(((r + 1.0 - d).clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        Self::from_mask(self.width, self.height, area, &out)
    }

    /// Shrunk by `r` pixels (the canvas edge doesn't count as unselected).
    pub fn shrunk(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let area = self.around(1);
        let (w, h) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        let m = self.mask(area);
        // Distance to the nearest unselected pixel inside the canvas.
        let d = distance(&m.iter().map(|v| *v < 128).collect::<Vec<_>>(), w, h);
        let out: Vec<u8> = m.iter().zip(d).map(|(v, d)| (*v).min((255.0 - (r + 1.0 - d).clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        Self::from_mask(self.width, self.height, area, &out)
    }

    /// Hard edges softened: a pixel on the edge gets the average of its
    /// 3 × 3 neighbors, kept on its side of one half (the magic wand's
    /// anti-aliasing).
    pub fn antialiased(&self) -> Option<Self> {
        let area = self.around(1);
        let (w, h) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        let m = self.mask(area);
        let mut out = m.clone();
        for y in 0..h {
            for x in 0..w {
                let v = m[y * w + x];
                let (mut sum, mut edge) = (0u32, false);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        let n = if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 { v } else { m[ny as usize * w + nx as usize] };
                        edge |= n != v;
                        sum += n as u32;
                    }
                }
                if edge {
                    let avg = sum as f32 / (9.0 * 255.0);
                    let f = if v > 127 { 0.5 + 0.5 * avg } else { 0.5 * avg };
                    out[y * w + x] = (f * 255.0).round() as u8;
                }
            }
        }
        Self::from_mask(self.width, self.height, area, &out)
    }

    /// Soft edges: a Gaussian blur of the coverage, sigma `r` (Photoshop's
    /// Feather; nothing past the canvas).
    pub fn feathered(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return Some(self.clone()); }
        let area = self.around((r * 3.0).ceil() as u32 + 1);
        let (w, h) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        let mut m: Vec<f32> = self.mask(area).iter().map(|v| *v as f32).collect();
        gaussian(&mut m, w, h, r);
        let out: Vec<u8> = m.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
        Self::from_mask(self.width, self.height, area, &out)
    }

    /// A soft band `2r` wide along the outline (Photoshop's Border).
    pub fn bordered(&self, r: f32) -> Option<Self> {
        if r <= 0.0 { return None; }
        let area = self.around(r.ceil() as u32 + 1);
        let (w, h) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        let m = self.mask(area);
        let inside: Vec<bool> = m.iter().map(|v| *v >= 128).collect();
        let outside: Vec<bool> = inside.iter().map(|v| !v).collect();
        let (to_in, to_out) = (distance(&inside, w, h), distance(&outside, w, h));
        let out: Vec<u8> = inside.iter().enumerate().map(|(i, &sel)| {
            let d = if sel { to_out[i] } else { to_in[i] };
            ((1.0 - (d - 1.0) / r).clamp(0.0, 1.0) * 255.0).round() as u8
        }).collect();
        Self::from_mask(self.width, self.height, area, &out)
    }

    /// The outline (where coverage crosses one half) as horizontal and
    /// vertical runs, looking at one pixel per `step` × `step` block (zoomed
    /// out, a fine outline is not seen anyway).
    pub fn outline(&self, step: u32) -> Vec<Segment> {
        let s = step.max(1);
        let b = self.bbox;
        if b.2 <= b.0 { return Vec::new(); }
        // Cells over the box, one more all around (outside).
        let (gx0, gy0) = (b.0 / s, b.1 / s);
        let (gx1, gy1) = (b.2.div_ceil(s), b.3.div_ceil(s));
        let (w, h) = ((gx1 - gx0 + 2) as usize, (gy1 - gy0 + 2) as usize);
        let mut grid = vec![false; w * h];
        for gy in 1..h - 1 {
            let y = ((gy0 + gy as u32 - 1) * s + s / 2).clamp(b.1, b.3 - 1);
            for gx in 1..w - 1 {
                let x = ((gx0 + gx as u32 - 1) * s + s / 2).clamp(b.0, b.2 - 1);
                grid[gy * w + gx] = self.inside(x, y);
            }
        }
        let at = |x: usize, y: usize| grid[y * w + x];
        // Cell edges -> canvas pixels, kept inside the box.
        let px = |g: usize| (((gx0 as i64 + g as i64 - 1) * s as i64).clamp(b.0 as i64, b.2 as i64)) as f32;
        let py = |g: usize| (((gy0 as i64 + g as i64 - 1) * s as i64).clamp(b.1 as i64, b.3 as i64)) as f32;
        let mut out = Vec::new();
        for y in 1..h {
            let mut run = None;
            for x in 0..=w {
                let edge = x < w && at(x, y) != at(x, y - 1);
                match (edge, run) {
                    (true, None) => run = Some(x),
                    (false, Some(x0)) => { out.push([(px(x0), py(y)), (px(x), py(y))]); run = None; }
                    _ => {}
                }
            }
        }
        for x in 1..w {
            let mut run = None;
            for y in 0..=h {
                let edge = y < h && at(x, y) != at(x - 1, y);
                match (edge, run) {
                    (true, None) => run = Some(y),
                    (false, Some(y0)) => { out.push([(px(x), py(y0)), (px(x), py(y))]); run = None; }
                    _ => {}
                }
            }
        }
        out
    }

    /// Blend `old` toward `new` by the coverage at (x, y): pixels of `r`,
    /// straight RGBA; the result goes to `new`. Unselected pixels keep
    /// `old`, half-selected ones get half of the change (premultiplied).
    pub fn blend_rect(&self, r: Rect, old: &[u8], new: &mut [u8]) {
        let m = self.mask(r);
        for ((n, o), &c) in new.chunks_exact_mut(4).zip(old.chunks_exact(4)).zip(&m) {
            Self::blend_px(o, n, c);
        }
    }

    /// One pixel of blend_rect: `new` becomes `old` changed by `c` / 255
    /// of the way to it.
    pub fn blend_px(old: &[u8], new: &mut [u8], c: u8) {
        match c {
            255 => {}
            0 => new.copy_from_slice(old),
            c => {
                let a = mix(old[3], new[3], c);
                if a == 0 { new.copy_from_slice(old); new[3] = 0; return; }
                for k in 0..3 {
                    let (po, pn) = (old[k] as u32 * old[3] as u32, new[k] as u32 * new[3] as u32);
                    let v = (po * (255 - c as u32) + pn * c as u32) / 255;
                    new[k] = ((v + a as u32 / 2) / a as u32).min(255) as u8;
                }
                new[3] = a;
            }
        }
    }
}

/// Distance from each pixel's center to the nearest `set` one (Felzenszwalb
/// and Huttenlocher's exact transform, a pass per column then per row).
fn distance(set: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    let mut g: Vec<f32> = set.iter().map(|&s| if s { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0f32; n], vec![0f32; n], vec![0usize; n], vec![0f32; n + 1]);
    let mut pass = |f: &[f32], len: usize, d: &mut [f32]| {
        let meet = |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
        let mut k = 0;
        v[0] = 0;
        z[0] = f32::NEG_INFINITY;
        z[1] = f32::INFINITY;
        for q in 1..len {
            let mut s = meet(q, v[k]);
            while s <= z[k] { k -= 1; s = meet(q, v[k]); }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
        }
        k = 0;
        for (q, dq) in d.iter_mut().enumerate().take(len) {
            while z[k + 1] < q as f32 { k += 1; }
            let p = v[k];
            *dq = (q as f32 - p as f32).powi(2) + f[p];
        }
    };
    for x in 0..w {
        for y in 0..h { f[y] = g[y * w + x]; }
        pass(&f, h, &mut d);
        for y in 0..h { g[y * w + x] = d[y]; }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        pass(&f, w, &mut d);
        g[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    g.iter().map(|v| v.min(INF).sqrt()).collect()
}

/// Gaussian blur, sigma `s`, by three box blurs a way (zero past the
/// edges).
fn gaussian(m: &mut [f32], w: usize, h: usize, s: f32) {
    // Box sizes giving that sigma (Kovesi).
    let n = 3.0;
    let wl = ((12.0 * s * s / n + 1.0).sqrt()).floor() as i64;
    let wl = if wl % 2 == 0 { wl - 1 } else { wl }.max(1);
    let wu = wl + 2;
    let mm = ((12.0 * s * s - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0)).round() as i64;
    let radii: Vec<usize> = (0..3).map(|i| (if i < mm { wl } else { wu } as usize - 1) / 2).collect();
    let mut tmp = vec![0f32; w.max(h)];
    let mut line = vec![0f32; w.max(h)];
    let blur = |line: &mut [f32], len: usize, r: usize, tmp: &mut [f32]| {
        if r == 0 { return; }
        let inv = 1.0 / (2 * r + 1) as f32;
        let mut acc = 0f32;
        for v in line.iter().take(r.min(len)) { acc += v; }
        for i in 0..len {
            if i + r < len { acc += line[i + r]; }
            if i > r { acc -= line[i - r - 1]; }
            tmp[i] = acc * inv;
        }
        line[..len].copy_from_slice(&tmp[..len]);
    };
    for &r in &radii {
        for y in 0..h { blur(&mut m[y * w..(y + 1) * w], w, r, &mut tmp); }
        for x in 0..w {
            for y in 0..h { line[y] = m[y * w + x]; }
            blur(&mut line, h, r, &mut tmp);
            for y in 0..h { m[y * w + x] = line[y]; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_lasso_select_the_pixels_inside() {
        let r = Selection::rect(100, 100, (10, 20, 30, 40)).unwrap();
        assert!(r.contains(10, 20) && r.contains(29, 39) && !r.contains(30, 20) && !r.contains(9, 25));
        assert_eq!(r.bbox(), (10, 20, 30, 40));
        assert!(Selection::rect(100, 100, (200, 0, 300, 10)).is_none());

        // A triangle: its tip is narrow, its base wide.
        let t = Selection::polygon(100, 100, &[(50.0, 10.0), (90.0, 90.0), (10.0, 90.0)], false).unwrap();
        assert!(t.contains(50, 50) && t.contains(15, 89));
        assert!(!t.contains(15, 15) && !t.contains(50, 95));
        // Outside the canvas: clipped, never out of bounds.
        let big = Selection::polygon(100, 100, &[(-50.0, -50.0), (500.0, -50.0), (500.0, 500.0)], true).unwrap();
        assert!(big.contains(99, 99) && big.bbox().2 <= 100);
        assert!(Selection::polygon(100, 100, &[(1.0, 1.0), (2.0, 2.0)], false).is_none());
        assert!(Selection::polygon(100, 100, &[(f32::NAN, 1.0), (2.0, 2.0), (3.0, 1.0)], false).is_none());
    }

    #[test]
    fn anti_aliased_shapes_have_soft_edges() {
        // Half a pixel across: half selected.
        let s = Selection::polygon(20, 20, &[(2.0, 2.0), (10.5, 2.0), (10.5, 10.0), (2.0, 10.0)], true).unwrap();
        assert_eq!(s.coverage(5, 5), 255);
        assert!((120..=135).contains(&s.coverage(10, 5)), "{}", s.coverage(10, 5));
        assert_eq!(s.coverage(11, 5), 0);
        let e = Selection::ellipse(200, 200, (20.0, 20.0, 180.0, 120.0), true).unwrap();
        assert_eq!(e.coverage(100, 70), 255);
        assert_eq!(e.coverage(25, 25), 0);
        // Its edge is soft, its area close to pi a b.
        let m = e.mask(e.bbox());
        assert!(m.iter().any(|v| *v > 0 && *v < 255));
        let area: f32 = m.iter().map(|v| *v as f32 / 255.0).sum();
        assert!((area - std::f32::consts::PI * 80.0 * 50.0).abs() < 20.0, "{area}");
    }

    #[test]
    fn modes_add_subtract_intersect() {
        let a = Selection::rect(100, 100, (0, 0, 60, 100));
        let b = Selection::rect(100, 100, (40, 0, 100, 100));
        let add = Selection::combine(a.as_ref(), b.clone(), Mode::Add).unwrap();
        assert_eq!(add.bbox(), (0, 0, 100, 100));
        let sub = Selection::combine(a.as_ref(), b.clone(), Mode::Subtract).unwrap();
        assert_eq!(sub.bbox(), (0, 0, 40, 100));
        let int = Selection::combine(a.as_ref(), b.clone(), Mode::Intersect).unwrap();
        assert_eq!(int.bbox(), (40, 0, 60, 100));
        assert!(Selection::combine(a.as_ref(), Selection::rect(100, 100, (70, 0, 90, 9)), Mode::Intersect).is_none());
        assert_eq!(Selection::combine(None, b.clone(), Mode::Add), b);
        assert!(Selection::combine(None, b, Mode::Subtract).is_none());
        let all = Selection::all(100, 100);
        let inv = a.unwrap().inverted().unwrap();
        assert_eq!(inv.bbox(), (60, 0, 100, 100));
        assert!(all.inverted().is_none());
    }

    #[test]
    fn grow_shrink_feather_border_and_move() {
        let s = Selection::rect(200, 200, (50, 50, 150, 150)).unwrap();
        let g = s.grown(10.0).unwrap();
        assert_eq!(g.bbox(), (40, 40, 160, 160));
        assert_eq!(g.coverage(40, 100), 255);
        // Round corners.
        assert_eq!(g.coverage(41, 41), 0);
        let k = s.shrunk(10.0).unwrap();
        assert_eq!(k.coverage(60, 100), 255);
        assert!(k.coverage(59, 100) < 255 && k.coverage(55, 100) == 0);
        // The canvas edge stays selected.
        let edge = Selection::rect(200, 200, (0, 0, 100, 200)).unwrap().shrunk(10.0).unwrap();
        assert_eq!(edge.coverage(0, 100), 255);
        assert!(edge.coverage(95, 100) == 0);
        let f = s.feathered(5.0).unwrap();
        assert!((100..=155).contains(&f.coverage(50, 100)), "{}", f.coverage(50, 100));
        assert_eq!(f.coverage(100, 100), 255);
        assert!(f.bbox().0 < 50);
        let b = s.bordered(4.0).unwrap();
        assert_eq!(b.coverage(100, 100), 0);
        assert!(b.coverage(50, 100) > 200 && b.coverage(49, 100) > 200);
        let m = s.translated(-60, 10).unwrap();
        assert_eq!(m.bbox(), (0, 60, 90, 160));
        assert!(s.translated(500, 0).is_none());
    }

    #[test]
    fn outline_follows_the_half_coverage() {
        let s = Selection::rect(100, 100, (2, 3, 10, 7)).unwrap();
        let o = s.outline(1);
        assert_eq!(o.len(), 4, "{o:?}");
        assert!(o.contains(&[(2.0, 3.0), (10.0, 3.0)]));
        assert!(o.contains(&[(10.0, 3.0), (10.0, 7.0)]));
        let mut hole = Selection::rect(100, 100, (0, 0, 50, 50)).unwrap();
        hole = Selection::combine(Some(&hole), Selection::rect(100, 100, (20, 20, 30, 30)), Mode::Subtract).unwrap();
        assert_eq!(hole.outline(1).len(), 8);
        // Coarser steps stay inside the box.
        for seg in hole.outline(8) { for p in seg { assert!(p.0 >= 0.0 && p.0 <= 50.0 && p.1 <= 50.0); } }
    }

    #[test]
    fn soft_selections_blend_changes() {
        let s = Selection::from_mask(4, 1, (0, 0, 4, 1), &[0, 128, 255, 0]).unwrap();
        let old = [10, 10, 10, 255, 10, 10, 10, 255, 10, 10, 10, 255, 255, 255, 255, 0];
        let mut new = [200, 200, 200, 255, 200, 200, 200, 255, 200, 200, 200, 255, 200, 0, 0, 255];
        s.blend_rect((0, 0, 4, 1), &old, &mut new);
        assert_eq!(&new[0..4], &[10, 10, 10, 255]);
        assert!((100..110).contains(&new[4]));
        assert_eq!(&new[8..12], &[200, 200, 200, 255]);
        assert_eq!(new[15], 0);
        assert_eq!(s.bbox(), (1, 0, 3, 1));
    }

    #[test]
    fn edge_tiles_compare_and_round_trip() {
        let a = Selection::all(70, 70);
        let b = Selection::rect(70, 70, (0, 0, 70, 70)).unwrap();
        assert_eq!(a, b);
        let m = Selection::from_mask(70, 70, (0, 0, 70, 70), &vec![255; 70 * 70]).unwrap();
        assert_eq!(a, m);
        assert_eq!(a.mask((60, 60, 80, 80)).iter().filter(|v| **v == 255).count(), 100);
    }
}

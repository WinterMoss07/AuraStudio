//! Fill tool: the area of similar color around a point (or everywhere:
//! the magic wand without "contiguous"), on what a `FillSource` gives:
//! the layer itself, or a merge `compose` makes (drafts left out, the
//! reference only). Worked out a block of 64 × 64 pixels at a time as the
//! fill reaches it; one bit per pixel. The canvas is unbounded: a fill
//! stays inside `bounds` (the frame, usually).

use aurastudio_core::{Rect, Rgba8};
use aurastudio_raster::{Image, Mask};

use crate::{LayerStack, Pick};

/// Pixels per bit word (a block is WORD × WORD pixels).
const WORD: u32 = 64;

/// What the fill looks at to find the area (CSP's "Refer multiple").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refer {
    /// Only the layer filled.
    Layer,
    /// Every visible layer, merged.
    #[default]
    All,
    /// The reference layer of the filled layer's folder (or the nearest
    /// folder around with one, up to the top level).
    References,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillParams {
    /// Largest difference per channel still counted as "the same color".
    pub tolerance:   u8,
    pub refer:       Refer,
    /// Leave the draft layers out of what is looked at (CSP's "Exclude
    /// draft layers").
    pub skip_drafts: bool,
    /// Grow the area by this many pixels, under the edges of anti-aliased
    /// lines (no white halo between the line and the fill).
    pub expand:      u32,
}

impl Default for FillParams {
    fn default() -> Self { Self { tolerance: 32, refer: Refer::All, skip_drafts: true, expand: 1 } }
}

impl FillParams {
    /// The merge looked at to fill `layer` (None: the layer itself).
    pub fn pick(&self, layers: &LayerStack, layer: u64) -> Option<Pick> {
        match self.refer {
            Refer::Layer => None,
            Refer::All => Some(Pick { skip_drafts: self.skip_drafts, ..Pick::ALL }),
            Refer::References => Some(Pick { skip_drafts: self.skip_drafts, reference: Some(layers.reference_for(layer).unwrap_or(0)), ..Pick::ALL }),
        }
    }
}

/// What the fill reads: pixels of a rect, and one color for a whole rect
/// when that is known without reading pixels.
pub trait FillSource {
    fn pixels(&self, r: Rect) -> Vec<Rgba8>;
    fn uniform(&self, _r: Rect) -> Option<Rgba8> { None }
}

impl FillSource for Image {
    fn pixels(&self, r: Rect) -> Vec<Rgba8> { self.read(r) }
    fn uniform(&self, r: Rect) -> Option<Rgba8> { self.uniform_in(r) }
}

/// The area: one bit per pixel of `bounds` (a word per 64 pixels of a
/// row), and its bounding box.
#[derive(Clone, Debug)]
pub struct Area {
    bounds: Rect,
    cols:   usize,
    bits:   Vec<u64>,
    /// Relative to `bounds`: x0, y0, x1, y1.
    bb:     (u32, u32, u32, u32),
}

impl Area {
    fn new(bounds: Rect) -> Self {
        let cols = bounds.w().div_ceil(WORD) as usize;
        Self { bounds, cols, bits: vec![0; cols * bounds.h() as usize], bb: (0, 0, 0, 0) }
    }

    /// Canvas pixels.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if !self.bounds.contains(x, y) { return false; }
        let (x, y) = ((x - self.bounds.x0) as u32, (y - self.bounds.y0) as u32);
        self.bits[y as usize * self.cols + (x / WORD) as usize] >> (x % WORD) & 1 == 1
    }

    /// The bounding box, canvas pixels.
    pub fn bbox(&self) -> Rect {
        let (x0, y0, x1, y1) = self.bb;
        if x1 <= x0 { return Rect::EMPTY; }
        Rect::new(self.bounds.x0 + x0 as i32, self.bounds.y0 + y0 as i32, self.bounds.x0 + x1 as i32, self.bounds.y0 + y1 as i32)
    }

    /// 255 inside, 0 outside, for each pixel of `r` (rows packed): the
    /// coverage the fill paints with, or a selection.
    pub fn coverage(&self, r: Rect) -> Vec<u8> {
        let mut out = Vec::with_capacity(r.area() as usize);
        for y in r.y0..r.y1 { for x in r.x0..r.x1 { out.push(if self.contains(x, y) { 255 } else { 0 }); } }
        out
    }

    fn set_run(&mut self, y: u32, x0: u32, x1: u32) {
        let row = y as usize * self.cols;
        let mut x = x0;
        while x < x1 {
            let (c, b) = ((x / 64) as usize, x % 64);
            let n = (x1 - x).min(64 - b);
            let ones = if n == 64 { u64::MAX } else { ((1u64 << n) - 1) << b };
            self.bits[row + c] |= ones;
            x += n;
        }
    }
}

/// Which pixels may be filled (similar to the seed color, selected),
/// worked out a whole block at a time the first time the fill reaches it.
struct Candidates<'a> {
    src:       &'a dyn FillSource,
    bounds:    Rect,
    limit:     Option<&'a Mask>,
    seed:      Rgba8,
    tolerance: u8,
    cols:      u32,
    /// Per block: one word per row.
    blocks:    Vec<Option<Box<[u64; 64]>>>,
}

impl Candidates<'_> {
    /// Row `y` of block column `c` (relative): bit i is pixel (c × 64 + i, y).
    fn word(&mut self, y: u32, c: u32) -> u64 {
        let i = ((y / WORD) * self.cols + c) as usize;
        let b = self.blocks[i].get_or_insert_with(|| Self::make(self.src, self.bounds, self.limit, self.seed, self.tolerance, c, y / WORD));
        b[(y % WORD) as usize]
    }

    fn make(src: &dyn FillSource, bounds: Rect, limit: Option<&Mask>, seed: Rgba8, tol: u8, bx: u32, by: u32) -> Box<[u64; 64]> {
        let (x0, y0) = (bounds.x0 + (bx * WORD) as i32, bounds.y0 + (by * WORD) as i32);
        let r = Rect::new(x0, y0, (x0 + WORD as i32).min(bounds.x1), (y0 + WORD as i32).min(bounds.y1));
        let (tw, th) = (r.w(), r.h());
        let mut out = Box::new([0u64; 64]);
        let full = if tw == 64 { u64::MAX } else { (1u64 << tw) - 1 };
        // Outside the limit: nothing; one color: all or nothing.
        let lim = limit.map(|m| (m.uniform_in(r), m));
        if matches!(lim, Some((Some(0), _))) { return out; }
        let px = match src.uniform(r) {
            Some(c) if similar(c, seed, tol) => None,
            Some(_) => return out,
            None => Some(src.pixels(r)),
        };
        let lim_px = match lim { Some((None, m)) => Some(m.read(r)), _ => None };
        for y in 0..th {
            let mut word = match &px {
                None => full,
                Some(p) => {
                    let mut w = 0u64;
                    for x in 0..tw { if similar(p[(y * tw + x) as usize], seed, tol) { w |= 1 << x; } }
                    w
                }
            };
            if let Some(l) = &lim_px {
                for x in 0..tw { if l[(y * tw + x) as usize] == 0 { word &= !(1 << x); } }
            }
            out[y as usize] = word;
        }
        out
    }
}

fn similar(a: Rgba8, b: Rgba8, tol: u8) -> bool {
    // Fully transparent pixels are alike whatever their color.
    if a[3] == 0 && b[3] == 0 { return true; }
    a.iter().zip(&b).all(|(&p, &q)| p.abs_diff(q) <= tol)
}

/// The area to fill from (x, y) inside `bounds` on what `src` shows,
/// within `limit` (a selection) if any. `None` if the point is outside
/// the bounds or the limit.
pub fn flood(src: &dyn FillSource, bounds: Rect, x: i32, y: i32, tolerance: u8, expand: u32, limit: Option<&Mask>) -> Option<Area> {
    aurastudio_core::zone!("fill flood");
    if !bounds.contains(x, y) || limit.is_some_and(|m| m.pixel(x, y) == 0) { return None; }
    let seed = src.pixels(Rect::new(x, y, x + 1, y + 1)).first().copied()?;
    let (w, h) = (bounds.w(), bounds.h());
    let cols = w.div_ceil(WORD);
    let mut cand = Candidates { src, bounds, limit, seed, tolerance, cols, blocks: vec![None; (cols * h.div_ceil(WORD)) as usize] };
    let (x, y) = ((x - bounds.x0) as u32, (y - bounds.y0) as u32);
    let mut area = Area::new(bounds);
    // Fillable and not filled yet, a word at a time.
    let free = |c: &mut Candidates, a: &Area, y: u32, col: u32| c.word(y, col) & !a.bits[y as usize * a.cols + col as usize];
    // Scanline fill: whole runs of a row at a time.
    let mut stack = vec![(x, y)];
    let (mut bx0, mut by0, mut bx1, mut by1) = (x, y, x + 1, y + 1);
    while let Some((sx, sy)) = stack.pop() {
        if free(&mut cand, &area, sy, sx / 64) >> (sx % 64) & 1 == 0 { continue; }
        // Run end on the right: first pixel not free.
        let mut x1 = sx;
        while x1 < w {
            let b = x1 % 64;
            let n = (!(free(&mut cand, &area, sy, x1 / 64) >> b)).trailing_zeros();
            x1 += n;
            if n < 64 - b { break; }
        }
        let x1 = x1.min(w);
        // Run start on the left.
        let mut x0 = sx;
        loop {
            let b = x0 % 64;
            let n = (!(free(&mut cand, &area, sy, x0 / 64) << (63 - b))).leading_zeros();
            if n < b + 1 || x0 < 64 { x0 = x0 + 1 - n; break; }
            x0 -= n;
        }
        area.set_run(sy, x0, x1);
        (bx0, bx1, by0, by1) = (bx0.min(x0), bx1.max(x1), by0.min(sy), by1.max(sy + 1));
        for ny in [sy.wrapping_sub(1), sy + 1] {
            if ny >= h { continue; }
            // One seed per run of free pixels on the next row, under x0..x1.
            let mut carry = 0u64;
            for c in x0 / 64..x1.div_ceil(64) {
                let lo = if c == x0 / 64 { u64::MAX << (x0 % 64) } else { u64::MAX };
                let hi_end = x1 - c * 64;
                let hi = if hi_end >= 64 { u64::MAX } else { (1u64 << hi_end) - 1 };
                let m = free(&mut cand, &area, ny, c) & lo & hi;
                let mut starts = m & !(m << 1 | carry);
                carry = m >> 63;
                while starts != 0 {
                    stack.push((c * 64 + starts.trailing_zeros(), ny));
                    starts &= starts - 1;
                }
            }
        }
    }
    area.bb = (bx0, by0, bx1, by1);
    for _ in 0..expand { grow(&mut area, w, h, limit); }
    Some(area)
}

/// Every pixel like the one at (x, y) inside `bounds`, wherever it is
/// (the magic wand without "contiguous").
pub fn similar_all(src: &dyn FillSource, bounds: Rect, x: i32, y: i32, tolerance: u8, expand: u32) -> Option<Area> {
    aurastudio_core::zone!("fill similar");
    if !bounds.contains(x, y) { return None; }
    let seed = src.pixels(Rect::new(x, y, x + 1, y + 1)).first().copied()?;
    let (w, h) = (bounds.w(), bounds.h());
    let cols = w.div_ceil(WORD);
    let mut cand = Candidates { src, bounds, limit: None, seed, tolerance, cols, blocks: vec![None; (cols * h.div_ceil(WORD)) as usize] };
    let mut area = Area::new(bounds);
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    for ty in 0..h.div_ceil(WORD) {
        for c in 0..cols {
            for yy in ty * WORD..((ty + 1) * WORD).min(h) {
                let word = cand.word(yy, c);
                let edge = w - c * 64;
                let word = if edge < 64 { word & ((1u64 << edge) - 1) } else { word };
                if word == 0 { continue; }
                area.bits[yy as usize * area.cols + c as usize] = word;
                bx0 = bx0.min(c * 64 + word.trailing_zeros());
                bx1 = bx1.max(c * 64 + 64 - word.leading_zeros());
                by0 = by0.min(yy);
                by1 = by1.max(yy + 1);
            }
            // Done with that block: free it.
            cand.blocks[(ty * cols + c) as usize] = None;
        }
    }
    if bx0 == u32::MAX { return None; }
    area.bb = (bx0, by0, bx1, by1);
    for _ in 0..expand { grow(&mut area, w, h, None); }
    Some(area)
}

/// Add every pixel next to the area (8 neighbors), whole words at a time.
fn grow(area: &mut Area, w: u32, h: u32, limit: Option<&Mask>) {
    let (_, y0, _, y1) = area.bb;
    let (ny0, ny1) = (y0.saturating_sub(1), (y1 + 1).min(h));
    let cols = area.cols;
    // The last word of a row only holds the pixels left of the edge.
    let last_mask = if w.is_multiple_of(64) { u64::MAX } else { (1u64 << (w % 64)) - 1 };
    let row = |y: u32| &area.bits[y as usize * cols..(y as usize + 1) * cols];
    let mut out = vec![0u64; (ny1 - ny0) as usize * cols];
    let mut v = vec![0u64; cols];
    for y in ny0..ny1 {
        // Rows above, at and below, merged; then spread one pixel sideways.
        v.copy_from_slice(row(y));
        if y > 0 { for (a, b) in v.iter_mut().zip(row(y - 1)) { *a |= b; } }
        if y + 1 < h { for (a, b) in v.iter_mut().zip(row(y + 1)) { *a |= b; } }
        let o = &mut out[(y - ny0) as usize * cols..][..cols];
        for c in 0..cols {
            let left_in = if c > 0 { v[c - 1] >> 63 } else { 0 };
            let right_in = if c + 1 < cols { v[c + 1] << 63 } else { 0 };
            o[c] = v[c] | v[c] << 1 | left_in | v[c] >> 1 | right_in;
        }
        o[cols - 1] &= last_mask;
    }
    for y in ny0..ny1 {
        let o = &out[(y - ny0) as usize * cols..][..cols];
        for (c, &grown) in o.iter().enumerate() {
            let old = area.bits[y as usize * cols + c];
            let mut add = grown & !old;
            if let Some(s) = limit {
                let mut bits = add;
                while bits != 0 {
                    let b = bits.trailing_zeros();
                    bits &= bits - 1;
                    let (px, py) = (area.bounds.x0 + (c as u32 * 64 + b) as i32, area.bounds.y0 + y as i32);
                    if s.pixel(px, py) == 0 { add &= !(1 << b); }
                }
            }
            area.bits[y as usize * cols + c] = old | add;
        }
    }
    let (x0, _, x1, _) = area.bb;
    area.bb = (x0.saturating_sub(1), ny0, (x1 + 1).min(w), ny1);
}


//! The round brush.
//!
//! The brush is smeared continuously along the pen path (no stamped dabs),
//! with "wash" accumulation (Krita's alpha darken): each pixel moves toward
//! the stroke's opacity (brush opacity × pressure) by how much brush passes
//! over it, never past it:
//!
//!   if mask < cap: mask += (cap - mask) × (1 - e^-G)
//!
//! so a stroke never gets more opaque than its pressure asks (crossing a
//! 20 % stroke at 30 % gives 30 %), while overlapping soft parts add up
//! smoothly (clean crossings, no crease).
//!
//! G is the brush deposited on the pixel: the path integral of a kernel
//! g(d) = K/R · (1 - d²/R²)² (d: distance to the path point, R: radius).
//! Stamped dabs left their spacing visible as beads inside soft strokes;
//! the integral has none. Across a straight stroke, at distance e from it,
//! G = 16/15 · K · (1 - e²/R²)^2.5: the hardness sets K so that the stroke
//! is opaque out to radius × hardness and fades to the edge from there
//! (1 px of anti-aliasing at 100 %).
//!
//! Along a segment the integral has a closed form (a polynomial, see `f`),
//! so the whole per-pixel computation is plain arithmetic that the
//! compiler vectorizes: no table lookups, no libm calls.
//!
//! The opacity a pixel moves toward is the pressure at its closest point
//! on the path, so pressure changes fade along the stroke instead of
//! leaving round blots or rings. Nothing is painted for good ahead of the
//! pen (the pressure there is unknown, and opacity can't be lowered
//! later): the round front of the brush at the pen is a preview, redrawn
//! as the pen moves on (`preview`).
//!
//! Cost: the path is drawn in segments of `spacing` pen travel (it grows
//! with the brush, as √radius past a point, like Krita's auto spacing), so
//! a 1000 px brush updates its area every ~45 px, not on every pen sample.

use aurastudio_canvas::{blend::unit_to_u8, PaintSurface, Rect};

use crate::raster::{capsule_row_span, changed_span, min_u8, write_pixels, CHUNK, IOTA};
use crate::{Brush, PressureSmoother};

/// x, y and smoothed pressure.
type PenPoint = (f32, f32, f32);

pub struct RoundBrush {
    brush:   Brush,
    /// Outer radius (brush radius + half a pixel).
    reach:   f32,
    /// Kernel strength, from the hardness.
    k:       f32,
    /// Pen travel per segment, in pixels.
    spacing: f32,
    pub smoother: PressureSmoother,
    /// Last pen position and smoothed pressure.
    last:    Option<(f32, f32, f32)>,
    /// Where the path not drawn yet starts (position, pressure), and the pen
    /// travel since.
    pending: (f32, f32, f32),
    travel:  f32,
    /// The last segment drawn (start and end).
    last_segment: Option<(PenPoint, PenPoint)>,
    /// Direction of the segment before the one being drawn.
    prev_dir: Option<(f32, f32)>,
    /// The path drawn so far, most recent part (about a reach long).
    path:    Vec<PathPoint>,
    /// Scratch: opacity of the path 0, 1, 2... px behind a segment start.
    behind:  Vec<f32>,
    /// What the brush front preview covers, to put back.
    tip:     Option<Tip>,
}

impl RoundBrush {
    pub fn new() -> Self {
        let mut e = Self {
            brush: Brush::new(), reach: 1.0, k: 1.0, spacing: 1.0,
            smoother: PressureSmoother::new(0.5), last: None, pending: (0.0, 0.0, 0.0), travel: 0.0,
            last_segment: None, prev_dir: None, path: Vec::new(), behind: Vec::new(), tip: None,
        };
        e.sync_from(&Brush::new());
        e
    }

    /// Take size, opacity, hardness, color and blend mode from the brush.
    pub fn sync_from(&mut self, b: &Brush) {
        self.brush = b.clone();
        let r = b.radius.max(1) as f32;
        self.reach = r + 0.5;
        self.spacing = (r / 6.0).min(2.0 * r.sqrt()).max(0.5);
        // Width of the fading edge, as a fraction of the radius: from the
        // whole radius (hardness 0) down to one pixel (100 %). K is set so
        // that a straight stroke is 95 % opaque where the fade starts.
        let fade = (1.0 - b.hardness.clamp(0.0, 1.0)).max(1.0 / self.reach);
        let q = 1.0 - (1.0 - fade) * (1.0 - fade);
        self.k = 3.0 / (16.0 / 15.0 * q * q * q.sqrt());
    }

    pub fn begin_stroke(&mut self) {
        self.last = None;
        self.travel = 0.0;
        self.last_segment = None;
        self.prev_dir = None;
        self.path.clear();
        self.tip = None;
        self.smoother.reset();
    }

    /// One pen sample (canvas pixels). Draws the path once it is a spacing
    /// long (without the brush front: see `preview`). Returns the bounds
    /// of what changed.
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        let p = self.smoother.apply(x, y, pressure, self.brush.radius as f32);
        let Some((lx, ly, _)) = self.last else {
            // Pen down: the path starts here.
            self.last = Some((x, y, p));
            self.pending = (x, y, p);
            self.path.push(PathPoint { arc: 0.0, cap: self.cap(p) });
            return None;
        };
        self.last = Some((x, y, p));
        self.travel += ((x - lx).powi(2) + (y - ly).powi(2)).sqrt();
        if self.travel < self.spacing { return None; }
        self.travel = 0.0;
        let from = std::mem::replace(&mut self.pending, (x, y, p));
        let restored = self.restore_tip(surface);
        union(restored, self.segment(surface, from, (x, y, p), Pass::Path))
    }

    /// Show the brush where the pen is: its round front past the path
    /// drawn so far (a whole dab before the pen moved). Temporary: the
    /// path to come decides what these pixels get for good. Called once
    /// the pending pen samples are painted, before showing the result
    /// (not after every sample: the front is half a dab to draw).
    pub fn preview<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        if self.tip.as_ref().is_some_and(|t| t.rect.0 < t.rect.2) { return None; }
        let (x, y, p) = self.last?;
        match self.last_segment {
            Some((a, b)) => {
                let rect = self.front_rect(surface, a, b);
                self.save_tip(surface, rect);
                union(self.segment(surface, a, b, Pass::Front), Some(rect))
            }
            None => {
                let rect = self.dab_rect(surface, x, y);
                self.save_tip(surface, rect);
                union(self.dab(surface, x, y, p), Some(rect))
            }
        }
    }

    /// Pen lifted: the rest of the path, and its round end for good.
    pub fn end_stroke<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        let end = self.last.take()?;
        let mut dirty = self.restore_tip(surface);
        self.tip = None;
        if self.travel > 0.0 {
            let from = self.pending;
            dirty = union(dirty, self.segment(surface, from, end, Pass::Path));
        }
        match self.last_segment {
            Some((a, b)) => union(dirty, self.segment(surface, a, b, Pass::Front)),
            // A click: one dab.
            None => union(dirty, self.dab(surface, end.0, end.1, end.2)),
        }
    }

    /// Opacity of the stroke at pen pressure `p`, in mask units (0..255).
    fn cap(&self, p: f32) -> f32 {
        (self.brush.opacity * p).clamp(0.0, 1.0) * 255.0
    }

    /// Opacity of the path drawn so far, `k` px (k = 0, 1, 2...) behind
    /// its end, into `self.behind` (up to a reach back).
    fn fill_behind(&mut self) {
        let n = self.reach.ceil() as usize + 2;
        self.behind.clear();
        let Some(end) = self.path.last().map(|p| p.arc) else { return };
        let mut seg = self.path.len() - 1;
        for k in 0..n {
            let arc = end - k as f32;
            while seg > 0 && self.path[seg - 1].arc > arc { seg -= 1; }
            let v = if seg == 0 {
                self.path[0].cap
            } else {
                let (a0, c0) = (self.path[seg - 1].arc, self.path[seg - 1].cap);
                let (a1, c1) = (self.path[seg].arc, self.path[seg].cap);
                c0 + (c1 - c0) * ((arc - a0) / (a1 - a0).max(1e-6)).clamp(0.0, 1.0)
            };
            self.behind.push(v);
        }
    }

    /// Bounds of a whole dab at (x, y), clipped to the surface.
    fn dab_rect<S: PaintSurface>(&self, surface: &S, x: f32, y: f32) -> Rect {
        clip_rect(surface, x - self.reach, y - self.reach, x + self.reach, y + self.reach)
    }

    /// Bounds of the half dab ahead of `b` (going from `a`).
    fn front_rect<S: PaintSurface>(&self, surface: &S, a: (f32, f32, f32), b: (f32, f32, f32)) -> Rect {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let (ux, uy) = (dx / len * self.reach, dy / len * self.reach);
        let pts = [(b.0 - uy, b.1 + ux), (b.0 + uy, b.1 - ux), (b.0 + ux - uy, b.1 + uy + ux), (b.0 + ux + uy, b.1 + uy - ux)];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y);
        }
        clip_rect(surface, x0, y0, x1, y1)
    }

    /// Keep what `rect` holds now (stroke coverage and layer pixels), to
    /// put it back before the next piece of path: the brush front drawn
    /// there meanwhile is only a preview.
    fn save_tip<S: PaintSurface>(&mut self, surface: &mut S, rect: Rect) {
        if rect.0 >= rect.2 || rect.1 >= rect.3 { self.tip = None; return; }
        surface.prepare(rect);
        let w = surface.width() as usize;
        let (pixels, _, mask) = surface.surfaces_mut();
        let mut tip = self.tip.take().unwrap_or_default();
        tip.rect = rect;
        tip.mask.clear();
        tip.pixels.clear();
        for y in rect.1..rect.3 {
            let (i0, i1) = (y as usize * w + rect.0 as usize, y as usize * w + rect.2 as usize);
            tip.mask.extend_from_slice(&mask[i0..i1]);
            tip.pixels.extend_from_slice(&pixels[i0 * 4..i1 * 4]);
        }
        self.tip = Some(tip);
    }

    /// Put back what the brush front preview covered.
    fn restore_tip<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        let tip = self.tip.as_mut()?;
        let rect = std::mem::take(&mut tip.rect);
        if rect.0 >= rect.2 { return None; }
        let w = surface.width() as usize;
        let rw = (rect.2 - rect.0) as usize;
        let (pixels, _, mask) = surface.surfaces_mut();
        for (k, y) in (rect.1..rect.3).enumerate() {
            let i0 = y as usize * w + rect.0 as usize;
            mask[i0..i0 + rw].copy_from_slice(&tip.mask[k * rw..(k + 1) * rw]);
            pixels[i0 * 4..(i0 + rw) * 4].copy_from_slice(&tip.pixels[k * rw * 4..(k + 1) * rw * 4]);
        }
        Some(rect)
    }

    /// One piece of the path, from `a` to `b` (x, y, pressure).
    ///
    /// `Pass::Path`: the pixels up to the line across `b`. A pixel the pen
    /// reaches for the first time (past the line across `a`) gets the
    /// brush of all the path behind it (taken as straight, as the front
    /// preview did) and of this segment; a pixel behind gets what this
    /// segment adds. It moves toward the opacity of the path at its
    /// closest point: the pressure interpolated along the segment, or what
    /// the pressure was back there (so a rising pressure doesn't lift the
    /// round area behind the pen: that left a ring every spacing).
    ///
    /// `Pass::Front`: the pixels past `b`, with the brush of all the path
    /// behind them, at the pressure of `b`: the round front of the brush.
    fn segment<S: PaintSurface>(&mut self, surface: &mut S, a: (f32, f32, f32), b: (f32, f32, f32), pass: Pass) -> Option<Rect> {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 { return None; }
        let front = pass == Pass::Front;
        // Opacity at both ends, in mask units (0..255).
        let (cap_a, cap_b) = (self.cap(a.2), self.cap(b.2));
        // Path length up to `a`.
        let arc_a = if front {
            self.path.last().map_or(0.0, |p| p.arc) - len
        } else {
            if let Some((pa, pb)) = self.last_segment {
                let (ddx, ddy) = (pb.0 - pa.0, pb.1 - pa.1);
                let l = (ddx * ddx + ddy * ddy).sqrt().max(1e-6);
                self.prev_dir = Some((ddx / l, ddy / l));
            }
            self.fill_behind();
            let arc_a = self.path.last().map_or(0.0, |p| p.arc);
            self.path.push(PathPoint { arc: arc_a + len, cap: cap_b });
            // Keep about a reach of path (plus the entry before it).
            while self.path.len() > 2 && self.path[1].arc < arc_a - self.reach { self.path.remove(0); }
            self.last_segment = Some((a, b));
            arc_a
        };
        let behind = &self.behind[..];
        let cap_max = if front { cap_b } else { behind.iter().copied().fold(cap_a.max(cap_b), f32::max) };
        let cap_max = unit_to_u8(cap_max / 255.0);
        if cap_max == 0 { return None; }

        let (w, h) = (surface.width() as i32, surface.height() as i32);
        let reach = self.reach;
        let x0 = ((a.0.min(b.0) - reach).floor() as i32).max(0);
        let y0 = ((a.1.min(b.1) - reach).floor() as i32).max(0);
        let x1 = ((a.0.max(b.0) + reach).ceil() as i32 + 1).min(w);
        let y1 = ((a.1.max(b.1) + reach).ceil() as i32 + 1).min(h);
        if x0 >= x1 || y0 >= y1 { return None; }
        surface.prepare((x0 as u32, y0 as u32, x1 as u32, y1 as u32));

        // Everything in units of the radius from here.
        let inv_r = 1.0 / reach;
        let (ux, uy) = (dx / len, dy / len);
        // The line across `a` is where the previous segment's pixels end.
        let (upx, upy) = if front { (ux, uy) } else { self.prev_dir.unwrap_or((ux, uy)) };
        let (len_n, arc_n) = (len * inv_r, arc_a * inv_r);
        let behind_n = (behind.len().max(1) - 1) as f32;
        let k = self.k;
        let sw = w as usize;
        let (pixels, base, mask) = surface.surfaces_mut();
        let (color, blend) = (self.brush.color, self.brush.blend);
        let (mut wx0, mut wy0, mut wx1, mut wy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);

        for py in y0..y1 {
            let cy = py as f32 + 0.5;
            // The pixels of this pass on the row: the capsule (the disk
            // around `b` for the front), on the right side of the line
            // across `b`.
            let span = if front { capsule_row_span((b.0, b.1), (b.0, b.1), reach, cy) } else { capsule_row_span((a.0, a.1), (b.0, b.1), reach, cy) };
            let Some((sx0, sx1)) = span else { continue };
            let (sx0, sx1) = clip_to_side(sx0, sx1, cy, (b.0, b.1), (ux, uy), front);
            let xa = (sx0.floor() as i32).max(x0);
            let xb = ((sx1.ceil() as i32) + 1).min(x1);
            if xa >= xb { continue; }
            let row = py as usize * sw;
            let ry = (cy - a.1) * inv_r;
            let (mut row_x0, mut row_x1) = (i32::MAX, i32::MIN);
            let mut x = xa;
            while x < xb {
                let n = ((xb - x) as usize).min(CHUNK);
                let i0 = row + x as usize;
                let mrow = &mut mask[i0..i0 + n];
                // Already at this segment's opacity: nothing to add.
                if min_u8(mrow) >= cap_max {
                    x += n as i32;
                    continue;
                }
                // Brush deposited on each pixel and where along the path
                // it is closest (plain arithmetic, vectorized)...
                let mut gs = [0.0f32; CHUNK];
                let mut alongs = [0.0f32; CHUNK];
                let rx0 = (x as f32 + 0.5 - a.0) * inv_r;
                for ((g, al), &j) in gs.iter_mut().zip(alongs.iter_mut()).zip(IOTA.iter()) {
                    let rx = rx0 + j * inv_r;
                    let along = rx * ux + ry * uy;
                    let across = ry * ux - rx * uy;
                    let q = 1.0 - across * across;
                    let sq = q.max(0.0).sqrt();
                    *al = along;
                    *g = k * if front {
                        // Past `b`: all the path behind, up to the start.
                        let d = along - len_n;
                        if d > 0.0 { f(q, sq, d + arc_n + len_n) - f(q, sq, d) } else { 0.0 }
                    } else if along <= len_n {
                        let own = f(q, sq, len_n - along) - f(q, sq, -along);
                        // First time the pen reaches the pixel: all the
                        // path before it too.
                        let along_p = rx * upx + ry * upy;
                        let across_p = ry * upx - rx * upy;
                        let qp = 1.0 - across_p * across_p;
                        let sqp = qp.max(0.0).sqrt();
                        let before = f(qp, sqp, along_p + arc_n) - f(qp, sqp, along_p);
                        own + if along_p >= 0.0 { before } else { 0.0 }
                    } else {
                        0.0
                    };
                }
                // ...the opacity it moves toward...
                let mut caps = [0.0f32; CHUNK];
                for (c, &along) in caps.iter_mut().zip(alongs.iter()) {
                    *c = if front {
                        cap_b
                    } else if along >= 0.0 {
                        cap_a + (cap_b - cap_a) * (along / len_n).min(1.0)
                    } else {
                        behind[((-along * reach) as i32).clamp(0, behind_n as i32) as usize]
                    };
                }
                // ...and the wash step (vectorized).
                let mut new = [0u8; CHUNK];
                let new = &mut new[..n];
                for (((nv, &cur), &g), &cap) in new.iter_mut().zip(mrow.iter()).zip(gs.iter()).zip(caps.iter()) {
                    let cap = (cap + 0.5) as i32 as f32;
                    let gain = (cap - cur as f32).max(0.0) * one_minus_exp_neg(g.max(0.0));
                    *nv = cur + (gain + 0.5) as i32 as u8;
                }
                let Some((c0, c1)) = changed_span(mrow, new) else {
                    x += n as i32;
                    continue;
                };
                mrow[c0..c1].copy_from_slice(&new[c0..c1]);
                let (p0, p1) = ((i0 + c0) * 4, (i0 + c1) * 4);
                write_pixels(&mut pixels[p0..p1], &base[p0..p1], &new[c0..c1], color, blend);
                row_x0 = row_x0.min(x + c0 as i32);
                row_x1 = row_x1.max(x + c1 as i32);
                x += n as i32;
            }
            if row_x0 < row_x1 {
                wx0 = wx0.min(row_x0);
                wx1 = wx1.max(row_x1);
                wy0 = wy0.min(py);
                wy1 = wy1.max(py + 1);
            }
        }
        (wx0 < wx1).then_some((wx0 as u32, wy0 as u32, wx1 as u32, wy1 as u32))
    }

    /// One whole dab (a click, or the brush before the pen moves): the
    /// cross-section of a stroke, turned around its center.
    fn dab<S: PaintSurface>(&self, surface: &mut S, cx: f32, cy: f32, pressure: f32) -> Option<Rect> {
        let cap = unit_to_u8(self.cap(pressure) / 255.0);
        if cap == 0 { return None; }
        let rect = self.dab_rect(surface, cx, cy);
        if rect.0 >= rect.2 || rect.1 >= rect.3 { return None; }
        surface.prepare(rect);
        let sw = surface.width() as usize;
        let (pixels, base, mask) = surface.surfaces_mut();
        let (color, blend) = (self.brush.color, self.brush.blend);
        let inv_r = 1.0 / self.reach;
        let kk = 16.0 / 15.0 * self.k;
        let capf = cap as f32;
        let (mut wx0, mut wy0, mut wx1, mut wy1) = (u32::MAX, u32::MAX, 0, 0);
        for py in rect.1..rect.3 {
            let dy = (py as f32 + 0.5 - cy) * inv_r;
            let row = py as usize * sw;
            let mut x = rect.0;
            while x < rect.2 {
                let n = ((rect.2 - x) as usize).min(CHUNK);
                let i0 = row + x as usize;
                let mrow = &mut mask[i0..i0 + n];
                let dx0 = (x as f32 + 0.5 - cx) * inv_r;
                let mut new = [0u8; CHUNK];
                let new = &mut new[..n];
                for ((nv, &cur), &j) in new.iter_mut().zip(mrow.iter()).zip(IOTA.iter()) {
                    let dx = dx0 + j * inv_r;
                    let q = (1.0 - dx * dx - dy * dy).max(0.0);
                    let g = kk * q * q * q.sqrt();
                    let gain = (capf - cur as f32).max(0.0) * one_minus_exp_neg(g);
                    *nv = cur + (gain + 0.5) as i32 as u8;
                }
                if let Some((c0, c1)) = changed_span(mrow, new) {
                    mrow[c0..c1].copy_from_slice(&new[c0..c1]);
                    let (p0, p1) = ((i0 + c0) * 4, (i0 + c1) * 4);
                    write_pixels(&mut pixels[p0..p1], &base[p0..p1], &new[c0..c1], color, blend);
                    wx0 = wx0.min(x + c0 as u32);
                    wx1 = wx1.max(x + c1 as u32);
                    wy0 = wy0.min(py);
                    wy1 = wy1.max(py + 1);
                }
                x += n as u32;
            }
        }
        (wx0 < wx1).then_some((wx0, wy0, wx1, wy1))
    }
}

impl Default for RoundBrush {
    fn default() -> Self { Self::new() }
}

/// ∫0..t (q - v²)² dv for |t| up to √q (where the kernel ends), with
/// `sq` = √q: the brush a pixel at squared distance 1 - q (in radii)
/// from a line gets from the line's points 0..t, divided by K.
#[inline(always)]
fn f(q: f32, sq: f32, t: f32) -> f32 {
    let t = t.min(sq).max(-sq);
    let t2 = t * t;
    t * (q * q - (2.0 / 3.0) * q * t2 + 0.2 * t2 * t2)
}

/// 1 - e^-g for g >= 0, without a libm call (2^x from the float bits
/// and a polynomial; error ~1e-7).
#[inline(always)]
fn one_minus_exp_neg(g: f32) -> f32 {
    // y = -g·log2(e) in [-60, 0], shifted to be positive so `as i32`
    // floors it.
    let y = (-g * std::f32::consts::LOG2_E).max(-60.0) + 64.0;
    let i = y as i32;
    let f = y - i as f32;
    let p = 1.0 + f * (std::f32::consts::LN_2 + f * (0.240_226_5 + f * (0.055_504_11 + f * (0.009_618_13 + f * 0.001_333_36))));
    let scale = f32::from_bits(((i + 63) as u32) << 23);
    1.0 - p * scale
}

/// One end of a drawn segment.
#[derive(Clone, Copy)]
struct PathPoint {
    /// Path length from the start of the stroke.
    arc: f32,
    /// Opacity there, mask units.
    cap: f32,
}

/// Which pixels of a segment to draw (see `segment`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Path,
    Front,
}

/// Saved stroke coverage and layer pixels under the brush front preview.
#[derive(Default)]
struct Tip {
    rect:   Rect,
    mask:   Vec<u8>,
    pixels: Vec<u8>,
}

/// The part of [x0, x1] on row `y` past the line through `p` across `u`
/// (`ahead`), or before it, with a pixel of margin (the exact test is made
/// per pixel).
fn clip_to_side(x0: f32, x1: f32, y: f32, p: (f32, f32), u: (f32, f32), ahead: bool) -> (f32, f32) {
    if u.0.abs() < 1e-4 { return (x0, x1); }
    // Where (x - p.x)·ux + (y - p.y)·uy = 0 on this row.
    let x = p.0 - (y - p.1) * u.1 / u.0;
    if ahead == (u.0 > 0.0) { (x0.max(x - 1.0), x1) } else { (x0, x1.min(x + 1.0)) }
}

fn clip_rect<S: PaintSurface>(surface: &S, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    let (w, h) = (surface.width() as f32, surface.height() as f32);
    let c = |v: f32, m: f32| v.clamp(0.0, m) as u32;
    (c(x0.floor(), w), c(y0.floor(), h), c(x1.ceil() + 1.0, w), c(y1.ceil() + 1.0, h))
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: u32 = 256;

    struct T { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for T {
        fn width(&self) -> u32 { N }
        fn height(&self) -> u32 { N }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    fn white() -> T {
        let px = vec![255u8; (N * N * 4) as usize];
        T { px: px.clone(), base: px, mask: vec![0; (N * N) as usize] }
    }

    fn red_at(s: &T, x: u32, y: u32) -> u8 { s.px[((y * N + x) * 4) as usize] }

    fn engine(radius: u32, hardness: f32) -> RoundBrush {
        let mut b = Brush::new();
        b.radius = radius;
        b.hardness = hardness;
        let mut e = RoundBrush::new();
        e.smoother.span = 0.0;
        e.sync_from(&b);
        e.begin_stroke();
        e
    }

    fn line(e: &mut RoundBrush, s: &mut T, from: f32, to: f32, y: f32, p: f32) {
        let steps = 60;
        for i in 0..=steps {
            let x = from + (to - from) * i as f32 / steps as f32;
            e.stroke_to(s, x, y, p);
        }
    }

    #[test]
    fn pressure_gives_matching_gray() {
        // 20 % pressure, black: the middle of the stroke is 20 % gray (204).
        let mut s = white();
        let mut e = engine(10, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 204).abs() <= 2, "expected ~204, got {v}");
    }

    #[test]
    fn crossing_the_same_stroke_does_not_build_up() {
        let mut s = white();
        let mut e = engine(10, 0.9);
        line(&mut e, &mut s, 40.0, 200.0, 128.0, 0.2);
        line(&mut e, &mut s, 200.0, 40.0, 128.0, 0.3);
        let v = red_at(&s, 120, 128) as i32;
        assert!((v - 178).abs() <= 2, "expected ~178 (30 %), got {v}");
    }

    #[test]
    fn stroke_reaches_the_end_point() {
        let mut s = white();
        let mut e = engine(40, 0.9);
        line(&mut e, &mut s, 40.0, 197.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        assert!(red_at(&s, 197, 128) < 10, "end not painted: {}", red_at(&s, 197, 128));
    }

    #[test]
    fn soft_crossing_has_no_crease() {
        // Two soft strokes crossing at a right angle: along the diagonal
        // through the crossing, values must change smoothly (the max of two
        // gradients makes a kink there, seen as a light line).
        let mut s = white();
        let mut e = engine(40, 0.0);
        line(&mut e, &mut s, 20.0, 236.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        e.begin_stroke();
        for i in 0..=60 {
            e.stroke_to(&mut s, 128.0, 20.0 + 216.0 * i as f32 / 60.0, 1.0);
        }
        e.end_stroke(&mut s);
        // Along a row in the soft edge of the horizontal stroke, crossing
        // the vertical one: a per-pixel max keeps the row flat, then bends
        // sharply where the vertical stroke takes over (a crease, seen as a
        // light line). Wash accumulation bends smoothly.
        let v: Vec<i32> = (0..60).map(|k| 255 - red_at(&s, 128 + k, 160) as i32).collect();
        let worst = v.windows(3).map(|w| (w[0] - 2 * w[1] + w[2]).abs()).max().unwrap();
        assert!(worst <= 8, "kink along the row: {worst} ({v:?})");
    }

    #[test]
    fn big_brush_draws_long_segments() {
        let e = engine(500, 0.5);
        assert!(e.spacing > 40.0, "spacing {}", e.spacing);
    }

    #[test]
    fn soft_stroke_has_no_beads() {
        // A big soft stroke at steady pressure: along its length, the
        // values must not ripple with the spacing.
        let mut s = white();
        let mut e = engine(60, 0.0);
        line(&mut e, &mut s, 20.0, 236.0, 128.0, 0.8);
        e.end_stroke(&mut s);
        for y in [128, 150, 170] {
            let row: Vec<i32> = (90..170).map(|x| red_at(&s, x, y) as i32).collect();
            let (lo, hi) = (row.iter().min().unwrap(), row.iter().max().unwrap());
            assert!(hi - lo <= 2, "row {y} ripples: {row:?}");
        }
    }

    #[test]
    fn hardness_sets_where_the_fade_starts() {
        // Hardness 50 %, radius 40: about opaque out to 20 px from the
        // stroke's middle, then fading out to 40.
        let mut s = white();
        let mut e = engine(40, 0.5);
        line(&mut e, &mut s, 20.0, 236.0, 128.0, 1.0);
        e.end_stroke(&mut s);
        let at = |d: u32| 255 - red_at(&s, 128, 128 + d) as i32;
        assert!(at(0) >= 250 && at(18) >= 235, "core {} {}", at(0), at(18));
        assert!(at(30) > 30 && at(30) < 200, "fade {}", at(30));
        assert!(at(41) < 3, "edge {}", at(41));
    }
}

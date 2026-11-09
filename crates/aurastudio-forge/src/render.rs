//! Dab rasterization: a dab (after dynamics) to its coverage over a
//! rectangle, for computed and sampled tips, with noise, per-tip texture,
//! wet edges, the Dual Brush's and the stroke's masks. From PhotoCraft's
//! brush engine (see NOTICE.md); the stroke buffer is in `stroke.rs`.

use std::sync::Arc;

use aurastudio_core::Rect;

use crate::brush::{BrushSettings, MaskMode, Pattern, TipShape};
use crate::dynamics::grid_cell;
use crate::rng::hash2;
use crate::tile::{Mips, PatternImage};
use crate::{Dab, TipProfile};

/// Edge of a stroke-buffer tile.
pub const COV_TILE: i32 = 64;

/// Work on at least this many pixels runs on several cores (rows of a
/// dab, tiles of a stroke): below it the threads cost more than they save.
/// Every stage is independent per row or tile: the pixels come out the
/// same either way.
pub(crate) const PARALLEL_MIN_PIXELS: usize = 256 * 256;

#[inline]
pub(crate) fn parallel(pixels: usize) -> bool { pixels >= PARALLEL_MIN_PIXELS }

/// `f` over `items`, collecting its results in order, on several cores when `pixels` are worth it.
pub(crate) fn map_items<T: Sync, R: Send>(items: &[T], pixels: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if parallel(pixels) {
        use rayon::prelude::*;
        return items.par_iter().map(&f).collect();
    }
    items.iter().map(&f).collect()
}

/// Computed tips with a smaller radius (in pixels) are rasterised by area, not at pixel centres.
const SMALL_TIP_RADIUS: f32 = 3.0;
/// Sub-pixel sample offsets (a 4×4 grid) for the area coverage of small tips.
const SUBPIXEL: [f32; 4] = [-0.375, -0.125, 0.125, 0.375];
const NOISE_SALT: u64 = 0x006E_6F69_7365;

#[inline]
/// Where an aliased (Pencil) dab of `diameter` pixels centred near (`x`, `y`) lands on the pixel
/// grid: the centre of the pixel holding the point for odd diameters, the nearest pixel corner
/// for even ones. Its footprint is then a whole-pixel block around that point, the same square
/// the Pencil cursor shows ([`grid_square`]).
pub fn grid_center(x: f64, y: f64, diameter: f32) -> (f32, f32) {
    let n = if diameter.is_finite() { diameter.round().max(1.0) as i64 } else { 1 };
    let snap = |v: f64| if n % 2 == 1 { grid_cell(v, 0.5) } else { v.round() };
    (snap(x) as f32, snap(y) as f32)
}

/// Wet Edges: how much paint stays at a pixel whose nearest dab reaches it at `rn` (0 at a dab's
/// centre, 1 at its rim). The interior keeps half the paint, the rim all of it.
#[inline]
fn wet_factor(rn: f32) -> f32 {
    0.5 + 0.5 * aurastudio_core::math::smoothstep((rn - 0.5) * 2.0)
}

/// Combine brush coverage `v` with a mask value `t` (texture/dual tip) at `depth`.
/// Every mode maps `v = 0` to 0 (a mask never adds paint where the brush has none).
pub fn mask_combine(mode: MaskMode, v: f32, t: f32, d: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * d;
    let r = match mode {
        MaskMode::Multiply => v * (1.0 - d + d * t),
        MaskMode::Subtract => (v - d * (1.0 - t)).max(0.0),
        MaskMode::Darken => v.min(1.0 - d + d * t),
        MaskMode::Overlay => {
            let o = if v < 0.5 { 2.0 * v * t } else { 1.0 - 2.0 * (1.0 - v) * (1.0 - t) };
            lerp(v, o)
        }
        MaskMode::ColorDodge => lerp(v, (v / (1.0 - t).max(1e-3)).min(1.0)),
        MaskMode::ColorBurn => lerp(v, 1.0 - ((1.0 - v) / t.max(1e-3)).min(1.0)),
        MaskMode::LinearBurn => lerp(v, (v + t - 1.0).max(0.0)),
        MaskMode::HardMix => lerp(v, if v + t > 1.0 { 1.0 } else { 0.0 }),
        MaskMode::LinearHeight => v * ((t - (1.0 - d * v)) * 4.0 + 0.5).clamp(0.0, 1.0),
        MaskMode::Height => {
            if t >= 1.0 - d * v {
                v
            } else {
                0.0
            }
        }
    };
    r.clamp(0.0, 1.0)
}

/// How far a Height-mode dab at full depth and coverage reaches into the pattern, in pattern
/// ranges (measured against Photoshop).
const HEIGHT_REACH: f32 = 12.0;

/// Combine brush coverage `v` with a Texture pattern value `t` at `depth` (Brush Settings ›
/// Texture). Where it differs from [`mask_combine`] (still used for the Dual Brush) it follows
/// Photoshop as measured with single dabs over a grey ramp:
/// - Subtract takes `depth × t` away, so a white pattern removes the most paint.
/// - Color Dodge and Color Burn move the pattern toward their neutral colour as depth falls.
/// - Height treats the pattern as a height field the brush presses into: the paint reaches
///   `12 × depth × v` and fills whatever lies below that, so a low depth paints only the dark
///   valleys, from about 17 % the whole pattern is under the paint, and a brighter pattern
///   takes paint away.
/// - Linear Height is Height, but where the reach stays below the top of the pattern the paint
///   fades with the pattern instead of being cut off.
pub fn texture_combine(mode: MaskMode, v: f32, t: f32, d: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let r = match mode {
        MaskMode::Subtract => v - d * t,
        MaskMode::ColorDodge => v / (1.0 - d * t).max(1e-3),
        MaskMode::ColorBurn => 1.0 - (1.0 - v) / (1.0 - d + d * t).max(1e-3),
        // Photoshop steps the depth in 1/255ths here, which shows at the lowest depths.
        MaskMode::Height => HEIGHT_REACH * (d * 255.0).round() / 255.0 * v - t,
        MaskMode::LinearHeight => {
            let reach = HEIGHT_REACH * (d * 255.0).round() / 255.0 * v;
            if reach < 1.0 { reach * (1.0 - t) } else { reach - t }
        }
        _ => mask_combine(mode, v, t, d),
    };
    if r.is_finite() { r.clamp(0.0, 1.0) } else { 0.0 }
}

/// A stroke-ready brush: the settings plus prepared tip mips and texture.
#[derive(Clone, Debug)]
pub struct BrushContext {
    pub brush: BrushSettings,
    tip: Option<Arc<Mips>>,
    dual_tip: Option<Arc<Mips>>,
    texture: Option<Arc<PatternImage>>,
}

fn prepare_pattern(b: &BrushSettings) -> Option<Arc<PatternImage>> {
    let t = &b.texture;
    if !t.enabled {
        return None;
    }
    let mut img = match &t.pattern {
        Pattern::Procedural { style, size, seed } => crate::procedural::pattern(*style, *size, *seed),
        p => match p.bitmap() {
            Some(g) if g.is_valid() => PatternImage { width: g.width as usize, height: g.height as usize, data: g.to_f32() },
            _ => return None,
        },
    };
    for v in &mut img.data {
        *v = t.level(*v);
    }
    Some(Arc::new(img))
}

/// The pixel rectangle `reach` (plus 1 px of anti-aliasing slack) around a dab's centre. The casts
/// saturate and the arithmetic saturates, so a far or non-finite centre or reach from a direct
/// caller gives a (possibly empty) rectangle instead of an `i32` overflow.
pub(crate) fn rect_around(d: &Dab, reach: f32) -> Rect {
    let rr = (reach.ceil() as i32).saturating_add(1);
    let (cx, cy) = (d.center.x.floor() as i32, d.center.y.floor() as i32);
    Rect::new(cx.saturating_sub(rr), cy.saturating_sub(rr), cx.saturating_add(rr).saturating_add(1), cy.saturating_add(rr).saturating_add(1))
}

impl BrushContext {
    /// The tip mips (main and dual) and the texture of the last stroke are
    /// kept: the same brush doesn't build them again at every pen-down.
    pub fn new(brush: &BrushSettings) -> Self {
        use std::sync::Mutex;
        type Kept<K, V> = Mutex<Vec<(K, Option<Arc<V>>)>>;
        static TIPS: Kept<TipShape, Mips> = Mutex::new(Vec::new());
        static TEXTURES: Kept<crate::brush::Texture, PatternImage> = Mutex::new(Vec::new());
        // The last `max`, newest last.
        fn kept<K: PartialEq + Clone, V>(list: &Kept<K, V>, max: usize, key: &K, make: impl FnOnce() -> Option<Arc<V>>) -> Option<Arc<V>> {
            let mut l = list.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = l.iter().position(|(k, _)| k == key) { let e = l.remove(i); let v = e.1.clone(); l.push(e); return v; }
            let v = make();
            l.push((key.clone(), v.clone()));
            if l.len() > max { l.remove(0); }
            v
        }
        let brush = brush.bounded_for_render();
        let mips = |t: &TipShape| kept(&TIPS, 2, t, || match t.bitmap() {
            Some(g) if g.is_valid() => Some(Arc::new(Mips::new(g))),
            _ => None,
        });
        let texture = if brush.texture.enabled { kept(&TEXTURES, 1, &brush.texture, || prepare_pattern(&brush)) } else { None };
        Self {
            tip: mips(&brush.tip),
            dual_tip: if brush.dual_brush.enabled { mips(&brush.dual_brush.tip) } else { None },
            texture,
            brush,
        }
    }

    /// Texture value at a document pixel (bilinear, tiled).
    #[inline]
    pub fn texture_at(&self, x: i32, y: i32) -> f32 {
        let Some(p) = &self.texture else { return 1.0 };
        let s = self.brush.texture.scale.max(0.01);
        if s == 1.0 {
            // Pixel centres land exactly on texels at 100 %.
            return p.data[y.rem_euclid(p.height as i32) as usize * p.width + x.rem_euclid(p.width as i32) as usize];
        }
        let (fx, fy) = ((x as f32 + 0.5) / s - 0.5, (y as f32 + 0.5) / s - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (p00, p10) = (p.sample_wrap(x0, y0), p.sample_wrap(x0 + 1.0, y0));
        let (p01, p11) = (p.sample_wrap(x0, y0 + 1.0), p.sample_wrap(x0 + 1.0, y0 + 1.0));
        let (a, b) = (p00 + (p10 - p00) * tx, p01 + (p11 - p01) * tx);
        a + (b - a) * ty
    }

    /// Bounding rectangle of a dab (rotated sampled tips need the corner reach).
    pub fn dab_rect(&self, d: &Dab, dual: bool) -> Rect {
        let sampled = if dual { self.dual_tip.is_some() } else { self.tip.is_some() };
        let hardness = if dual { self.brush.dual_brush.hardness } else { self.brush.hardness };
        // A computed soft tip paints past its nominal radius (see `tip_shape`).
        let aliased = self.brush.aliased && !dual;
        // A sampled tip: its own box (the longer side across the dab, the
        // other squashed by the roundness), turned by the angle. The
        // circle around the square (√2) was up to twice the pixels.
        let mips = if dual { self.dual_tip.as_deref() } else { self.tip.as_deref() };
        if let (true, Some((w0, h0, _))) = (sampled, mips.and_then(|m| m.levels.first())) {
            let r = d.radius;
            let big = (*w0).max(*h0).max(1) as f32;
            let ro = d.roundness.max(0.5 / r.max(1e-3)).min(1.0);
            let hu = r * (*w0 as f32 + 2.0) / big;
            let hv = r * ro * (*h0 as f32 + 2.0) / big;
            let (sn, cs) = d.angle.sin_cos();
            let (hx, hy) = (hu * cs.abs() + hv * sn.abs(), hu * sn.abs() + hv * cs.abs());
            if hx.is_finite() && hy.is_finite() {
                let (rx, ry) = ((hx.ceil() as i32).saturating_add(1), (hy.ceil() as i32).saturating_add(1));
                let (cx, cy) = (d.center.x.floor() as i32, d.center.y.floor() as i32);
                return Rect::new(cx.saturating_sub(rx), cy.saturating_sub(ry), cx.saturating_add(rx).saturating_add(1), cy.saturating_add(ry).saturating_add(1));
            }
        }
        let reach = if sampled {
            d.radius * std::f32::consts::SQRT_2
        } else if aliased {
            d.radius
        } else {
            crate::tip_reach(d.radius, hardness).max(d.radius)
        };
        rect_around(d, reach)
    }

    /// Rasterise a dab over `rect` into `out` (tip shape × noise × per-tip texture × wet edges × flow).
    /// Wet Edges shapes this one dab; a stroke applies it to its accumulated coverage instead.
    pub fn rasterize(&self, d: &Dab, dual: bool, rect: Rect, out: &mut Vec<f32>) {
        self.rasterize_into(d, dual, rect, out, None);
    }

    /// [`rasterize`](Self::rasterize); with `depth`, Wet Edges is left out of `out` and each
    /// painted pixel's depth inside the dab (`1 − rn`) goes to `depth`, for the stroke to apply it.
    pub fn rasterize_into(&self, d: &Dab, dual: bool, rect: Rect, out: &mut Vec<f32>, mut depth: Option<&mut Vec<f32>>) {
        let (w, h) = (rect.w() as usize, rect.h() as usize);
        out.clear();
        out.resize(w * h, 0.0);
        if let Some(dp) = depth.as_deref_mut() {
            dp.clear();
            dp.resize(w * h, 0.0);
        }
        let b = &self.brush;
        let (hardness, mips) = if dual { (b.dual_brush.hardness, self.dual_tip.as_deref()) } else { (b.hardness, self.tip.as_deref()) };
        let aliased = b.aliased && !dual;
        let wet = b.wet_edges && !dual && depth.is_none();
        let noise = b.noise && !dual;
        let tex_tip = !dual && b.texture.enabled && b.texture.each_tip && self.texture.is_some();
        let (cx, cy) = if aliased { grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
        let (sn, cs) = d.angle.sin_cos();
        let (fx, fy) = (if d.flip_x { -1.0 } else { 1.0 }, if d.flip_y { -1.0 } else { 1.0 });
        // Brush Projection: stretch the sampling coordinates along the tilt direction, which
        // foreshortens the tip there by `proj_scale`.
        let proj = (!dual && d.proj_scale < 0.999).then(|| {
            let (ps, pc) = d.proj_angle.sin_cos();
            (pc, ps, 1.0 / d.proj_scale.max(0.05) - 1.0)
        });
        let r = d.radius;
        let ro = d.roundness.max(0.5 / r).min(1.0);
        let rm = (r * ro).max(0.5);
        let profile = TipProfile::new(rm, hardness);
        let reach = if aliased || mips.is_some() { rm } else { profile.reach().max(rm) };
        let reach2 = (reach + 0.5) * (reach + 0.5);
        // Sampled-tip mapping.
        let (level, inv_scale, tw, th) = match mips {
            Some(m) => {
                let (w0, h0, _) = &m.levels[0];
                let big = *w0.max(h0) as f32;
                (m.level_for(2.0 * r * ro.max(0.25)), big / (2.0 * r), *w0 as f32, *h0 as f32)
            }
            None => (0, 1.0, 1.0, 1.0),
        };
        // A pixel offset from the dab centre in tip space: to y-up, project, rotate by -angle, flip.
        let to_tip = |dx: f32, dy: f32| {
            let (mut ux, mut uy) = (dx, -dy);
            if let Some((ax, ay, k)) = proj {
                let t = (ux * ax + uy * ay) * k;
                ux += t * ax;
                uy += t * ay;
            }
            ((ux * cs + uy * sn) * fx, (-ux * sn + uy * cs) * fy)
        };
        // A tip a few pixels across takes each pixel's covered area (a 4×4 grid inside it):
        // sampling only the pixel centre made a tiny dab's total coverage swing with its sub-pixel
        // position, and thin lines beaded.
        let small = mips.is_none() && !aliased && rm < SMALL_TIP_RADIUS;
        let small_reach2 = (reach + 0.75) * (reach + 0.75);
        // Keep ordinary computed tips separate from the feature loop, so their dense rows
        // can be vectorised without the per-pixel sampled-mask and stroke-depth branches.
        if !(mips.is_some() || small || (aliased && b.square) || noise || tex_tip || wet)
            && depth.is_none()
            && proj.is_none()
            // Rotation preserves a circle's distance; ellipses still need the transform.
            && (d.angle == 0.0 || ro == 1.0)
            && d.angle.is_finite()
            && d.radius.is_finite()
            && d.roundness.is_finite()
            && rm.is_finite()
            && cx.is_finite()
            && cy.is_finite()
            && hardness.is_finite()
            && d.alpha.is_finite()
            && w > 0
        {
            rasterize_round(d, rect, out, profile, aliased);
            return;
        }
        // One row of the dab; the rows on all cores for a big one (an
        // 800 px sampled tip is over a million pixels).
        let row = |yy: usize, orow: &mut [f32], mut drow: Option<&mut [f32]>| {
            let y = rect.y0 + yy as i32;
            let dy = y as f32 + 0.5 - cy;
            for (xx, slot) in orow.iter_mut().enumerate() {
                let x = rect.x0 + xx as i32;
                let dx = x as f32 + 0.5 - cx;
                let (u, v) = to_tip(dx, dy);
                let (mut val, rn) = match mips {
                    // Block: the whole-pixel square, hard (the Eraser's Block mode).
                    _ if aliased && b.square => {
                        if dx.abs() >= r || dy.abs() >= r {
                            continue;
                        }
                        (1.0, 0.0)
                    }
                    None if small => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= small_reach2 {
                            continue;
                        }
                        let mut sum = 0.0;
                        for sy in SUBPIXEL {
                            for sx in SUBPIXEL {
                                let (su, sv) = to_tip(dx + sx, dy + sy);
                                let sd = ((su * ro).powi(2) + sv * sv).sqrt();
                                // Each sample takes the tip's own edge, so the covered area
                                // changes smoothly with the dab's position.
                                sum += profile.falloff(sd);
                            }
                        }
                        (sum / (SUBPIXEL.len() * SUBPIXEL.len()) as f32, d2.sqrt() / rm)
                    }
                    None => {
                        let d2 = (u * ro).powi(2) + v * v;
                        if d2 >= reach2 {
                            continue;
                        }
                        let dd = d2.sqrt();
                        let val = if aliased { if dd <= rm { 1.0 } else { 0.0 } } else { profile.coverage(dd) };
                        (val, dd / rm)
                    }
                    Some(m) => {
                        let (px, py) = (u * inv_scale + tw / 2.0, -(v / ro) * inv_scale + th / 2.0);
                        if px < -1.0 || py < -1.0 || px > tw + 1.0 || py > th + 1.0 {
                            continue;
                        }
                        let s = m.sample_clamped(level, px / tw, py / th);
                        let val = if aliased { if s >= 0.5 { 1.0 } else { 0.0 } } else { s };
                        (val, 1.0 - s)
                    }
                };
                if val <= 0.0 {
                    continue;
                }
                if noise && val < 1.0 {
                    let n = hash2(x, y, NOISE_SALT);
                    let hard = if n < val { 1.0 } else { 0.0 };
                    val += (hard - val) * 0.7;
                }
                if tex_tip {
                    val = texture_combine(b.texture.mode, val, self.texture_at(x, y), d.depth);
                }
                if wet {
                    val *= wet_factor(rn);
                }
                if let Some(ds) = drow.as_deref_mut().and_then(|dp| dp.get_mut(xx)) {
                    *ds = (1.0 - rn).clamp(0.0, 1.0);
                }
                *slot = val * d.alpha;
            }
        };
        match depth {
            Some(dp) if parallel(w * h) => {
                use rayon::prelude::*;
                out.par_chunks_exact_mut(w).zip(dp.par_chunks_exact_mut(w)).enumerate().for_each(|(yy, (o, d))| row(yy, o, Some(d)));
            }
            Some(dp) => for (yy, (o, d)) in out.chunks_exact_mut(w).zip(dp.chunks_exact_mut(w)).enumerate() { row(yy, o, Some(d)) },
            None if parallel(w * h) => {
                use rayon::prelude::*;
                out.par_chunks_exact_mut(w).enumerate().for_each(|(yy, o)| row(yy, o, None));
            }
            None => for (yy, o) in out.chunks_exact_mut(w).enumerate() { row(yy, o, None) },
        }
    }


    /// The stroke has a texture laid over it as a whole (not tip by tip).
    pub(crate) fn stroke_texture(&self) -> bool { self.texture.is_some() && !self.brush.texture.each_tip }

    /// [`stroke_mask`](Self::stroke_mask) with Wet Edges from the stroke's depth at the pixel
    /// (the deepest any dab reaches it, see [`CoverageMap::depth`]).
    #[inline]
    pub fn stroke_mask_wet(&self, c: f32, depth: Option<f32>, dual: Option<f32>, x: i32, y: i32) -> f32 {
        let b = &self.brush;
        let mut m = match depth {
            Some(dp) => c * wet_factor(1.0 - dp),
            None => c,
        };
        if let Some(dv) = dual {
            m = mask_combine(b.dual_brush.mode, m, dv, 1.0);
        }
        if self.texture.is_some() && !b.texture.each_tip {
            m = texture_combine(b.texture.mode, m, self.texture_at(x, y), b.texture.depth.clamp(0.0, 1.0));
        }
        m
    }
}

// ---------------------------------------------------------------------------
// Sparse stroke buffer
// ---------------------------------------------------------------------------



// With zero angle, the transform only changes signs. For circles, any rotation preserves
// radial distance. Both cases can omit the transform (projection is excluded by the caller).
#[inline(never)]
fn rasterize_round(d: &Dab, rect: Rect, out: &mut [f32], profile: TipProfile, aliased: bool) {
    let (cx, cy) = if aliased { grid_center(d.center.x, d.center.y, 2.0 * d.radius) } else { (d.center.x as f32, d.center.y as f32) };
    let ro = d.roundness.max(0.5 / d.radius).min(1.0);
    let rm = (d.radius * ro).max(0.5);
    let w = rect.w() as usize;
    if w == 0 {
        return;
    }
    let reach = if aliased { rm } else { profile.reach().max(rm) };
    let tip = RoundTip { cx, cy, ro, rm, reach2: (reach + 0.5) * (reach + 0.5), profile, alpha: d.alpha, aliased, x0: rect.x0, y0: rect.y0 };
    // Rows are independent: a large tip's rows run in parallel, to the same values. The serial
    // path stays a plain loop over an inlined row: the same code as before the threads (a closure
    // passed to a helper cost a small tip half its rasterization time).
    if parallel(out.len()) {
        use rayon::prelude::*;
        out.par_chunks_exact_mut(w).enumerate().for_each(|(yy, row)| tip.row(yy, row));
        return;
    }
    for (yy, row) in out.chunks_exact_mut(w).enumerate() {
        tip.row(yy, row);
    }
}

/// A round tip's coverage, computed row by row (see [`rasterize_round`]).
#[derive(Clone, Copy)]
struct RoundTip {
    cx: f32,
    cy: f32,
    ro: f32,
    rm: f32,
    reach2: f32,
    profile: TipProfile,
    alpha: f32,
    aliased: bool,
    x0: i32,
    y0: i32,
}

impl RoundTip {
    /// Row `yy` of the tip's rectangle into `row`.
    #[inline(always)]
    fn row(self, yy: usize, row: &mut [f32]) {
        let dy = (self.y0 + yy as i32) as f32 + 0.5 - self.cy;
        if self.aliased {
            for (xx, dst) in row.iter_mut().enumerate() {
                let dx = (self.x0 + xx as i32) as f32 + 0.5 - self.cx;
                let d2 = (dx * self.ro).powi(2) + dy * dy;
                *dst = if d2 < self.reach2 && d2.sqrt() <= self.rm { self.alpha } else { 0.0 };
            }
        } else {
            for (xx, dst) in row.iter_mut().enumerate() {
                let dx = (self.x0 + xx as i32) as f32 + 0.5 - self.cx;
                let d2 = (dx * self.ro).powi(2) + dy * dy;
                let val = if d2 >= self.reach2 { 0.0 } else { self.profile.falloff(d2.sqrt()) };
                *dst = if val <= 0.0 { 0.0 } else { val * self.alpha };
            }
        }
    }
}


//! The stroke: dabs build up a private coverage buffer (sparse 64 px tiles,
//! f32, plus a color per pixel with Color Dynamics and a depth with Wet
//! Edges), then only the tiles that changed are painted onto the layer
//! through `core::PaintSurface`, from the pixels before the stroke.
//!
//! The buffer and its build-up are PhotoCraft's (see NOTICE.md); the
//! painting is AuraStudio's: runs from the storage (several cores for a
//! big brush), the coverage kept in the stroke's 8-bit mask (dithered: a
//! soft, low-flow stroke doesn't band), the selection as the mask's limit,
//! 8-bit integer blends for the modes the layers have.

use std::collections::HashSet;

use aurastudio_core::math::mul255;
use aurastudio_core::{PaintSurface, Rect, Rgba8, Span};
use rustc_hash::FxHashMap;

use crate::brush::BrushSettings;
use crate::dynamics::DabGenerator;
use crate::mode::{self, PaintMode};
use crate::render::{map_items, parallel, BrushContext, COV_TILE};
use crate::rng::hash2;
use crate::{Dab, StrokePoint};

/// Salt of the per-pixel dither of the coverage.
const DITHER_SALT: u64 = 0x0000_6469_7468_6572;

/// Pixels (all the dirty tiles together) from which a stroke is painted
/// on several cores.
const PARALLEL_PIXELS: usize = 128 * 128;

/// Coverage (and depth, colour) kept at 16 bits: 0..=65535 for 0..1, a
/// 65535th apart everywhere (half floats lose the small steps near full
/// coverage that low-flow build-up adds). Half the memory of f32.
#[inline(always)]
fn q16(v: f32) -> u16 { (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16 }

#[inline(always)]
fn f16(v: u16) -> f32 { v as f32 * (1.0 / 65535.0) }

#[derive(Clone, Debug)]
struct CovTile {
    cov: Vec<u16>,
    /// Wet Edges: the deepest any dab reaches each pixel (`1 − rn` of the nearest dab), empty
    /// without Wet Edges. The stroke's interior is deep, its rim shallow, whatever the dabs' overlap.
    depth: Vec<u16>,
    /// Premultiplied native colour channels (`nc` per pixel), empty without per-dab colour.
    col: Vec<u16>,
}

impl CovTile {
    fn new() -> Self {
        Self { cov: vec![0; (COV_TILE * COV_TILE) as usize], depth: Vec::new(), col: Vec::new() }
    }
}

/// Preserve both signed coordinates without collisions. FxHash mixes the packed word so
/// horizontal and vertical runs both distribute across the table's buckets.
#[inline]
fn tile_key((tx, ty): (i32, i32)) -> u64 {
    (u64::from(tx as u32) << 32) | u64::from(ty as u32)
}

#[inline]
fn tile_coords(key: u64) -> (i32, i32) {
    ((key >> 32) as i32, key as i32)
}

/// A sparse tiled coverage (+ optional colour) buffer.
#[derive(Clone, Debug, Default)]
pub struct CoverageMap {
    tiles: FxHashMap<u64, CovTile>,
    nc: usize,
    bounds: Rect,
    dirty: HashSet<(i32, i32)>,
}

impl CoverageMap {
    fn bytes(&self) -> usize {
        self.tiles.values().map(|t| (t.cov.capacity() + t.depth.capacity() + t.col.capacity()) * 2).sum()
    }

    pub fn new(color_channels: usize) -> Self {
        Self { nc: color_channels, ..Default::default() }
    }
    #[cfg(test)]
    pub fn bounds(&self) -> Rect {
        self.bounds
    }
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> f32 {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        self.tiles.get(&tile_key((tx, ty))).map_or(0.0, |t| f16(t.cov[((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize]))
    }

    /// Wet Edges depth at a pixel (see [`CovTile::depth`]); `None` when the stroke has none.
    #[inline]
    #[cfg(test)]
    pub fn depth(&self, x: i32, y: i32) -> Option<f32> {
        let (tx, ty) = (x.div_euclid(COV_TILE), y.div_euclid(COV_TILE));
        self.tiles.get(&tile_key((tx, ty))).and_then(|t| t.depth.get(((y - ty * COV_TILE) * COV_TILE + (x - tx * COV_TILE)) as usize).map(|v| f16(*v)))
    }

    /// Accumulate dab values over `rect`: flow builds up towards the dab's opacity ceiling
    /// (`c ← c + v·(ceil − c)`). `color` = native colour for per-dab colour. `depth` (Wet Edges, one
    /// value per pixel of `rect`) keeps the deepest any dab reaches each pixel.
    pub fn accumulate(&mut self, rect: Rect, vals: &[f32], ceil: f32, color: Option<&[f32]>, depth: Option<&[f32]>) {
        if rect.is_empty() {
            return;
        }
        self.bounds = self.bounds.union(&rect);
        let w = rect.w() as usize;
        let nc = self.nc;
        let (t0x, t0y) = (rect.x0.div_euclid(COV_TILE), rect.y0.div_euclid(COV_TILE));
        let (t1x, t1y) = ((rect.x1 - 1).div_euclid(COV_TILE), (rect.y1 - 1).div_euclid(COV_TILE));
        let want_col = color.is_some() && nc > 0;
        let dab = DabValues { vals, rect, w, ceil, color: if want_col { color } else { None }, depth, nc };
        // Iterate directly for small dabs: no work-list allocations or removing existing tiles.
        let keys = (t0y..=t1y).flat_map(|ty| {
            (t0x..=t1x).map(move |tx| {
                let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&rect);
                ((tx, ty), tr)
            })
        });
        {
            let n = (COV_TILE * COV_TILE) as usize;
            let count = (i64::from(t1x) - i64::from(t0x) + 1) as usize;
            let count = count.saturating_mul((i64::from(t1y) - i64::from(t0y) + 1) as usize);
            if parallel(count.saturating_mul(n)) {
                let keys: Vec<_> = keys.collect();
                let touched = map_items(&keys, count.saturating_mul(n), |(_, tr)| dab.touches(*tr));
                let keys: Vec<_> = keys.into_iter().zip(touched).filter_map(|(key, touched)| touched.then_some(key)).collect();
                if parallel(keys.len().saturating_mul(n)) {
                    use rayon::prelude::*;
                    // Only genuinely parallel work takes ownership of tiles out of the map.
                    let mut tiles: Vec<_> = keys
                        .into_iter()
                        .map(|(key, tr)| {
                            self.dirty.insert(key);
                            let tile = self.tiles.remove(&tile_key(key)).unwrap_or_else(CovTile::new);
                            (key, tr, tile)
                        })
                        .collect();
                    tiles.par_iter_mut().for_each(|(key, tr, tile)| {
                        dab.prepare(tile);
                        dab.tile(*tr, *key, tile);
                    });
                    for (key, _, tile) in tiles {
                        self.tiles.insert(tile_key(key), tile);
                    }
                } else {
                    self.accumulate_serial(keys.into_iter(), dab);
                }
                return;
            }
        }
        self.accumulate_serial(keys.filter(|(_, tr)| dab.touches(*tr)), dab);
    }

    fn accumulate_serial(&mut self, keys: impl Iterator<Item = ((i32, i32), Rect)>, dab: DabValues<'_>) {
        for (key, tr) in keys {
            let tile = self.tiles.entry(tile_key(key)).or_insert_with(CovTile::new);
            dab.prepare(tile);
            dab.tile(tr, key, tile);
            self.dirty.insert(key);
        }
    }

    fn take_dirty(&mut self) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = self.dirty.drain().collect();
        v.sort_unstable();
        v
    }


}

/// One dab's coverage (`vals` over `rect`, `w` wide) as [`CoverageMap::accumulate`] adds it to
/// a tile: the dab's opacity ceiling, its colour when the stroke has per-dab colour (`nc` channels),
/// its Wet Edges depth.
#[derive(Clone, Copy)]
struct DabValues<'a> {
    vals: &'a [f32],
    rect: Rect,
    w: usize,
    ceil: f32,
    color: Option<&'a [f32]>,
    depth: Option<&'a [f32]>,
    nc: usize,
}

impl DabValues<'_> {
    fn touches(self, tr: Rect) -> bool {
        (tr.y0..tr.y1).any(|y| {
            let row = (y - self.rect.y0) as usize * self.w;
            self.vals[row + (tr.x0 - self.rect.x0) as usize..row + (tr.x1 - self.rect.x0) as usize].iter().any(|&v| v > 0.0)
        })
    }

    fn prepare(self, tile: &mut CovTile) {
        let n = (COV_TILE * COV_TILE) as usize;
        if self.color.is_some() && tile.col.is_empty() {
            tile.col = vec![0; n * self.nc];
        }
        if self.depth.is_some() && tile.depth.is_empty() {
            tile.depth = vec![0; n];
        }
    }

    /// Adds the dab over `tr` (within tile `(tx, ty)`) to `tile`.
    #[inline(always)]
    fn tile(self, tr: Rect, (tx, ty): (i32, i32), tile: &mut CovTile) {
        let DabValues { vals, rect, w, ceil, color, depth, nc } = self;
        for y in tr.y0..tr.y1 {
            let row = (y - rect.y0) as usize * w;
            let trow = ((y - ty * COV_TILE) * COV_TILE) as usize;
            for x in tr.x0..tr.x1 {
                let v = vals[row + (x - rect.x0) as usize];
                if v <= 0.0 {
                    continue;
                }
                let ti = trow + (x - tx * COV_TILE) as usize;
                if let (Some(dv), Some(slot)) = (depth.and_then(|dp| dp.get(row + (x - rect.x0) as usize)), tile.depth.get_mut(ti)) {
                    *slot = (*slot).max(q16(*dv));
                }
                let c = f16(tile.cov[ti]);
                let nv = if ceil > c { c + v * (ceil - c) } else { c };
                if q16(nv) <= tile.cov[ti] {
                    continue;
                }
                if let Some(col) = color {
                    let k = if c < 1.0 { (nv - c) / (1.0 - c) } else { 0.0 };
                    let p = &mut tile.col[ti * nc..(ti + 1) * nc];
                    for i in 0..nc {
                        p[i] = q16(col[i] * k + f16(p[i]) * (1.0 - k));
                    }
                }
                tile.cov[ti] = q16(nv);
            }
        }
    }
}

/// One stroke: feed points, paint the changed tiles.
#[derive(Clone, Debug)]
pub struct StrokeRenderer {
    pub ctx: BrushContext,
    generator: DabGenerator,
    cov: CoverageMap,
    dual: Option<CoverageMap>,
    per_dab_color: bool,
    dabs_done: usize,
    scratch: Vec<f32>,
    /// Wet Edges depth of the dab being accumulated (see [`CovTile::depth`]).
    depth_scratch: Vec<f32>,
    dab_buf: Vec<Dab>,
    dual_buf: Vec<Dab>,
    all_dabs: Option<Vec<Dab>>,
}

impl StrokeRenderer {
    /// Bytes of its coverage (and dual brush) tiles.
    pub fn bytes(&self) -> usize { self.cov.bytes() + self.dual.as_ref().map_or(0, CoverageMap::bytes) }

    /// `zoom`: the view's, for smoothing.
    pub fn new(brush: &BrushSettings, zoom: f32) -> Self {
        let per_dab_color = brush.color_dynamics.enabled && !brush.erase;
        Self {
            ctx: BrushContext::new(brush),
            generator: DabGenerator::new(brush, zoom),
            cov: CoverageMap::new(if per_dab_color { 3 } else { 0 }),
            dual: brush.dual_brush.enabled.then(|| CoverageMap::new(0)),
            per_dab_color,
            dabs_done: 0,
            scratch: Vec::new(),
            depth_scratch: Vec::new(),
            dab_buf: Vec::new(),
            dual_buf: Vec::new(),
            all_dabs: None,
        }
    }

    /// Keep a copy of every primary dab (tests).
    #[cfg(test)]
    pub fn record_dabs(mut self) -> Self {
        self.all_dabs = Some(Vec::new());
        self
    }

    #[cfg(test)]
    pub fn dabs(&self) -> &[Dab] { self.all_dabs.as_deref().unwrap_or(&[]) }

    /// Union of the dab rectangles so far.
    pub fn bounds(&self) -> Rect { self.cov.bounds }

    fn raster_pending(&mut self) {
        let mut dabs = std::mem::take(&mut self.dab_buf);
        let mut duals = std::mem::take(&mut self.dual_buf);
        let wet = self.ctx.brush.wet_edges;
        for d in &dabs {
            let rect = self.ctx.dab_rect(d, false);
            // Wet Edges acts on the whole stroke: the dab records how deep it reaches each pixel
            // and the stroke darkens its own rim when painted, not every dab's.
            self.ctx.rasterize_into(d, false, rect, &mut self.scratch, wet.then_some(&mut self.depth_scratch));
            let rgb = [d.color[0], d.color[1], d.color[2]];
            let col = self.per_dab_color.then_some(&rgb[..]);
            self.cov.accumulate(rect, &self.scratch, d.opacity, col, wet.then_some(&self.depth_scratch[..]));
            // Bounds track the full dab rectangle (even fully transparent parts).
            self.cov.bounds = self.cov.bounds.union(&rect);
        }
        if let Some(dm) = self.dual.as_mut() {
            for d in &duals {
                let rect = self.ctx.dab_rect(d, true);
                self.ctx.rasterize(d, true, rect, &mut self.scratch);
                dm.accumulate(rect, &self.scratch, 1.0, None, None);
            }
        }
        self.dabs_done += dabs.len();
        if let Some(all) = self.all_dabs.as_mut() { all.extend_from_slice(&dabs); }
        dabs.clear();
        duals.clear();
        self.dab_buf = dabs;
        self.dual_buf = duals;
    }

    /// Feed input points (any chunking gives the same result).
    pub fn push(&mut self, pts: &[StrokePoint]) {
        self.generator.push(pts, &mut self.dab_buf, &mut self.dual_buf);
        self.raster_pending();
    }

    /// End of stroke (smoothing catch-up, a lone first point).
    pub fn finish(&mut self) {
        self.generator.finish(&mut self.dab_buf, &mut self.dual_buf);
        self.raster_pending();
    }

    /// Paint the coverage tiles changed since the last call (`all`: every
    /// tile) onto `s`. Returns the area painted.
    pub fn paint<S: PaintSurface<Rgba8>>(&mut self, s: &mut S, all: bool) -> Rect {
        let keys: Vec<(i32, i32)> = if all {
            self.cov.dirty.clear();
            let mut k: Vec<_> = self.cov.tiles.keys().copied().map(tile_coords).collect();
            k.sort_unstable();
            k
        } else {
            self.cov.take_dirty()
        };
        if let Some(d) = self.dual.as_mut() { d.dirty.clear(); }
        if keys.is_empty() { return Rect::EMPTY; }
        let bounds = self.bounds();
        let p = Painter::new(self);
        // Big strokes: every dirty tile's runs at once, on all cores.
        let rects: Vec<Rect> = keys.iter()
            .map(|&(tx, ty)| Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&bounds))
            .filter(|r| !r.is_empty() && self.cov.tiles.contains_key(&tile_key((r.x0.div_euclid(COV_TILE), r.y0.div_euclid(COV_TILE)))))
            .collect();
        let area: usize = rects.iter().map(|r| r.area() as usize).sum();
        if area >= PARALLEL_PIXELS {
            let mut v: Vec<Span<'_, Rgba8>> = Vec::new();
            if s.spans_in(&rects, &mut v) {
                use rayon::prelude::*;
                let tiles = &self.cov.tiles;
                v.par_iter_mut().with_min_len(16).for_each(|sp| {
                    let (tx, ty) = (sp.x.div_euclid(COV_TILE), sp.y.div_euclid(COV_TILE));
                    if let Some(tile) = tiles.get(&tile_key((tx, ty))) {
                        p.span(&TileRef { tile, x0: tx * COV_TILE, y0: ty * COV_TILE }, sp);
                    }
                });
                return rects.iter().fold(Rect::EMPTY, |a, r| a.union(r));
            }
        }
        let mut done = Rect::EMPTY;
        let mut spans: Vec<Span<'_, Rgba8>> = Vec::new();
        for (tx, ty) in keys {
            let Some(tile) = self.cov.tiles.get(&tile_key((tx, ty))) else { continue };
            let tr = Rect::new(tx * COV_TILE, ty * COV_TILE, (tx + 1) * COV_TILE, (ty + 1) * COV_TILE).intersect(&bounds);
            if tr.is_empty() { continue; }
            s.prepare(tr);
            done = done.union(&tr);
            let t = TileRef { tile, x0: tx * COV_TILE, y0: ty * COV_TILE };
            let mut v = aurastudio_core::surface::recycle_spans(std::mem::take(&mut spans));
            if s.spans_mut(tr, &mut v) {
                for sp in &mut v { p.span(&t, sp); }
                spans = aurastudio_core::surface::recycle_spans(v);
            } else {
                drop(v);
                for y in tr.y0..tr.y1 {
                    let mut x = tr.x0;
                    while x < tr.x1 {
                        let mut sp = s.span_mut(x, y, (tr.x1 - x) as usize);
                        let n = sp.len();
                        if n == 0 { break; }
                        p.span(&t, &mut sp);
                        x += n as i32;
                    }
                }
            }
        }
        done
    }
}

/// A coverage tile and where it lies.
struct TileRef<'a> {
    tile: &'a CovTile,
    x0:   i32,
    y0:   i32,
}

/// What painting a pixel needs, once per call.
struct Painter<'a> {
    r:        &'a StrokeRenderer,
    opacity:  f32,
    /// The brush color, straight 0..1 and 8-bit.
    color:    [f32; 4],
    color8:   [u8; 3],
    mode:     PaintMode,
    fast:     Option<aurastudio_core::BlendMode>,
    erase:    bool,
    behind:   bool,
    dither:   bool,
    seed:     u64,
}

impl<'a> Painter<'a> {
    fn new(r: &'a StrokeRenderer) -> Self {
        let b = &r.ctx.brush;
        let c = b.color.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        Self {
            r,
            opacity: b.opacity.clamp(0.0, 1.0) * c[3],
            color: c,
            color8: [to8(c[0]), to8(c[1]), to8(c[2])],
            mode: b.mode,
            fast: b.mode.core(),
            erase: b.erase,
            behind: b.behind,
            // Aliased strokes paint every pixel by the same amount: no gradient to dither.
            dither: !b.aliased,
            seed: b.seed,
        }
    }

    /// The stroke's coverage at a pixel, 0..255 (dithered).
    #[inline(always)]
    fn coverage(&self, t: &TileRef<'_>, x: i32, y: i32) -> u8 {
        let ti = ((y - t.y0) * COV_TILE + (x - t.x0)) as usize;
        let c = f16(t.tile.cov[ti]);
        if c <= 0.0 { return 0; }
        let m = self.r.ctx.stroke_mask_wet(c, t.tile.depth.get(ti).map(|v| f16(*v)), self.r.dual.as_ref().map(|d| d.get(x, y)), x, y);
        let mut k = (m * self.opacity).min(1.0);
        if k <= 0.0 { return 0; }
        if self.mode == PaintMode::Dissolve {
            k = if hash2(x, y, self.seed) < k { 1.0 } else { return 0 };
        }
        let d = if self.dither { hash2(x, y, DITHER_SALT) - 0.5 } else { 0.0 };
        (k * 255.0 + 0.5 + d).clamp(0.0, 255.0) as u8
    }

    /// The color at a pixel: the brush's, or Color Dynamics' (straight).
    #[inline(always)]
    fn color_at(&self, t: &TileRef<'_>, x: i32, y: i32) -> [f32; 3] {
        let ti = ((y - t.y0) * COV_TILE + (x - t.x0)) as usize;
        if self.r.per_dab_color && !t.tile.col.is_empty() {
            let c = f16(t.tile.cov[ti]).max(1e-6);
            let p = &t.tile.col[ti * 3..ti * 3 + 3];
            return [(f16(p[0]) / c).clamp(0.0, 1.0), (f16(p[1]) / c).clamp(0.0, 1.0), (f16(p[2]) / c).clamp(0.0, 1.0)];
        }
        [self.color[0], self.color[1], self.color[2]]
    }

    fn span(&self, t: &TileRef<'_>, sp: &mut Span<'_, Rgba8>) {
        let y = sp.y;
        for i in 0..sp.len() {
            let x = sp.x + i as i32;
            let cov = self.coverage(t, x, y);
            if cov == 0 { continue; }
            let m = sp.cover(i, cov);
            if m == 0 { continue; }
            let base = sp.base[i];
            sp.pixels[i] = self.pixel(t, x, y, base, m);
        }
    }

    /// `base` (the pixel before the stroke) painted at coverage `m`.
    #[inline(always)]
    fn pixel(&self, t: &TileRef<'_>, x: i32, y: i32, base: Rgba8, m: u8) -> Rgba8 {
        if self.erase { return [base[0], base[1], base[2], mul255(base[3], 255 - m)]; }
        let per_pixel = self.r.per_dab_color;
        if self.behind {
            if base[3] == 255 { return base; }
            let c = if per_pixel { self.color_at(t, x, y) } else { [self.color[0], self.color[1], self.color[2]] };
            let da = from8(base[3]);
            let under = from8(m) * (1.0 - da);
            let a = da + under;
            let mix = |b: u8, c: f32| to8((from8(b) * da + c * under) / a);
            return [mix(base[0], c[0]), mix(base[1], c[1]), mix(base[2], c[2]), to8(a)];
        }
        if let (Some(fast), false) = (self.fast, per_pixel) {
            return aurastudio_core::blend::paint_pixel_u8(base, self.color8, m, fast);
        }
        let c = if per_pixel { self.color_at(t, x, y) } else { [self.color[0], self.color[1], self.color[2]] };
        let b = [from8(base[0]), from8(base[1]), from8(base[2]), from8(base[3])];
        let src = [c[0], c[1], c[2], 1.0];
        let o = mode::composite(self.mode, b, src, from8(m));
        [to8(o[0]), to8(o[1]), to8(o[2]), to8(o[3])]
    }
}

#[inline(always)]
fn from8(v: u8) -> f32 { v as f32 * (1.0 / 255.0) }

#[inline(always)]
fn to8(v: f32) -> u8 { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }

#[cfg(test)]
mod coverage_tests;

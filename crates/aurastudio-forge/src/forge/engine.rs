//! Forge, the brush engine: libmypaint's settings, curves and stroke path,
//! with Krita's additions (curves that multiply, wash painting, auto
//! spacing). Two ways to the layer (`Painting`):
//! - Build-up: the stroke's tiles, loaded from the layer as before the
//!   stroke into fix15, get libmypaint's dab blending (smudge, eraser...),
//!   and the changed pixels are written back.
//! - Wash: dabs go through the stroke mask (wash.rs).

use std::collections::{HashMap, HashSet};

use aurastudio_core::{BlendMode, PaintSurface, Rect};

use crate::forge::color::rgb_to_hsv;
use crate::forge::tile::{TilePixels, TILE_SIZE};
use crate::forge::wash::Wash;
use crate::forge::{BrushSetting, BrushState, Dab, ForgeBrush, Painting, Spacing, TiledSurface};
use crate::Brush;

/// One pen sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenSample {
    /// Canvas pixels.
    pub x:        f32,
    pub y:        f32,
    /// 0..1.
    pub pressure: f32,
    /// Pen tilt toward +x / +y, -1..1 (1: 60°, libmypaint's scale); 0 for
    /// a mouse.
    pub tilt:     (f32, f32),
    /// Pen time, seconds.
    pub time:     f64,
}

impl PenSample {
    pub fn new(x: f32, y: f32, pressure: f32, time: f64) -> Self {
        Self { x, y, pressure, tilt: (0.0, 0.0), time }
    }
}


const ONE: u32 = 1 << 15;
const TS: i32 = TILE_SIZE as i32;
// libmypaint's tiles are the layer's tiles.
const _: () = assert!(TILE_SIZE as u32 == aurastudio_core::TILE);

/// Settings by which a brush works on the paint already on the layer.
const PAINT_SETTINGS: [BrushSetting; 5] = [BrushSetting::Smudge, BrushSetting::Eraser, BrushSetting::Posterize, BrushSetting::Colorize, BrushSetting::LockAlpha];

/// A stroke being painted.
pub struct Forge {
    /// The preset's brush, with color / size / opacity bases set from
    /// the shared Brush at each stroke (its curves are kept).
    brush: ForgeBrush,
    state: BrushState,
    /// Working tiles of the current stroke, loaded from the layer.
    tiles: HashMap<(i32, i32), Box<TilePixels>>,
    /// Every tile outside the canvas (dabs there are thrown away).
    scratch: Box<TilePixels>,
    /// Pen time of the previous sample, seconds.
    last_time: Option<f64>,
    /// The previous pen sample, and whether the brush follows the pen
    /// (it is over the canvas, see `stroke_to`).
    last: Option<PenSample>,
    on_canvas: bool,
    /// Upper bound of the dab radius: how far off the canvas the pen may
    /// go and still paint on it (see `stroke_to`).
    reach: f32,
    /// The preset's own eraser setting (the Eraser tool turns it fully on).
    preset_eraser: f32,
    /// The Opacity slider: multiplies every dab's opacity. The brush's own
    /// opacity and its pressure curve stay as the .myb file made them
    /// (many start at 0 and get everything from pressure).
    opacity: f32,
    /// Blend mode (wash painting).
    blend: BlendMode,
    wash: Wash,
}

impl Forge {
    pub fn new(brush: ForgeBrush) -> Self {
        let preset_eraser = brush.get(BrushSetting::Eraser).base_value;
        Self { brush, preset_eraser, state: BrushState::default(), tiles: HashMap::new(),
            scratch: Box::new([[[0u16; 4]; TILE_SIZE]; TILE_SIZE]), last_time: None, last: None, on_canvas: false, reach: 64.0, opacity: 1.0,
            blend: BlendMode::Normal, wash: Wash::default() }
    }

    fn washes(&self) -> bool { self.brush.options.painting == Painting::Wash }

    /// Stroke previews: a dab every `px` along the path whatever the
    /// size, random numbers by place (`ForgeOptions::seeds_by_place`).
    pub fn preview_spacing(&mut self, px: f32, stride: u32) {
        self.brush.options.spacing = Spacing::Fixed(px);
        self.brush.options.seeds_by_place = Some(stride);
    }

    /// Size (diameter, px) the .myb file was made with, to show it in the
    /// UI when the preset is selected.
    pub fn native_size(brush: &ForgeBrush) -> u32 {
        let r = brush.get(BrushSetting::Radius).base_value.exp();
        (r * 2.0).round().clamp(1.0, 1000.0) as u32
    }

    /// Start a stroke's random sequences afresh (smudge color sampling):
    /// the same stroke then paints the same pixels (stroke previews don't
    /// shimmer from one repaint to the next).
    pub fn reset_random() {
        crate::forge::brushmodes::reset_get_color_rng();
    }

    /// A copy that only paints (no erasing, smudging, colorizing...): the
    /// shape of its stroke, for the brush lists' stroke previews.
    pub fn painting_only(brush: &ForgeBrush) -> ForgeBrush {
        let mut b = brush.clone();
        for k in PAINT_SETTINGS {
            let v = b.get_mut(k);
            v.base_value = 0.0;
            v.inputs.clear();
        }
        b
    }

    /// Edge hardness the .myb file was made with.
    pub fn native_hardness(brush: &ForgeBrush) -> f32 {
        brush.get(BrushSetting::Hardness).base_value.clamp(0.0, 1.0)
    }

    /// Apply the shared settings (color docker, size and opacity sliders)
    /// as the brush's base values; its pressure / speed curves stay.
    pub fn sync_from(&mut self, b: &Brush) {
        let hsv = rgb_to_hsv(b.color[0] as f32 / 255.0, b.color[1] as f32 / 255.0, b.color[2] as f32 / 255.0);
        self.brush.get_mut(BrushSetting::ColorH).base_value = hsv.h;
        self.brush.get_mut(BrushSetting::ColorS).base_value = hsv.s;
        self.brush.get_mut(BrushSetting::ColorV).base_value = hsv.v;
        let radius = b.radius.max(0.5);
        self.brush.get_mut(BrushSetting::Radius).base_value = radius.ln();
        self.opacity = b.opacity.clamp(0.0, 1.0);
        self.brush.get_mut(BrushSetting::Hardness).base_value = b.hardness.clamp(0.0, 1.0);
        self.brush.get_mut(BrushSetting::Eraser).base_value = if b.erase { 1.0 } else { self.preset_eraser };
        self.blend = b.blend;
        // Margin: dynamics grow the dab, smudge reads around it.
        self.reach = (radius * 3.0 + 16.0).min(4096.0);
    }

    pub fn begin_stroke(&mut self) {
        self.tiles.clear();
        self.last = None;
        self.on_canvas = false;
        self.resume();
    }

    /// The area a wash stroke step changed.
    fn wash_step<S: PaintSurface>(&mut self, surface: &mut S, step: impl FnOnce(&ForgeBrush, &mut BrushState, &mut WashTarget<S>)) -> Option<Rect> {
        let mut t = WashTarget { surface, wash: &mut self.wash, scratch: &mut self.scratch, opacity: self.opacity, blend: self.blend, dirty: None };
        step(&self.brush, &mut self.state, &mut t);
        t.dirty
    }

    /// Start following the pen afresh within the same stroke (after
    /// `lift`): the tiles already painted are kept.
    fn resume(&mut self) {
        self.state = BrushState::default();
        self.last_time = None;
        self.wash.begin_stroke();
    }

    /// One pen sample. Only the path over the canvas (and a margin) is
    /// painted; off it, the stroke pauses like a lifted pen.
    #[profiling::function]
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, p: PenSample) -> Option<Rect> {
        let prev = self.last.replace(p);
        let m = (self.reach + 64.0).max(256.0);
        let area = (-m, -m, surface.width() as f32 + m, surface.height() as f32 + m);
        let seg = match prev {
            None => (p.x >= area.0 && p.y >= area.1 && p.x <= area.2 && p.y <= area.3).then_some((0.0, 1.0)),
            Some(q) => clip(&q, &p, area),
        };
        let at = |t: f32| prev.map_or(p, |q| lerp(&q, &p, t));
        let mut dirty = None;
        match seg {
            Some((t0, t1)) => {
                if !self.on_canvas {
                    // Coming (back) onto the canvas: a fresh start there.
                    self.resume();
                    self.on_canvas = true;
                    merge(&mut dirty, self.step(surface, at(t0)));
                }
                if prev.is_some() { merge(&mut dirty, self.step(surface, at(t1))); }
                if t1 < 1.0 { merge(&mut dirty, self.lift(surface)); }
            }
            None if self.on_canvas => merge(&mut dirty, self.lift(surface)),
            None => {}
        }
        dirty
    }

    fn step<S: PaintSurface>(&mut self, surface: &mut S, p: PenSample) -> Option<Rect> {
        let PenSample { x, y, pressure, tilt, time } = p;
        let dtime = self.last_time.map(|t| (time - t).clamp(0.001, 0.25)).unwrap_or(0.0);
        self.last_time = Some(time);
        if self.washes() {
            return self.wash_step(surface, |b, st, t| { b.stroke_to(st, t, x, y, pressure, tilt.0, tilt.1, dtime); });
        }
        // Tiles load as dabs reach them, or as smudge reads them
        // (`prepare_read`): only where the stroke goes, not a box around it.
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new(), area: None, opacity: self.opacity };
        self.brush.stroke_to(&mut self.state, &mut tiles, x, y, pressure, tilt.0, tilt.1, dtime);
        let (dirty, area) = (std::mem::take(&mut tiles.dirty), tiles.area);
        write_back(&self.tiles, &dirty, area, surface)
    }

    /// End of stroke: let libmypaint finish its smoothed path.
    #[profiling::function]
    pub fn end_stroke<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        let rect = self.lift(surface);
        self.tiles.clear();
        rect
    }

    /// Pen lifted within the stroke (it left the canvas): finish the
    /// smoothed path, keep the tiles for `resume`. Reloading them from the
    /// layer as it was before the stroke would erase what was painted.
    pub fn lift<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        if !std::mem::take(&mut self.on_canvas) { return None; }
        if self.washes() {
            return self.wash_step(surface, |b, st, t| {
                b.finish_stroke(st, t);
                // The stroke reaches the pen (dabs stop up to a spacing short).
                if b.options.dab_on_press && st.started && st.dist_past_dab > 0.01 {
                    let (x, y, p) = (st.actual_x, st.actual_y, st.last_pressure);
                    b.dab_at_rest(st, t, x, y, p);
                }
            });
        }
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new(), area: None, opacity: self.opacity };
        self.brush.finish_stroke(&mut self.state, &mut tiles);
        let (dirty, area) = (std::mem::take(&mut tiles.dirty), tiles.area);
        write_back(&self.tiles, &dirty, area, surface)
    }
}

/// The stroke's tiles as a libmypaint surface. A tile is loaded from the
/// layer as it was before the stroke the first time it is needed.
struct TileCache<'a, S: PaintSurface> {
    tiles:   &'a mut HashMap<(i32, i32), Box<TilePixels>>,
    scratch: &'a mut Box<TilePixels>,
    surface: &'a mut S,
    dirty:   HashSet<(i32, i32)>,
    /// Pixels the dabs may have changed (x0, y0, x1, y1): only those are
    /// written back, not the whole tiles (a small brush touches a few
    /// pixels of each).
    area:    Option<(i32, i32, i32, i32)>,
    /// The Opacity slider, applied to each dab.
    opacity: f32,
}

impl<S: PaintSurface> TileCache<'_, S> {
    fn load(&mut self, tx: i32, ty: i32) -> &mut Box<TilePixels> {
        let (w, h) = (self.surface.width(), self.surface.height());
        let surface = &mut *self.surface;
        self.tiles.entry((tx, ty)).or_insert_with(|| {
            let (x0, y0) = ((tx * TS).clamp(0, w as i32) as u32, (ty * TS).clamp(0, h as i32) as u32);
            let (x1, y1) = ((tx * TS + TS).clamp(0, w as i32) as u32, (ty * TS + TS).clamp(0, h as i32) as u32);
            let mut tile: Box<TilePixels> = Box::new([[[0u16; 4]; TILE_SIZE]; TILE_SIZE]);
            if x0 >= x1 || y0 >= y1 { return tile; }
            surface.prepare((x0, y0, x1, y1));
            let off = (x0 as i32 - tx * TS) as usize;
            for y in y0..y1 {
                let (_, base, _) = surface.span_mut(x0, y, (x1 - x0) as usize);
                let row = &mut tile[(y as i32 - ty * TS) as usize][off..];
                for (px, b) in row.iter_mut().zip(base.chunks_exact(4)) {
                    *px = to_fix15([b[0], b[1], b[2], b[3]]);
                }
            }
            tile
        })
    }

    /// Tile (tx, ty) covers some canvas pixels.
    fn on_canvas(&self, tx: i32, ty: i32) -> bool {
        let (w, h) = (self.surface.width() as i32, self.surface.height() as i32);
        tx >= 0 && ty >= 0 && tx * TS < w && ty * TS < h
    }

    /// Load the canvas tiles in this area (the rest is off the canvas).
    fn load_area(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let (w, h) = (self.surface.width() as f32, self.surface.height() as f32);
        let t = |v: f32, max: f32| (v.clamp(0.0, max - 1.0).floor() as i32).div_euclid(TS);
        if x1 < 0.0 || y1 < 0.0 || x0 >= w || y0 >= h { return; }
        for ty in t(y0, h)..=t(y1, h) {
            for tx in t(x0, w)..=t(x1, w) {
                self.load(tx, ty);
            }
        }
    }
}

impl<S: PaintSurface> TiledSurface for TileCache<'_, S> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        if !self.on_canvas(tx, ty) { return self.scratch; }
        self.dirty.insert((tx, ty));
        self.load(tx, ty)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tiles_mut(&mut self, tiles: &[(i32, i32)]) -> Option<Vec<(usize, &mut TilePixels)>> {
        // (Off the canvas: nothing to paint, nothing kept.)
        let mut index = HashMap::with_capacity(tiles.len());
        for (i, &(tx, ty)) in tiles.iter().enumerate() {
            if !self.on_canvas(tx, ty) { continue; }
            self.dirty.insert((tx, ty));
            self.load(tx, ty);
            index.insert((tx, ty), i);
        }
        Some(self.tiles.iter_mut().filter_map(|(k, t)| index.get(k).map(|&i| (i, &mut **t))).collect())
    }

    fn prepare_read(&mut self, x: f32, y: f32, radius: f32) {
        // (The tiles get_color_default reads: radius at least 1, plus a
        // pixel of fringe.)
        let r = radius.max(1.0) + 1.0;
        self.load_area(x - r, y - r, x + r, y + r);
    }

    fn draw_dab(&mut self, dab: &Dab) -> bool {
        // (As draw_dab_default's bounds: radius + 1 around the center.)
        let r = dab.radius + 1.0;
        let b = ((dab.x - r).floor() as i32, (dab.y - r).floor() as i32, (dab.x + r).floor() as i32 + 1, (dab.y + r).floor() as i32 + 1);
        self.area = Some(match self.area {
            None => b,
            Some(a) => (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)),
        });
        if self.opacity >= 1.0 { return crate::forge::brushmodes::draw_dab_default(self, dab); }
        let dab = Dab { opaque: dab.opaque * self.opacity, ..*dab };
        crate::forge::brushmodes::draw_dab_default(self, &dab)
    }

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|b| &**b)
    }
}

/// The layer as a libmypaint surface for wash painting: dabs go to
/// wash.rs (nothing is read back: no smudge).
struct WashTarget<'a, S: PaintSurface> {
    surface: &'a mut S,
    wash:    &'a mut Wash,
    scratch: &'a mut Box<TilePixels>,
    opacity: f32,
    blend:   BlendMode,
    dirty:   Option<Rect>,
}

impl<S: PaintSurface> TiledSurface for WashTarget<'_, S> {
    fn tile_request_start(&mut self, _tx: i32, _ty: i32) -> &mut TilePixels { self.scratch }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn draw_dab(&mut self, dab: &Dab) -> bool {
        let Some(r) = self.wash.dab(self.surface, dab, self.opacity, self.blend) else { return false };
        merge(&mut self.dirty, Some(r));
        true
    }
}

/// The part of the segment a → b inside `area` (x0, y0, x1, y1), as
/// fractions of it (Liang-Barsky).
fn clip(a: &PenSample, b: &PenSample, area: (f32, f32, f32, f32)) -> Option<(f32, f32)> {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.x as f64 - area.0 as f64), (dx, area.2 as f64 - a.x as f64),
                   (-dy, a.y as f64 - area.1 as f64), (dy, area.3 as f64 - a.y as f64)] {
        if p == 0.0 {
            if q < 0.0 { return None; }
            continue;
        }
        let r = q / p;
        if p < 0.0 { t0 = t0.max(r) } else { t1 = t1.min(r) }
        if t0 > t1 { return None; }
    }
    Some((t0 as f32, t1 as f32))
}

fn lerp(a: &PenSample, b: &PenSample, t: f32) -> PenSample {
    let l = |u: f32, v: f32| u + (v - u) * t;
    PenSample {
        x: l(a.x, b.x), y: l(a.y, b.y), pressure: l(a.pressure, b.pressure),
        tilt: (l(a.tilt.0, b.tilt.0), l(a.tilt.1, b.tilt.1)),
        time: a.time + (b.time - a.time) * t as f64,
    }
}

fn merge(acc: &mut Option<Rect>, r: Option<Rect>) {
    let Some(r) = r else { return };
    *acc = Some(acc.map_or(r, |d| aurastudio_core::union(d, r)));
}

/// Straight 8-bit RGBA -> premultiplied fix15.
fn to_fix15(p: [u8; 4]) -> [u16; 4] {
    let a = (p[3] as u32 * ONE + 127) / 255;
    let c = |v: u8| ((v as u32 * a + 127) / 255) as u16;
    [c(p[0]), c(p[1]), c(p[2]), a as u16]
}

/// Premultiplied fix15 -> straight 8-bit RGBA (white when transparent,
/// like our cleared layers).
fn from_fix15(p: [u16; 4]) -> [u8; 4] {
    let a = (p[3] as u32).min(ONE);
    if a == ONE {
        // Opaque: no division.
        let c = |v: u16| ((v as u32 * 255 + ONE / 2) >> 15).min(255) as u8;
        return [c(p[0]), c(p[1]), c(p[2]), 255];
    }
    let a8 = ((a * 255 + ONE / 2) / ONE) as u8;
    if a8 == 0 {
        return [255, 255, 255, 0];
    }
    // (v * 255 + a / 2) / a by one integer reciprocal: exact for these
    // numerators (below 2^23; see the test).
    let m = (1u64 << 40) / a as u64 + 1;
    let c = |v: u16| ((((v as u32 * 255 + a / 2) as u64 * m) >> 40) as u32).min(255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), a8]
}

/// Copy the changed pixels (of the dirty tiles, inside `area`) into the
/// layer; returns their bounds. A big area is converted on several cores
/// (the conversion is most of the cost), then copied in.
fn write_back<S: PaintSurface>(tiles: &HashMap<(i32, i32), Box<TilePixels>>, dirty: &HashSet<(i32, i32)>, area: Option<(i32, i32, i32, i32)>, surface: &mut S) -> Option<Rect> {
    let (w, h) = (surface.width(), surface.height());
    let a = area?;
    // (tile, its part to write: x0, y0, x1, y1 in canvas pixels)
    type Job<'t> = (&'t TilePixels, (i32, i32, i32, i32));
    let jobs: Vec<Job> = dirty.iter().filter_map(|&(tx, ty)| {
        let tile = tiles.get(&(tx, ty))?;
        let r = ((tx * TS).max(0).max(a.0), (ty * TS).max(0).max(a.1), (tx * TS + TS).min(w as i32).min(a.2), (ty * TS + TS).min(h as i32).min(a.3));
        (r.0 < r.2 && r.1 < r.3).then_some((&**tile, r))
    }).collect();
    let convert = |(tile, r): &Job| -> Vec<u8> {
        let (tx0, ty0) = (r.0.div_euclid(TS) * TS, r.1.div_euclid(TS) * TS);
        let w = (r.2 - r.0) as usize;
        let mut out = vec![0u8; w * (r.3 - r.1) as usize * 4];
        for (y, dst) in (r.1..r.3).zip(out.chunks_exact_mut(w * 4)) {
            let src = &tile[(y - ty0) as usize][(r.0 - tx0) as usize..(r.2 - tx0) as usize];
            for (d, &v) in dst.chunks_exact_mut(4).zip(src) { d.copy_from_slice(&from_fix15(v)); }
        }
        out
    };
    let pixels: usize = jobs.iter().map(|(_, r)| ((r.2 - r.0) * (r.3 - r.1)) as usize).sum();
    let converted: Vec<Vec<u8>> = if pixels >= 128 * 1024 && jobs.len() > 1 {
        use rayon::prelude::*;
        jobs.par_iter().map(convert).collect()
    } else {
        jobs.iter().map(convert).collect()
    };
    let mut out: Option<Rect> = None;
    for ((_, r), px) in jobs.iter().zip(&converted) {
        let row = (r.2 - r.0) as usize * 4;
        for (y, src) in (r.1..r.3).zip(px.chunks_exact(row)) {
            let (dst, _, _) = surface.span_mut(r.0 as u32, y as u32, (r.2 - r.0) as usize);
            dst.copy_from_slice(src);
        }
        merge(&mut out, Some((r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 256;
    const H: u32 = 128;

    use aurastudio_core::FlatSurface as S;

    fn surface(rgba: [u8; 4]) -> S {
        S::new(W, H, rgba)
    }

    fn stroke(e: &mut Forge, s: &mut S) {
        e.begin_stroke();
        for i in 0..=60 {
            e.stroke_to(s, PenSample::new(30.0 + i as f32 * 3.0, 64.0, 1.0, i as f64 * 0.008));
        }
        e.end_stroke(s);
    }

    #[test]
    fn fix15_reciprocal_is_exact() {
        for a in 1..=ONE {
            for k in 0..24u32 {
                let v = (k.wrapping_mul(2_654_435_761u32.wrapping_mul(a)) >> 5) % (ONE + 1);
                let want = ((v * 255 + a / 2) / a).min(255) as u8;
                let a8 = ((a * 255 + ONE / 2) / ONE) as u8;
                let got = from_fix15([v as u16, a as u16, 0, a as u16]);
                if a8 != 0 && a != ONE { assert_eq!(got[0], want, "v {v} a {a}"); }
            }
        }
    }

    #[test]
    fn fix15_round_trip() {
        for p in [[0, 0, 0, 255], [255, 255, 255, 0], [10, 200, 90, 128], [255, 255, 255, 255]] {
            let back = from_fix15(to_fix15(p));
            assert_eq!(back[3], p[3]);
            if p[3] > 0 {
                for k in 0..3 { assert!((back[k] as i32 - p[k] as i32).abs() <= 1, "{p:?} -> {back:?}"); }
            }
        }
    }

    #[test]
    fn default_myb_brush_paints_and_eraser_erases() {
        // A plain brush defined in .myb JSON.
        let json = r#"{"version": 3, "settings": {
            "radius_logarithmic": {"base_value": 2.0, "inputs": {}},
            "hardness": {"base_value": 0.9, "inputs": {}},
            "opaque": {"base_value": 1.0, "inputs": {}},
            "opaque_multiply": {"base_value": 1.0, "inputs": {}},
            "dabs_per_actual_radius": {"base_value": 4.0, "inputs": {}}
        }}"#;
        let preset = crate::forge::myb::from_str(json).expect("parse");
        let mut e = Forge::new(preset.clone());
        let mut b = Brush::new();
        b.radius = 6.0;
        e.sync_from(&b);
        let mut s = surface([255, 255, 255, 255]);
        stroke(&mut e, &mut s);
        let i = ((64 * W + 100) * 4) as usize;
        assert!(s.pixels[i] < 40, "not painted: {:?}", &s.pixels[i..i + 4]);

        // Same brush as an eraser, on the painted layer: back to transparent.
        let mut eraser = preset;
        eraser.get_mut(BrushSetting::Eraser).base_value = 1.0;
        let mut e = Forge::new(eraser);
        e.sync_from(&b);
        s.base.copy_from_slice(&s.pixels);
        stroke(&mut e, &mut s);
        assert!(s.pixels[i + 3] < 30, "not erased: {:?}", &s.pixels[i..i + 4]);
    }

    /// Zoomed out, a stroke can leave the canvas by a huge distance. Only
    /// canvas tiles may be kept (a tile per off-canvas spot ran out of
    /// memory), and the part on the canvas is still painted.
    #[test]
    fn stroke_far_off_the_canvas_keeps_only_canvas_tiles() {
        let json = r#"{"version": 3, "settings": {
            "radius_logarithmic": {"base_value": 2.0, "inputs": {}},
            "opaque": {"base_value": 1.0, "inputs": {}},
            "dabs_per_actual_radius": {"base_value": 2.0, "inputs": {}}
        }}"#;
        let mut e = Forge::new(crate::forge::myb::from_str(json).expect("parse"));
        e.sync_from(&Brush::new());
        let mut s = surface([255, 255, 255, 255]);
        e.begin_stroke();
        // (MyPaint strokes ramp up from no pressure: a few samples first.)
        for (k, x) in [90.0, 95.0, 100.0].into_iter().enumerate() {
            e.stroke_to(&mut s, PenSample::new(x, 64.0, 1.0, k as f64 * 0.005));
        }
        e.stroke_to(&mut s, PenSample::new(200_000.0, -150_000.0, 1.0, 0.01));
        e.stroke_to(&mut s, PenSample::new(-90_000.0, 300_000.0, 1.0, 0.02));
        let canvas_tiles = (W.div_ceil(TS as u32) * H.div_ceil(TS as u32)) as usize;
        assert!(e.tiles.len() <= canvas_tiles, "{} tiles kept", e.tiles.len());
        e.end_stroke(&mut s);
        let i = ((64 * W + 100) * 4) as usize;
        assert!(s.pixels[i] < 128, "start of the stroke not painted: {:?}", &s.pixels[i..i + 4]);
    }
}

//! Forge, AuraStudio's brush engine: libmypaint's model (its ~60 settings,
//! inputs and curves, its stroke path and dab rules) grown with Krita's
//! ways (curves that multiply, wash painting, auto spacing). Brushes from
//! other formats are read into it (.myb today; Krita next).
//!
//! Two ways to reach the layer (`Painting`):
//! - Build-up: the tiles a stroke touches are loaded from the layer as
//!   before the stroke into 64x64 premultiplied fix15 tiles (1.0 = 1 <<
//!   15, sRGB-encoded values like our layers), libmypaint's dab blending
//!   applies (eraser, smudge, colorize... read the colors already there),
//!   and the changed tiles are written back.
//! - Wash: dabs go through the layer's stroke mask (wash.rs), with the
//!   brush's blend mode.

use std::collections::{HashMap, HashSet};

use aurastudio_canvas::{BlendMode, PaintSurface, Rect};

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
// libmypaint tiles line up with the layer tiles: a libmypaint tile row is
// one run of a layer tile.
const _: () = assert!(TILE_SIZE as u32 == aurastudio_canvas::TILE);

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
    /// Shared stand-in for every tile outside the canvas: dabs there are
    /// drawn into it and thrown away. Storing a tile per off-canvas spot
    /// ran out of memory (zoomed out, a stroke leaving the canvas spans
    /// millions of tiles).
    scratch: Box<TilePixels>,
    /// Pen time of the previous sample, seconds.
    last_time: Option<f64>,
    /// The previous pen sample, and whether the brush follows the pen
    /// (it is over the canvas, see `stroke_to`).
    last: Option<PenSample>,
    on_canvas: bool,
    /// Upper bound of the dab radius, for preloading the tiles a dab or a
    /// smudge sample may read.
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
        // Dynamics can grow the dab (pressure, random...), smudge reads
        // around it: keep a generous margin.
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

    /// One pen sample. Only the path over the canvas (plus a margin)
    /// is painted: zoomed out, samples can land millions of pixels away,
    /// and dabs out there take seconds and show nothing. Off it, the
    /// stroke pauses like a lifted pen and starts afresh where it comes
    /// back.
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
        // Tiles a dab or a smudge sample may touch around the pen must be
        // loaded first: libmypaint reads them through tile_lookup, which
        // can't load anything.
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new(), opacity: self.opacity };
        // From the brush's smoothed position (dabs are placed between it and
        // the pen) to the pen, with the margin of the biggest dab so far.
        let prev = if self.state.started { (self.state.actual_x, self.state.actual_y) } else { (x, y) };
        let reach = self.reach.max(self.state.actual_radius * 3.0 + 16.0);
        tiles.preload(x.min(prev.0) - reach, y.min(prev.1) - reach, x.max(prev.0) + reach, y.max(prev.1) + reach);
        self.brush.stroke_to(&mut self.state, &mut tiles, x, y, pressure, tilt.0, tilt.1, dtime);
        let dirty = std::mem::take(&mut tiles.dirty);
        write_back(&self.tiles, &dirty, surface)
    }

    /// End of stroke: let libmypaint finish its smoothed path.
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
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new(), opacity: self.opacity };
        self.brush.finish_stroke(&mut self.state, &mut tiles);
        let dirty = std::mem::take(&mut tiles.dirty);
        write_back(&self.tiles, &dirty, surface)
    }
}

/// The stroke's tiles as a libmypaint surface. A tile is loaded from the
/// layer as it was before the stroke the first time it is needed.
struct TileCache<'a, S: PaintSurface> {
    tiles:   &'a mut HashMap<(i32, i32), Box<TilePixels>>,
    scratch: &'a mut Box<TilePixels>,
    surface: &'a mut S,
    dirty:   HashSet<(i32, i32)>,
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
    fn preload(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
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

    fn draw_dab(&mut self, dab: &Dab) -> bool {
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
        self.dirty = Some(match self.dirty {
            None => r,
            Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
        });
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
    *acc = Some(match *acc {
        None => r,
        Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
    });
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
        // Opaque (most painted pixels): same rounding as below, without
        // the divisions (a third of a big brush's write-back).
        let c = |v: u16| ((v as u32 * 255 + ONE / 2) >> 15).min(255) as u8;
        return [c(p[0]), c(p[1]), c(p[2]), 255];
    }
    let a8 = ((a * 255 + ONE / 2) / ONE) as u8;
    if a8 == 0 {
        return [255, 255, 255, 0];
    }
    // (v * 255 + a / 2) / a, by one reciprocal instead of three integer
    // divisions. Exact: the quotient's fraction is a multiple of 1 / a
    // (≥ 2^-15), far above the f64 product's error, which the tiny bias
    // keeps from rounding an exact integer down.
    let inv = 1.0 / a as f64;
    let c = |v: u16| (((v as u32 * 255 + a / 2) as f64 * inv + 1e-9) as u32).min(255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), a8]
}

/// Copy the changed tiles into the layer pixels; returns their bounds.
fn write_back<S: PaintSurface>(tiles: &HashMap<(i32, i32), Box<TilePixels>>, dirty: &HashSet<(i32, i32)>, surface: &mut S) -> Option<Rect> {
    let (w, h) = (surface.width(), surface.height());
    let mut out: Option<Rect> = None;
    for &(tx, ty) in dirty {
        let Some(tile) = tiles.get(&(tx, ty)) else { continue };
        let x0 = (tx * TS).max(0);
        let y0 = (ty * TS).max(0);
        let x1 = (tx * TS + TS).min(w as i32);
        let y1 = (ty * TS + TS).min(h as i32);
        if x0 >= x1 || y0 >= y1 { continue; }
        for y in y0..y1 {
            let row = &tile[(y - ty * TS) as usize][(x0 - tx * TS) as usize..];
            let (pixels, _, _) = surface.span_mut(x0 as u32, y as u32, (x1 - x0) as usize);
            for (p, &v) in pixels.chunks_exact_mut(4).zip(row) {
                p.copy_from_slice(&from_fix15(v));
            }
        }
        let r = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        out = Some(match out {
            None => r,
            Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 256;
    const H: u32 = 128;

    use aurastudio_canvas::FlatSurface as S;

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

//! Native MyPaint brushes (.myb files) through hokusai, a Rust port of
//! libmypaint.
//!
//! Unlike the "wash" engine (hokusai_engine.rs, our CSP-like round brush,
//! which paints on an empty stroke surface), a MyPaint brush works on the
//! layer itself: the tiles it touches are loaded from the layer as it was
//! before the stroke, and libmypaint's own dab blending applies (normal,
//! eraser, lock alpha, colorize...). That is what smudge, blender and
//! eraser brushes need: they read the colors already on the canvas.
//!
//! Tiles are 64x64 premultiplied RGBA in fix15 (1.0 = 1 << 15), holding
//! sRGB-encoded values like our layers (see hokusai_engine.rs).

use std::collections::{HashMap, HashSet};

use aurastudio_canvas::{PaintSurface, Rect};
use hokusai::color::rgb_to_hsv;
use hokusai::tile::{TilePixels, TILE_SIZE};
use hokusai::{BrushSetting, BrushState, TiledSurface};

use crate::Brush;

const ONE: u32 = 1 << 15;
const TS: i32 = TILE_SIZE as i32;

pub struct MyPaintEngine {
    /// The preset's brush, with color / size / opacity bases set from
    /// the shared Brush at each stroke (its curves are kept).
    brush: hokusai::Brush,
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
    /// Upper bound of the dab radius, for preloading the tiles a dab or a
    /// smudge sample may read.
    reach: f32,
}

impl MyPaintEngine {
    pub fn new(brush: hokusai::Brush) -> Self {
        Self { brush, state: BrushState::default(), tiles: HashMap::new(),
            scratch: Box::new([[[0u16; 4]; TILE_SIZE]; TILE_SIZE]), last_time: None, reach: 64.0 }
    }

    /// Size (diameter, px) and opacity the .myb file was made with, to show
    /// them in the UI when the preset is selected.
    pub fn native_size_and_opacity(brush: &hokusai::Brush) -> (u32, f32) {
        let r = brush.get(BrushSetting::Radius).base_value.exp();
        let d = (r * 2.0).round().clamp(1.0, 1000.0) as u32;
        (d, brush.get(BrushSetting::Opaque).base_value.clamp(0.0, 1.0))
    }

    /// Edge hardness the .myb file was made with.
    pub fn native_hardness(brush: &hokusai::Brush) -> f32 {
        brush.get(BrushSetting::Hardness).base_value.clamp(0.0, 1.0)
    }

    /// Apply the shared settings (color docker, size and opacity sliders)
    /// as the brush's base values; its pressure / speed curves stay.
    pub fn sync_from(&mut self, b: &Brush) {
        let hsv = rgb_to_hsv(b.color[0] as f32 / 255.0, b.color[1] as f32 / 255.0, b.color[2] as f32 / 255.0);
        self.brush.get_mut(BrushSetting::ColorH).base_value = hsv.h;
        self.brush.get_mut(BrushSetting::ColorS).base_value = hsv.s;
        self.brush.get_mut(BrushSetting::ColorV).base_value = hsv.v;
        let radius = b.radius.max(1) as f32;
        self.brush.get_mut(BrushSetting::Radius).base_value = radius.ln();
        self.brush.get_mut(BrushSetting::Opaque).base_value = b.opacity.clamp(0.0, 1.0);
        self.brush.get_mut(BrushSetting::Hardness).base_value = b.hardness.clamp(0.0, 1.0);
        // Dynamics can grow the dab (pressure, random...), smudge reads
        // around it: keep a generous margin.
        self.reach = (radius * 3.0 + 16.0).min(4096.0);
    }

    pub fn begin_stroke(&mut self) {
        self.tiles.clear();
        self.resume();
    }

    /// Start following the pen afresh within the same stroke (after
    /// `lift`): the tiles already painted are kept.
    pub fn resume(&mut self) {
        self.state = BrushState::default();
        self.last_time = None;
    }

    /// One pen sample at pen time `time` (seconds).
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32, time: f64) -> Option<Rect> {
        let dtime = self.last_time.map(|t| (time - t).clamp(0.001, 0.25)).unwrap_or(0.0);
        self.last_time = Some(time);
        // Tiles a dab or a smudge sample may touch around the pen must be
        // loaded first: libmypaint reads them through tile_lookup, which
        // can't load anything.
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new() };
        // From the brush's smoothed position (dabs are placed between it and
        // the pen) to the pen, with the margin of the biggest dab so far.
        let prev = if self.state.started { (self.state.actual_x, self.state.actual_y) } else { (x, y) };
        let reach = self.reach.max(self.state.actual_radius * 3.0 + 16.0);
        tiles.preload(x.min(prev.0) - reach, y.min(prev.1) - reach, x.max(prev.0) + reach, y.max(prev.1) + reach);
        self.brush.stroke_to(&mut self.state, &mut tiles, x, y, pressure, 0.0, 0.0, dtime);
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
        let mut tiles = TileCache { tiles: &mut self.tiles, scratch: &mut self.scratch, surface, dirty: HashSet::new() };
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
}

impl<S: PaintSurface> TileCache<'_, S> {
    fn load(&mut self, tx: i32, ty: i32) -> &mut Box<TilePixels> {
        let (w, h) = (self.surface.width(), self.surface.height());
        let surface = &mut *self.surface;
        self.tiles.entry((tx, ty)).or_insert_with(|| {
            let (x0, y0) = ((tx * TS).clamp(0, w as i32) as u32, (ty * TS).clamp(0, h as i32) as u32);
            let (x1, y1) = ((tx * TS + TS).clamp(0, w as i32) as u32, (ty * TS + TS).clamp(0, h as i32) as u32);
            surface.prepare((x0, y0, x1, y1));
            let (_, base, _) = surface.surfaces_mut();
            let mut tile: Box<TilePixels> = Box::new([[[0u16; 4]; TILE_SIZE]; TILE_SIZE]);
            for (ly, row) in tile.iter_mut().enumerate() {
                let y = ty * TS + ly as i32;
                if y < 0 || y >= h as i32 { continue; }
                for (lx, px) in row.iter_mut().enumerate() {
                    let x = tx * TS + lx as i32;
                    if x < 0 || x >= w as i32 { continue; }
                    let i = (y as usize * w as usize + x as usize) * 4;
                    *px = to_fix15([base[i], base[i + 1], base[i + 2], base[i + 3]]);
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

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|b| &**b)
    }
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
    let a8 = ((a * 255 + ONE / 2) / ONE) as u8;
    if a8 == 0 {
        return [255, 255, 255, 0];
    }
    let c = |v: u16| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), a8]
}

/// Copy the changed tiles into the layer pixels; returns their bounds.
fn write_back<S: PaintSurface>(tiles: &HashMap<(i32, i32), Box<TilePixels>>, dirty: &HashSet<(i32, i32)>, surface: &mut S) -> Option<Rect> {
    let (w, h) = (surface.width(), surface.height());
    let (pixels, _, _) = surface.surfaces_mut();
    let mut out: Option<Rect> = None;
    for &(tx, ty) in dirty {
        let Some(tile) = tiles.get(&(tx, ty)) else { continue };
        let x0 = (tx * TS).max(0);
        let y0 = (ty * TS).max(0);
        let x1 = (tx * TS + TS).min(w as i32);
        let y1 = (ty * TS + TS).min(h as i32);
        if x0 >= x1 || y0 >= y1 { continue; }
        for y in y0..y1 {
            let row = &tile[(y - ty * TS) as usize];
            for x in x0..x1 {
                let i = (y as usize * w as usize + x as usize) * 4;
                pixels[i..i + 4].copy_from_slice(&from_fix15(row[(x - tx * TS) as usize]));
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

    struct S { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for S {
        fn width(&self) -> u32 { W }
        fn height(&self) -> u32 { H }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    fn surface(rgba: [u8; 4]) -> S {
        let px = rgba.repeat((W * H) as usize);
        S { px: px.clone(), base: px, mask: vec![0; (W * H) as usize] }
    }

    fn stroke(e: &mut MyPaintEngine, s: &mut S) {
        e.begin_stroke();
        for i in 0..=60 {
            e.stroke_to(s, 30.0 + i as f32 * 3.0, 64.0, 1.0, i as f64 * 0.008);
        }
        e.end_stroke(s);
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
        let preset = hokusai::myb::from_str(json).expect("parse");
        let mut e = MyPaintEngine::new(preset.clone());
        let mut b = Brush::new();
        b.radius = 6;
        e.sync_from(&b);
        let mut s = surface([255, 255, 255, 255]);
        stroke(&mut e, &mut s);
        let i = ((64 * W + 100) * 4) as usize;
        assert!(s.px[i] < 40, "not painted: {:?}", &s.px[i..i + 4]);

        // Same brush as an eraser, on the painted layer: back to transparent.
        let mut eraser = preset;
        eraser.get_mut(BrushSetting::Eraser).base_value = 1.0;
        let mut e = MyPaintEngine::new(eraser);
        e.sync_from(&b);
        s.base.copy_from_slice(&s.px);
        stroke(&mut e, &mut s);
        assert!(s.px[i + 3] < 30, "not erased: {:?}", &s.px[i..i + 4]);
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
        let mut e = MyPaintEngine::new(hokusai::myb::from_str(json).expect("parse"));
        e.sync_from(&Brush::new());
        let mut s = surface([255, 255, 255, 255]);
        e.begin_stroke();
        e.stroke_to(&mut s, 100.0, 64.0, 1.0, 0.0);
        e.stroke_to(&mut s, 200_000.0, -150_000.0, 1.0, 0.01);
        e.stroke_to(&mut s, -90_000.0, 300_000.0, 1.0, 0.02);
        let canvas_tiles = (W.div_ceil(TS as u32) * H.div_ceil(TS as u32)) as usize;
        assert!(e.tiles.len() <= canvas_tiles, "{} tiles kept", e.tiles.len());
        e.end_stroke(&mut s);
        let i = ((64 * W + 100) * 4) as usize;
        assert!(s.px[i] < 128, "start of the stroke not painted: {:?}", &s.px[i..i + 4]);
    }
}

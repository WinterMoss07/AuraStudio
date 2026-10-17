//! Brush engine backed by hokusai (a Rust port of libmypaint).
//!
//! hokusai paints into 64×64 tiles of premultiplied RGBA fix15. Despite
//! its docs, like libmypaint it writes the dab color as-is (HSV→RGB,
//! sRGB-encoded values, not linearized), so we composite directly in
//! sRGB with no gamma conversion.
//!
//! Opacity model: Krita's "wash" mode (alpha darken composite op).
//! Within ONE stroke, the dab's opacity (brush opacity × pressure) is a
//! CEILING: a pixel never gets more opaque than the strongest dab that
//! covered it, so pressing at 20% with pure black gives 20% gray and
//! crossing the same stroke at 30% gives 30%, not 20+30. The dab's shape
//! (hardness, anti-aliased edge) only sets how far a pixel moves toward
//! that ceiling. A new stroke is painted over the previous ones as usual.
//!
//! A plain per-pixel max was tried first: it showed every dab's circular
//! edge as rings / darker blotches whenever pressure varied along the
//! stroke. libmypaint's own default (normal blend of dab over dab) builds
//! up like an airbrush, which is not wanted either.
//!
//! Each sample, only the tiles touched are recomposited:
//! layer pixels = stroke_base (the layer before the stroke) + stroke tiles.

use std::collections::HashSet;
use std::time::Instant;

use aurastudio_canvas::{PaintSurface, Rect};
use hokusai::brushmodes::draw_dab_default;
use hokusai::color::rgb_to_hsv;
use hokusai::fix15;
use hokusai::mapping::{InputMapping, SettingValue};
use hokusai::surface::Dab;
use hokusai::tile::{TilePixels, TILE_SIZE};
use hokusai::tile_mem::MemSurface;
use hokusai::{BrushInput, BrushSetting, BrushState, TiledSurface};

use crate::{Brush, PressureSmoother};

/// Stroke coverage surface with alpha-darken ("wash") accumulation.
///
/// Every dab is first rendered alone by hokusai's own `draw_dab_default`
/// into an empty scratch tile, so hardness, anti-aliasing and aspect ratio
/// match libmypaint exactly. That scratch alpha is `mask × opaque`; we
/// split it back into the dab mask (0..1) and its opacity ceiling, then
/// merge into `stroke` with Krita's alpha darken:
///   if dst_alpha < ceiling: dst_alpha += (ceiling - dst_alpha) × mask
struct WashStrokeSurface {
    stroke:  MemSurface,
    scratch: MemSurface,
    dirty:   HashSet<(i32, i32)>,
}

impl WashStrokeSurface {
    fn new() -> Self {
        Self { stroke: MemSurface::new(), scratch: MemSurface::new(), dirty: HashSet::new() }
    }

    fn tile(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.stroke.tile(tx, ty)
    }
}

impl TiledSurface for WashStrokeSurface {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.dirty.insert((tx, ty));
        self.stroke.tile_request_start(tx, ty)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.stroke.tile_lookup(tx, ty)
    }

    fn begin_atomic(&mut self) {
        self.dirty.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.dirty.drain().collect()
    }

    fn draw_dab(&mut self, dab: &Dab) -> bool {
        self.scratch.begin_atomic();
        let painted = draw_dab_default(&mut self.scratch, dab);
        let touched = self.scratch.end_atomic();

        let ceiling = dab.opaque.clamp(0.0, 1.0);
        if ceiling <= 0.0 {
            return painted;
        }
        let one = fix15::from_f32(1.0) as f32;
        let cap = ceiling * one;

        for (tx, ty) in touched {
            let src = self.scratch.tile_request_start(tx, ty);
            let dst = self.stroke.tile_request_start(tx, ty);
            for (src_row, dst_row) in src.iter_mut().zip(dst.iter_mut()) {
                for (s, d) in src_row.iter_mut().zip(dst_row.iter_mut()) {
                    let sa = s[3] as f32;
                    if sa > 0.0 {
                        let da = d[3] as f32;
                        if da < cap {
                            let mask  = (sa / cap).min(1.0);
                            let new_a = da + (cap - da) * mask;
                            // Straight dab color, re-premultiplied by the
                            // new alpha (the color is constant per stroke).
                            for c in 0..3 {
                                d[c] = (s[c] as f32 / sa * new_a).round() as u16;
                            }
                            d[3] = new_a.round() as u16;
                        }
                    }
                    // Leave the scratch tile empty for the next dab.
                    *s = [0; 4];
                }
            }
            self.dirty.insert((tx, ty));
        }
        painted
    }
}

pub struct HokusaiEngine {
    brush:     hokusai::Brush,
    state:     BrushState,
    stroke:    WashStrokeSurface,
    last_time: Option<Instant>,
    radius:    f32,
    pub smoother: PressureSmoother,
}

impl HokusaiEngine {
    pub fn new() -> Self {
        Self {
            brush: base_brush(),
            state: BrushState::default(),
            stroke: WashStrokeSurface::new(),
            last_time: None,
            radius: 25.0,
            smoother: PressureSmoother::new(0.5),
        }
    }

    /// Copy color / size / opacity from the shared Brush (the one the
    /// color docker edits) into the libmypaint settings.
    pub fn sync_from(&mut self, b: &Brush) {
        let hsv = rgb_to_hsv(
            b.color[0] as f32 / 255.0,
            b.color[1] as f32 / 255.0,
            b.color[2] as f32 / 255.0,
        );
        self.brush.set(BrushSetting::ColorH, SettingValue::constant(hsv.h));
        self.brush.set(BrushSetting::ColorS, SettingValue::constant(hsv.s));
        self.brush.set(BrushSetting::ColorV, SettingValue::constant(hsv.v));
        // libmypaint stores the radius as a natural log.
        self.radius = b.radius.max(1) as f32;
        self.brush.set(BrushSetting::Radius, SettingValue::constant(self.radius.ln()));
        self.brush.set(BrushSetting::Opaque, SettingValue::constant(b.opacity.clamp(0.0, 1.0)));
    }

    /// Dab density along the stroke (libmypaint's dabs_per_actual_radius).
    pub fn set_dabs_per_radius(&mut self, dabs: f32) {
        self.brush.set(BrushSetting::DabsPerActualRadius, SettingValue::constant(dabs.max(1.0)));
    }

    pub fn begin_stroke(&mut self) {
        self.stroke    = WashStrokeSurface::new();
        self.state     = BrushState::default();
        self.last_time = None;
        self.smoother.reset();
    }


    /// One pointer sample, in canvas coordinates. hokusai interpolates the
    /// dabs between two samples itself.
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        let now   = Instant::now();
        let dtime = self.last_time.map(|t| (now - t).as_secs_f64()).unwrap_or(0.0);
        self.last_time = Some(now);
        let pressure = self.smoother.apply(x, y, pressure, self.radius);

        self.stroke.begin_atomic();
        self.brush.stroke_to(&mut self.state, &mut self.stroke, x, y, pressure, 0.0, 0.0, dtime);
        let tiles = self.stroke.end_atomic();
        self.composite_tiles(surface, &tiles)
    }

    /// End of stroke: let hokusai catch up with its smoothed position.
    pub fn end_stroke<S: PaintSurface>(&mut self, surface: &mut S) -> Option<Rect> {
        self.stroke.begin_atomic();
        self.brush.finish_stroke(&mut self.state, &mut self.stroke);
        let tiles = self.stroke.end_atomic();
        self.composite_tiles(surface, &tiles)
    }

    fn composite_tiles<S: PaintSurface>(&self, surface: &mut S, tiles: &[(i32, i32)]) -> Option<Rect> {
        let w = surface.width() as i32;
        let h = surface.height() as i32;
        let ts = TILE_SIZE as i32;
        let (pixels, base, _mask) = surface.surfaces_mut();
        let mut dirty: Option<Rect> = None;

        for &(tx, ty) in tiles {
            let Some(tile) = self.stroke.tile(tx, ty) else { continue };
            let x0 = (tx * ts).max(0);
            let y0 = (ty * ts).max(0);
            let x1 = (tx * ts + ts).min(w);
            let y1 = (ty * ts + ts).min(h);
            if x0 >= x1 || y0 >= y1 { continue; }

            for y in y0..y1 {
                let row = &tile[(y - ty * ts) as usize];
                for x in x0..x1 {
                    let p    = row[(x - tx * ts) as usize];
                    let idx  = ((y * w + x) * 4) as usize;
                    let sa   = fix15::to_f32(p[3]).min(1.0);
                    if sa <= 0.0 {
                        pixels[idx..idx + 4].copy_from_slice(&base[idx..idx + 4]);
                        continue;
                    }
                    // Straight-alpha "over": stroke (premultiplied) over base.
                    let ba    = base[idx + 3] as f32 / 255.0;
                    let k     = ba * (1.0 - sa);
                    let out_a = sa + k;
                    for c in 0..3 {
                        let v = (fix15::to_f32(p[c]) + base[idx + c] as f32 / 255.0 * k) / out_a;
                        pixels[idx + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    pixels[idx + 3] = (out_a * 255.0).round() as u8;
                }
            }

            let r = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
            dirty = Some(match dirty {
                None    => r,
                Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
            });
        }
        dirty
    }
}

impl Default for HokusaiEngine {
    fn default() -> Self { Self::new() }
}

/// CSP-style round brush: crisp edge, opacity driven by pressure.
/// Color / size / base opacity are overwritten by sync_from().
fn base_brush() -> hokusai::Brush {
    let mut b = hokusai::Brush::new();
    b.set(BrushSetting::Hardness, SettingValue::constant(0.9));
    b.set(BrushSetting::AntiAliasing, SettingValue::constant(1.0));
    // Dab spacing: 6 per radius keeps big strokes' edges smooth.
    b.set(BrushSetting::DabsPerActualRadius, SettingValue::constant(6.0));
    // No opaque_linearize: it lowers each dab's alpha to compensate for
    // dabs piling up, but the wash ceiling already prevents piling up, so
    // it would only make the stroke lighter than the requested opacity.
    b.set(BrushSetting::OpaqueLinearize, SettingValue::constant(0.0));
    // Opacity = brush opacity × pen pressure (linear).
    b.set(
        BrushSetting::OpaqueMultiply,
        SettingValue {
            base_value: 0.0,
            inputs: vec![InputMapping {
                input:  BrushInput::Pressure,
                points: vec![(0.0, 0.0), (1.0, 1.0)],
            }],
            ..Default::default()
        },
    );
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 256;
    const H: u32 = 256;

    struct TestSurface { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8> }
    impl PaintSurface for TestSurface {
        fn width(&self) -> u32 { W }
        fn height(&self) -> u32 { H }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    fn white() -> TestSurface {
        let white = vec![255u8; (W * H * 4) as usize];
        TestSurface { px: white.clone(), base: white, mask: vec![0; (W * H) as usize] }
    }

    fn engine(radius: u32) -> HokusaiEngine {
        let mut e = HokusaiEngine::new();
        let mut b = Brush::new();
        b.radius = radius;
        e.sync_from(&b);
        e.begin_stroke();
        e
    }

    fn red_at(s: &TestSurface, x: u32, y: u32) -> u8 {
        s.px[((y * W + x) * 4) as usize]
    }

    #[test]
    fn paints_black_on_white() {
        let mut s = white();
        let mut e = engine(10);
        let mut dirty = None;
        for i in 0..=50 {
            dirty = dirty.or(e.stroke_to(&mut s, 40.0 + i as f32 * 3.0, 128.0, 1.0));
        }
        e.end_stroke(&mut s);

        assert!(dirty.is_some(), "no tile touched");
        assert!(red_at(&s, 100, 128) < 5, "stroke center not black: {}", red_at(&s, 100, 128));
        assert_eq!(red_at(&s, 10, 10), 255);
    }

    #[test]
    fn pressure_gives_matching_gray() {
        // 20% pressure, black, opacity 100% -> 20% gray = 255 * 0.8 = 204.
        let mut s = white();
        let mut e = engine(10);
        for i in 0..=50 {
            e.stroke_to(&mut s, 40.0 + i as f32 * 3.0, 128.0, 0.2);
        }
        e.end_stroke(&mut s);
        let v = red_at(&s, 100, 128) as i32;
        assert!((v - 204).abs() <= 2, "expected ~204, got {v}");
    }

    #[test]
    fn crossing_the_same_stroke_does_not_build_up() {
        // Go right at 20%, come back over the same line at 30%: the
        // result must be 30% gray (~178), not 20+30 = 50% (~128).
        let mut s = white();
        let mut e = engine(10);
        for i in 0..=50 {
            e.stroke_to(&mut s, 40.0 + i as f32 * 3.0, 128.0, 0.2);
        }
        let after_first = red_at(&s, 100, 128) as i32;
        for i in (0..=50).rev() {
            e.stroke_to(&mut s, 40.0 + i as f32 * 3.0, 128.0, 0.3);
        }
        e.end_stroke(&mut s);
        let v = red_at(&s, 100, 128) as i32;
        assert!((after_first - 204).abs() <= 2, "first pass: expected ~204, got {after_first}");
        assert!((v - 178).abs() <= 2, "expected ~178 (30%), got {v}");
    }

    #[test]
    fn new_stroke_paints_over_previous_one() {
        // Two separate strokes at 50% do stack (that's normal painting):
        // 1 - 0.5*0.5 = 75% -> ~64.
        let mut s = white();
        for _ in 0..2 {
            s.base.copy_from_slice(&s.px);
            let mut e = engine(10);
            for i in 0..=50 {
                e.stroke_to(&mut s, 40.0 + i as f32 * 3.0, 128.0, 0.5);
            }
            e.end_stroke(&mut s);
        }
        let v = red_at(&s, 100, 128) as i32;
        assert!((v - 64).abs() <= 3, "expected ~64, got {v}");
    }
}

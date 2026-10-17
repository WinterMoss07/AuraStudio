//! Moteur de brush basé sur hokusai (port Rust de libmypaint).
//!
//! hokusai peint dans sa propre surface à tuiles 64×64 (RGBA fix15
//! prémultiplié). Malgré la doc, comme libmypaint, la couleur du dab
//! est écrite telle quelle (HSV→RGB, valeurs encodées sRGB, pas
//! linéarisées) : on composite donc directement en sRGB, sans
//! conversion de gamma. On garde une surface de tuiles *par
//! trait* : à chaque échantillon, on recompose uniquement les tuiles
//! modifiées = stroke_base (calque avant le trait) + tuiles du trait.
//! Même principe anti double-blend que le brush simple : on ne mélange
//! jamais un trait avec lui-même.

use std::time::Instant;

use aurastudio_canvas::{PaintSurface, Rect};
use hokusai::color::rgb_to_hsv;
use hokusai::fix15;
use hokusai::mapping::{InputMapping, SettingValue};
use hokusai::tile::TILE_SIZE;
use hokusai::tile_mem::MemSurface;
use hokusai::{BrushInput, BrushSetting, BrushState, TiledSurface};

use crate::Brush;

pub struct HokusaiEngine {
    brush:     hokusai::Brush,
    state:     BrushState,
    stroke:    MemSurface,
    last_time: Option<Instant>,
}

impl HokusaiEngine {
    pub fn new() -> Self {
        Self {
            brush: base_brush(),
            state: BrushState::default(),
            stroke: MemSurface::new(),
            last_time: None,
        }
    }

    /// Recopie couleur / taille / opacité du Brush partagé (celui que
    /// règle le color picker) dans les settings libmypaint.
    pub fn sync_from(&mut self, b: &Brush) {
        let hsv = rgb_to_hsv(
            b.color[0] as f32 / 255.0,
            b.color[1] as f32 / 255.0,
            b.color[2] as f32 / 255.0,
        );
        self.brush.set(BrushSetting::ColorH, SettingValue::constant(hsv.h));
        self.brush.set(BrushSetting::ColorS, SettingValue::constant(hsv.s));
        self.brush.set(BrushSetting::ColorV, SettingValue::constant(hsv.v));
        // libmypaint stocke le rayon en log naturel.
        self.brush.set(BrushSetting::Radius, SettingValue::constant((b.radius.max(1) as f32).ln()));
        self.brush.set(BrushSetting::Opaque, SettingValue::constant(b.opacity.clamp(0.0, 1.0)));
    }

    pub fn begin_stroke(&mut self) {
        self.stroke    = MemSurface::new();
        self.state     = BrushState::default();
        self.last_time = None;
    }

    /// Un échantillon pointeur (coordonnées canvas). hokusai interpole
    /// lui-même les dabs entre deux échantillons.
    pub fn stroke_to<S: PaintSurface>(&mut self, surface: &mut S, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        let now   = Instant::now();
        let dtime = self.last_time.map(|t| (now - t).as_secs_f64()).unwrap_or(0.0);
        self.last_time = Some(now);

        self.stroke.begin_atomic();
        self.brush.stroke_to(&mut self.state, &mut self.stroke, x, y, pressure, 0.0, 0.0, dtime);
        let tiles = self.stroke.end_atomic();
        self.composite_tiles(surface, &tiles)
    }

    /// Fin de trait : laisse hokusai rattraper la position lissée.
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

/// Brush rond "à la CSP" : bord net, opacité pilotée par la pression,
/// espacement serré pour un trait lisse. Couleur/taille/opacité de base
/// sont ensuite écrasées par sync_from().
fn base_brush() -> hokusai::Brush {
    let mut b = hokusai::Brush::new();
    b.set(BrushSetting::Hardness, SettingValue::constant(0.9));
    b.set(BrushSetting::AntiAliasing, SettingValue::constant(1.0));
    // 8 dabs par rayon : en dessous, anneaux visibles sur les gros diamètres.
    b.set(BrushSetting::DabsPerActualRadius, SettingValue::constant(8.0));
    // Valeur du pinceau rond stock de MyPaint : rampe de pression douce
    // sans que les dabs superposés saturent trop vite. 0 = extrémités
    // trop sombres à faible pression ; 0.9 = trait délavé.
    b.set(BrushSetting::OpaqueLinearize, SettingValue::constant(0.44));
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

    struct TestSurface { px: Vec<u8>, base: Vec<u8>, mask: Vec<u8>, w: u32, h: u32 }
    impl PaintSurface for TestSurface {
        fn width(&self) -> u32 { self.w }
        fn height(&self) -> u32 { self.h }
        fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) { (&mut self.px, &self.base, &mut self.mask) }
    }

    #[test]
    fn paints_black_on_white() {
        let (w, h) = (256u32, 256u32);
        let white = vec![255u8; (w * h * 4) as usize];
        let mut s = TestSurface { px: white.clone(), base: white, mask: vec![0; (w * h) as usize], w, h };

        let mut e = HokusaiEngine::new();
        let mut b = Brush::new();
        b.radius = 10;
        e.sync_from(&b);
        e.begin_stroke();
        let mut dirty = None;
        for i in 0..=50 {
            let x = 40.0 + i as f32 * 3.0;
            dirty = dirty.or(e.stroke_to(&mut s, x, 128.0, 1.0));
        }
        e.end_stroke(&mut s);

        assert!(dirty.is_some(), "aucune tuile modifiée");
        let center = ((128 * w + 100) * 4) as usize;
        assert!(s.px[center] < 40, "centre du trait pas assez sombre: {}", s.px[center]);
        let far = ((10 * w + 10) * 4) as usize;
        assert_eq!(&s.px[far..far + 4], &[255, 255, 255, 255]);
    }
}

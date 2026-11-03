//! The brush engine. For now a placeholder: round dabs (size, hardness,
//! opacity, blend mode, eraser, pressure to size), painted through
//! `core::PaintSurface`: each pixel keeps its highest coverage of the
//! stroke, so overlapping dabs never build up (CSP's stroke opacity).
//! PhotoCraft's engine takes its place; `Brush`, `PenSample` and the
//! `Forge` calls are what the app and the interface hold on to.

use aurastudio_core::blend::paint_pixel_u8;
use aurastudio_core::{BlendMode, PaintSurface, Rect, Rgba8, Span};

/// The settings of the brush in use (the color is global).
#[derive(Clone, Debug, PartialEq)]
pub struct Brush {
    pub color:    [u8; 3],
    pub opacity:  f32,
    /// Radius in canvas pixels; 0.5 is a 1 px brush.
    pub radius:   f32,
    /// How the brush color combines with what is already on the layer.
    pub blend:    BlendMode,
    /// Edge softness, 0..1. 1 = crisp edge (1 px anti-aliasing); lower
    /// values fade the coverage out from `radius * hardness` to `radius`.
    pub hardness: f32,
    /// Eraser: the dabs take opacity away instead of adding color.
    pub erase:    bool,
    /// The pen's pressure sets the size (from this share of it at no
    /// pressure, 0..1).
    pub min_size: f32,
    /// Dabs per diameter.
    pub spacing:  f32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25.0, blend: BlendMode::Normal, hardness: 1.0, erase: false, min_size: 0.2, spacing: 0.1 }
    }
}

impl Default for Brush {
    fn default() -> Self { Self::new() }
}

/// The toolbox group a brush belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind { Draw, Paint, Eraser }

/// A named brush (the placeholder set until PhotoCraft's presets).
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name:     String,
    pub kind:     Kind,
    pub settings: Brush,
}

impl Preset {
    /// The built-in brushes.
    pub fn builtin() -> Vec<Preset> {
        let b = Brush::new();
        let p = |name: &str, kind, settings| Preset { name: name.into(), kind, settings };
        vec![
            p("Pencil", Kind::Draw, Brush { radius: 2.0, hardness: 0.9, opacity: 0.9, min_size: 0.5, ..b.clone() }),
            p("Pen", Kind::Draw, Brush { radius: 3.0, hardness: 1.0, min_size: 0.05, ..b.clone() }),
            p("Marker", Kind::Draw, Brush { radius: 8.0, hardness: 1.0, opacity: 0.6, min_size: 1.0, ..b.clone() }),
            p("Round brush", Kind::Paint, Brush { radius: 25.0, hardness: 0.85, min_size: 0.3, ..b.clone() }),
            p("Soft brush", Kind::Paint, Brush { radius: 60.0, hardness: 0.1, opacity: 0.5, min_size: 0.6, ..b.clone() }),
            p("Hard eraser", Kind::Eraser, Brush { radius: 20.0, hardness: 1.0, erase: true, min_size: 1.0, ..b.clone() }),
            p("Soft eraser", Kind::Eraser, Brush { radius: 50.0, hardness: 0.1, erase: true, min_size: 1.0, ..b }),
        ]
    }
}

/// One pen position: canvas pixels, pressure 0..1, time in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub x:        f32,
    pub y:        f32,
    pub pressure: f32,
    pub time:     f64,
}

impl PenSample {
    pub fn new(x: f32, y: f32, pressure: f32, time: f64) -> Self { Self { x, y, pressure: pressure.clamp(0.0, 1.0), time } }
}

/// A stroke being painted.
pub struct Forge {
    brush: Brush,
    last:  Option<PenSample>,
    /// Distance walked since the last dab (px).
    rest:  f32,
}

impl Forge {
    pub fn new(brush: Brush) -> Self { Self { brush, last: None, rest: 0.0 } }

    pub fn brush(&self) -> &Brush { &self.brush }

    /// New settings for the next stroke.
    pub fn sync_from(&mut self, b: &Brush) { self.brush = b.clone(); }

    /// Radius of a dab at pressure `p`.
    fn radius(&self, p: f32) -> f32 {
        let b = &self.brush;
        (b.radius * (b.min_size + (1.0 - b.min_size) * p)).max(0.5)
    }

    /// Paint from the last sample to `p` (the first one: a dab there).
    /// Returns the area painted.
    pub fn stroke_to<S: PaintSurface>(&mut self, s: &mut S, p: PenSample) -> Option<Rect> {
        let Some(last) = self.last.replace(p) else {
            self.rest = 0.0;
            return self.dab(s, p.x, p.y, self.radius(p.pressure));
        };
        let (dx, dy) = (p.x - last.x, p.y - last.y);
        let len = dx.hypot(dy);
        let mut area: Option<Rect> = None;
        let mut t = 0.0;
        loop {
            let pr = last.pressure + (p.pressure - last.pressure) * (t / len.max(1e-6)).min(1.0);
            let step = (2.0 * self.radius(pr) * self.brush.spacing).max(0.5);
            let need = step - self.rest;
            if t + need > len { self.rest += len - t; break; }
            t += need;
            self.rest = 0.0;
            let k = t / len;
            if let Some(r) = self.dab(s, last.x + dx * k, last.y + dy * k, self.radius(pr)) { area = Some(area.map_or(r, |a| a.union(&r))); }
        }
        area
    }

    /// The stroke is over: the next one starts afresh.
    pub fn end_stroke<S: PaintSurface>(&mut self, _s: &mut S) -> Option<Rect> {
        self.last = None;
        self.rest = 0.0;
        None
    }

    /// One round dab at (x, y).
    fn dab<S: PaintSurface>(&self, s: &mut S, x: f32, y: f32, r: f32) -> Option<Rect> {
        let b = &self.brush;
        let rect = Rect::around((x - r - 1.0) as f64, (y - r - 1.0) as f64, (x + r + 1.0) as f64, (y + r + 1.0) as f64);
        if rect.is_empty() { return None; }
        s.prepare(rect);
        let inner = (r * b.hardness.clamp(0.0, 1.0)).min(r - 0.5).max(0.0);
        let fade = (r - inner).max(0.5);
        let opacity = b.opacity.clamp(0.0, 1.0) * 255.0;
        let paint = |sp: &mut Span<'_, Rgba8>| {
            let fy = sp.y as f32 + 0.5 - y;
            for i in 0..sp.len() {
                let fx = sp.x as f32 + i as f32 + 0.5 - x;
                let d = fx.hypot(fy);
                let c = ((r + 0.5 - d) / (fade + 0.5)).clamp(0.0, 1.0);
                let cov = (c * opacity + 0.5) as u8;
                if cov == 0 { continue; }
                let m = sp.cover(i, cov);
                if m == 0 { continue; }
                let base = sp.base[i];
                sp.pixels[i] = if b.erase {
                    [base[0], base[1], base[2], aurastudio_core::math::mul255(base[3], 255 - m)]
                } else {
                    paint_pixel_u8(base, b.color, m, b.blend)
                };
            }
        };
        let mut spans = Vec::new();
        if s.spans_mut(rect, &mut spans) {
            use rayon::prelude::*;
            if spans.len() > 64 { spans.par_iter_mut().for_each(paint); } else { spans.iter_mut().for_each(paint); }
        } else {
            for yy in rect.y0..rect.y1 {
                let mut xx = rect.x0;
                while xx < rect.x1 {
                    let mut sp = s.span_mut(xx, yy, (rect.x1 - xx) as usize);
                    let n = sp.len();
                    if n == 0 { break; }
                    paint(&mut sp);
                    xx += n as i32;
                }
            }
        }
        Some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_core::FlatSurface;

    #[test]
    fn a_stroke_paints_without_building_up() {
        let mut s = FlatSurface::new(100, 40, [255, 255, 255, 0]);
        let mut f = Forge::new(Brush { radius: 5.0, opacity: 0.5, color: [10, 20, 30], ..Brush::new() });
        f.stroke_to(&mut s, PenSample::new(10.0, 20.0, 1.0, 0.0));
        let r = f.stroke_to(&mut s, PenSample::new(90.0, 20.0, 1.0, 0.1)).unwrap();
        assert!(r.contains(50, 20));
        f.end_stroke(&mut s);
        let p = s.pixel(50, 20);
        assert_eq!(&p[..3], &[10, 20, 30]);
        assert!((p[3] as i32 - 128).abs() <= 1, "{p:?}");
        assert_eq!(s.pixel(50, 35)[3], 0);
    }

    #[test]
    fn the_eraser_takes_alpha_away() {
        let mut s = FlatSurface::new(20, 20, [9, 9, 9, 255]);
        let mut f = Forge::new(Brush { radius: 4.0, erase: true, ..Brush::new() });
        f.stroke_to(&mut s, PenSample::new(10.0, 10.0, 1.0, 0.0));
        assert_eq!(s.pixel(10, 10)[3], 0);
        assert_eq!(s.pixel(1, 1)[3], 255);
    }
}

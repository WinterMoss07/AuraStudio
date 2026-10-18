use aurastudio_canvas::BlendMode;

mod round_brush;
mod mypaint_engine;
mod raster;
pub use round_brush::RoundBrush;
pub use mypaint_engine::MyPaintEngine;
/// hokusai's brush type (a parsed .myb), for the brush library.
pub use hokusai::Brush as MyPaintBrush;
pub use hokusai::myb::from_str as parse_myb;

#[derive(Clone, Debug)]
pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    pub radius:  u32,
    /// How the brush color combines with what is already on the layer.
    pub blend:   BlendMode,
    /// Edge softness, 0..1. 1 = crisp edge (1 px anti-aliasing); lower
    /// values fade the coverage out from `radius * hardness` to `radius`.
    pub hardness: f32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25, blend: BlendMode::Normal, hardness: 1.0 }
    }
}

impl Default for Brush {
    fn default() -> Self { Self::new() }
}

/// Distance-based pressure smoothing of the round brush.
///
/// Pressure may only move toward the pen value in proportion to the
/// distance travelled, relative to the brush radius. Without it, a short
/// pressure spike leaves a full brush-sized disc at the higher opacity
/// (later, lighter samples can't lower it: no build-up means no "undo"
/// either), which shows as round blotches on big brushes.
/// Positions are never smoothed, so this adds no positional latency.
pub struct PressureSmoother {
    /// Smoothing distance as a fraction of the brush radius. 0 = raw.
    pub span: f32,
    last: Option<(f32, f32, f32)>,
}

impl PressureSmoother {
    pub fn new(span: f32) -> Self { Self { span, last: None } }

    pub fn reset(&mut self) { self.last = None; }

    pub fn apply(&mut self, x: f32, y: f32, pressure: f32, radius: f32) -> f32 {
        let span = radius * self.span;
        let p = match self.last {
            Some((lx, ly, lp)) if span > 0.0 => {
                let dist = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt();
                lp + (pressure - lp) * (dist / span).min(1.0)
            }
            _ => pressure,
        };
        self.last = Some((x, y, p));
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoother_limits_pressure_spikes() {
        let mut sm = PressureSmoother::new(0.5);
        let r = 100.0;
        assert_eq!(sm.apply(0.0, 0.0, 0.2, r), 0.2);
        // A spike to 1.0 over 5 px (span = 50 px) only moves 10% of the way.
        let p = sm.apply(5.0, 0.0, 1.0, r);
        assert!((p - 0.28).abs() < 1e-4, "{p}");
    }
}

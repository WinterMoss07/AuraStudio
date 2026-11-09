//! Forge, the brush engine: Photoshop-grade brushes (computed and sampled
//! tips, Shape Dynamics, Scattering, Texture, Dual Brush, Color Dynamics,
//! Transfer, Brush Pose, Noise, Wet Edges, Build-up, Smoothing, the Mixer
//! Brush), the brush library and the importers (Photoshop .abr, Procreate
//! .brush / .brushset).
//!
//! The model, the dynamics and the tips come from PhotoCraft's brush engine
//! (MIT OR Apache-2.0, see NOTICE.md). The pixels are AuraStudio's: dabs
//! build a private coverage buffer ([`render`]), and only its changed
//! tiles are painted through `core::PaintSurface` (the layer's tiles, the
//! stroke base kept by `raster`, the selection as a limit, runs on several
//! cores, 8-bit integer blends where the mode allows).
//!
//! Strokes are data ([`StrokePoint`]s and [`BrushSettings`]) so they replay
//! the same: every jitter is a hash of the brush seed and the dab index
//! ([`rng`]). Within one stroke coverage builds up with flow toward each
//! dab's opacity ceiling, and the stroke is painted once at the stroke
//! opacity (Photoshop's Opacity versus Flow).

use aurastudio_core::Point;
use serde::{Deserialize, Serialize};

mod brush;
mod curve;
mod dynamics;
mod engine;
pub mod import;
mod library;
mod mixer;
mod mode;
mod presets;
mod procedural;
mod render;
mod stroke;
mod rng;
mod tile;

pub use brush::{BrushSettings, TipShape};
pub(crate) use brush::{BrushPreset, Control, DualBrush, Dynamic, MAX_BRUSH_SIZE, MAX_SCATTER, MaskMode, Pattern, PatternStyle, Pose, ShapeDynamics, Transfer};
pub(crate) use curve::ResponseCurve;
pub(crate) use mixer::MixerSettings;
pub(crate) use tile::GrayTile;
pub use engine::{Forge, PenSample};
pub use library::{Importer, Kind, Library, Preset, ReadFile};
pub use mode::PaintMode;
pub use rng::stroke_seed;

/// The settings of a brush (the name the rest of the app uses).
pub type Brush = BrushSettings;

/// One input sample. Missing stylus data defaults to "mouse": full pressure, no tilt/rotation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StrokePoint {
    pub x: f64,
    pub y: f64,
    /// 0..1, 1 for mouse.
    pub pressure: f32,
    /// Degrees -90..90 (W3C Pointer Events `tiltX`/`tiltY`).
    pub tilt_x: f32,
    pub tilt_y: f32,
    /// Barrel rotation in degrees 0..360.
    pub rotation: f32,
    /// Airbrush stylus wheel 0..1.
    pub wheel: f32,
    /// Timestamp in milliseconds (airbrush build-up and smoothing catch-up).
    pub time: f64,
}

impl Default for StrokePoint {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, pressure: 1.0, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0, wheel: 1.0, time: 0.0 }
    }
}

impl StrokePoint {
    pub fn new(x: f64, y: f64, pressure: f32) -> Self {
        Self { x, y, pressure, ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: BrushSettings,
    pub points: Vec<StrokePoint>,
}

/// A dab placed along the stroke (after dynamics).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub center: Point,
    pub radius: f32,
    /// Flow: per-dab build-up amount.
    pub alpha: f32,
    /// Radians, counter-clockwise.
    pub angle: f32,
    pub roundness: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Opacity ceiling this dab builds up to (Transfer › Opacity).
    pub opacity: f32,
    /// Straight RGBA paint colour (Color Dynamics).
    pub color: [f32; 4],
    /// Texture depth for per-tip texturing.
    pub depth: f32,
    /// Position in the stroke (0 = first).
    pub index: u64,
    /// Brush Projection: direction (radians, counter-clockwise, y up) along which the tip is
    /// foreshortened, and the factor (1 = no projection).
    pub proj_angle: f32,
    pub proj_scale: f32,
    /// Mixer Brush multipliers of Wet and Mix (Transfer › Wetness/Mix Jitter), 1 = as set.
    pub wet: f32,
    pub mix: f32,
}

impl Dab {
    /// A plain round dab.
    pub fn round(center: Point, radius: f32, alpha: f32) -> Self {
        Self {
            center,
            radius,
            alpha,
            angle: 0.0,
            roundness: 1.0,
            flip_x: false,
            flip_y: false,
            opacity: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
            depth: 1.0,
            index: 0,
            proj_angle: 0.0,
            proj_scale: 1.0,
            wet: 1.0,
            mix: 1.0,
        }
    }
}

/// All primary dabs of a stroke (smoothing, pose, spacing, build-up and dynamics applied).
pub fn dabs(stroke: &Stroke) -> Vec<Dab> {
    let mut g = dynamics::DabGenerator::new(&stroke.brush, 1.0);
    let (mut out, mut dual) = (Vec::new(), Vec::new());
    g.push(&stroke.points, &mut out, &mut dual);
    g.finish(&mut out, &mut dual);
    out
}

/// Photoshop's round tip, measured (Photoshop 27.11, single dabs at Spacing 1000 %, hardness 0–100
/// at 40 px and checked at 100, 9 and 3 px): a solid core of radius `r0 · R`, then a Gaussian
/// shoulder `exp(-((d − r0) / s)²)` of width `s · R`, both by hardness (0, 0.1, … 1). A soft tip
/// reaches well past its nominal radius (hardness 0 is a Gaussian of σ ≈ 0.47 R: 8 % at 1.05 R).
const TIP_SHAPE: [(f32, f32); 11] =
    [(0.00, 0.664), (0.14, 0.58), (0.28, 0.50), (0.40, 0.44), (0.52, 0.36), (0.63, 0.29), (0.70, 0.24), (0.78, 0.19), (0.84, 0.15), (0.89, 0.11), (1.00, 0.0)];

/// Photoshop's anti-aliased edge: the shoulder never narrows below this many pixels, and the core
/// stays this far inside the radius (a hardness-100 tip is a disc with a ~1 px soft edge at any size).
const TIP_EDGE_PX: f32 = 1.2;

/// The core radius and shoulder width (both in pixels) of a round tip of `radius` and `hardness`.
#[inline]
pub fn tip_shape(radius: f32, hardness: f32) -> (f32, f32) {
    let h = if hardness.is_nan() { 0.0 } else { hardness.clamp(0.0, 1.0) } * 10.0;
    let i = (h.floor() as usize).min(9);
    let t = h - i as f32;
    let (a, b) = (TIP_SHAPE[i], TIP_SHAPE[i + 1]);
    let (r0, s) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let radius = if radius.is_finite() { radius.max(0.0) } else { 0.0 };
    // A tip of a few pixels keeps a narrower edge, so a small hard dab stays solid in the middle.
    // Never sharper than a quarter pixel, the small-tip supersampling step.
    let edge = TIP_EDGE_PX.min(0.3 * radius).max(0.25);
    ((r0 * radius).min(radius - edge).max(0.0), (s * radius).max(edge).max(1e-3))
}

/// Dab-invariant round-tip geometry, shared by all pixels and subpixel samples.
#[derive(Clone, Copy)]
pub(crate) struct TipProfile {
    core: f32,
    shoulder: f32,
    reach: f32,
}

impl TipProfile {
    #[inline]
    pub(crate) fn new(radius: f32, hardness: f32) -> Self {
        let (core, shoulder) = tip_shape(radius, hardness);
        Self { core, shoulder, reach: core + 2.6 * shoulder }
    }

    #[inline]
    pub(crate) fn reach(self) -> f32 {
        self.reach
    }

    #[inline]
    pub(crate) fn falloff(self, d: f32) -> f32 {
        if d <= self.core {
            return 1.0;
        }
        // Keep division: multiplying by a cached reciprocal changes coverage bits.
        let x = (d - self.core) / self.shoulder;
        soft_exp(x * x)
    }

    #[inline]
    pub(crate) fn coverage(self, d: f32) -> f32 {
        if d.is_nan() || d >= self.reach + 0.5 {
            return 0.0;
        }
        self.falloff(d)
    }
}

/// `exp(-u)` as `(1 − u/64)⁶⁴` by six squarings: within 1/255 of `exp` where it matters, exact
/// arithmetic (the same on every platform) and exactly 0 from `u = 64`.
#[inline]
fn soft_exp(u: f32) -> f32 {
    let mut g = (1.0 - u / 64.0).clamp(0.0, 1.0);
    for _ in 0..6 {
        g *= g;
    }
    g
}

/// Opacity of a round tip at distance `d` from its centre, edge included (see [`tip_shape`]).
#[cfg(test)]
pub(crate) fn tip_falloff(d: f32, radius: f32, hardness: f32) -> f32 {
    TipProfile::new(radius, hardness).falloff(d)
}

/// How far from its centre a round tip still paints (where its falloff drops below ~0.1 %).
#[inline]
pub(crate) fn tip_reach(radius: f32, hardness: f32) -> f32 {
    let (r0, s) = tip_shape(radius, hardness);
    r0 + 2.6 * s
}

/// Distance from the centre where a round tip drops to 50 %: the contour Preferences ›
/// Cursors › Normal Brush Tip outlines. `(1 − u/64)⁶⁴ = ½` at `u = 64 (1 − ½^(1/64))`.
#[inline]
pub(crate) fn half_coverage_radius(radius: f32, hardness: f32) -> f32 {
    let (r0, s) = tip_shape(radius, hardness);
    let u = 64.0 * (1.0 - 0.5f32.powf(1.0 / 64.0));
    r0 + s * u.sqrt()
}


#[cfg(test)]
mod brush_tests;

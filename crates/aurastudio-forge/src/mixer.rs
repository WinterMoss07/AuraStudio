//! Mixer Brush: a reservoir of loaded paint plus a pickup well that mixes in canvas colour.
//!
//! Model (behavioural, our own): each dab samples the canvas colour under its footprint. The pickup
//! colour `P` moves towards it by `wet`. The deposited colour mixes the reservoir `R` (weight
//! `amount × (1 − mix·wet)`) with the pickup (weight `mix·wet`); it lands at `flow` times the total
//! weight. The reservoir amount drains per dab unless `load` is 100 %, so low Load strokes dry out.
//! A dry brush (`wet = 0`) paints only reservoir colour; an empty, wet brush smears canvas colour.
//! From PhotoCraft (see NOTICE.md); the dabs are painted by `engine.rs`.

use serde::{Deserialize, Serialize};

/// Fraction of the way the pickup colour moves towards the canvas colour per dab at 100 % Wet.
pub const PICKUP_RATE: f32 = 0.25;

/// Mixer Brush options (all 0..1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MixerSettings {
    /// The brush is a Mixer Brush (its strokes mix with the canvas).
    pub enabled: bool,
    pub wet: f32,
    pub load: f32,
    pub mix: f32,
    pub flow: f32,
    /// Pick up colour from all visible layers instead of the target layer only.
    pub sample_all_layers: bool,
    /// Load the brush with the stroke color after each stroke (the automatic Load option).
    #[serde(alias = "reloadAfterStroke")]
    pub load_after_stroke: bool,
    /// Empty the brush after each stroke (the automatic Clean option).
    pub clean_after_stroke: bool,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self { enabled: false, wet: 0.5, load: 0.5, mix: 0.5, flow: 1.0, sample_all_layers: false, clean_after_stroke: true, load_after_stroke: true }
    }
}

impl MixerSettings {
    /// Effective per-dab wetness, including Transfer's Wetness control.
    pub fn effective_wet(&self, dab: &crate::Dab) -> f32 {
        self.wet.clamp(0.0, 1.0) * dab.wet.clamp(0.0, 1.0)
    }

    /// Whether this dab can update the pickup well from the canvas.
    pub fn samples_canvas(&self, dab: &crate::Dab) -> bool {
        self.effective_wet(dab) > 0.0
    }
}

/// Paint carried between strokes (session tool state).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MixerState {
    /// Loaded paint (straight RGBA), None = clean/unloaded.
    pub reservoir: Option<[f32; 4]>,
    /// Remaining reservoir amount 0..1.
    pub amount: f32,
    /// Colour picked up from the canvas (straight RGBA), None = clean.
    pub pickup: Option<[f32; 4]>,
    /// The foreground colour in effect when the reservoir was loaded from the canvas
    /// (`load_sampled`), None when it holds the foreground itself.
    pub sampled_over: Option<[f32; 4]>,
    /// A bounded 64 x 64 spatial load in tip-local coordinates. Empty cells carry no paint.
    pub spatial: Option<Vec<[f32; 4]>>,
    /// The color recorded by the last stroke (its color) or the last Load Brush or Clean Brush
    /// (the foreground). None = no stroke, load or clean yet.
    pub color: Option<[f32; 4]>,
}

impl MixerState {
    /// Load Brush: fill the reservoir with a color (full).
    pub fn load(&mut self, color: [f32; 4]) {
        self.spatial = None;
        self.reservoir = Some(color);
        self.amount = 1.0;
        self.sampled_over = None;
    }

    /// Load the reservoir with a colour sampled from the canvas (⌥-click) while `foreground` is
    /// the foreground colour; `holds_sample` stays true until the foreground changes.
    pub fn load_sampled(&mut self, color: [f32; 4], foreground: [f32; 4]) {
        self.load(color);
        self.sampled_over = Some(foreground);
        self.color = Some(foreground);
    }
    /// Clean Brush: empty the reservoir and the pickup well.
    pub fn clean(&mut self) {
        self.spatial = None;
        self.reservoir = None;
        self.amount = 0.0;
        self.pickup = None;
        self.sampled_over = None;
    }

    /// Whether the reservoir holds a canvas sample taken while `foreground` was the foreground.
    pub fn holds_sample(&self, foreground: [f32; 4]) -> bool {
        self.reservoir.is_some() && self.sampled_over == Some(foreground)
    }
    /// Before a stroke in `color`: choosing a color other than [`MixerState::color`] loads the
    /// reservoir with it.
    pub fn begin_stroke(&mut self, color: [f32; 4]) {
        if self.color != Some(color) {
            self.load(color);
            self.color = Some(color);
        }
    }
    /// After a stroke in `color`: the automatic Clean empties the brush, then the automatic Load
    /// fills the reservoir again.
    pub fn end_stroke(&mut self, color: [f32; 4], clean: bool, load: bool) {
        // Automatic Load replenishes a sampled tip as well as a solid-color load.
        // Move its bounded buffer through Clean instead of allocating a copy.
        let sampled = self.holds_sample(color).then_some(color);
        let spatial = if load { self.spatial.take() } else { None };
        let paint = if spatial.is_some() || sampled.is_some() { self.reservoir.unwrap_or(color) } else { color };
        if clean {
            self.clean();
        }
        if load {
            self.load(paint);
            self.spatial = spatial;
            self.sampled_over = sampled;
        }
        self.color = Some(color);
    }
    /// The reservoir color the next stroke in `color` starts with (the Current Brush Load
    /// swatch); None = a clean brush.
    pub fn next_load(&self, color: [f32; 4]) -> Option<[f32; 4]> {
        if self.color != Some(color) { Some(color) } else { self.reservoir }
    }
}

/// Sample a spatial load at a dab pixel using the dab's rotation, aspect and flips.
/// Keeping the reservoir in tip coordinates lets it follow subsequent brush poses.
pub fn load_at(load: &[[f32; 4]], dab: &crate::Dab, x: f32, y: f32) -> [f32; 4] {
    let mut dx = x - dab.center.x as f32;
    let mut dy = y - dab.center.y as f32;
    if dab.proj_scale < 0.999 {
        let (sin, cos) = dab.proj_angle.sin_cos();
        let along = (dx * cos - dy * sin) * (1.0 / dab.proj_scale.max(0.05) - 1.0);
        dx += along * cos;
        dy -= along * sin;
    }
    let (sin, cos) = dab.angle.sin_cos();
    let mut u = (dx * cos - dy * sin) / dab.radius.max(0.01);
    let mut v = (dx * sin + dy * cos) / (dab.radius * dab.roundness).max(0.5);
    if dab.flip_x {
        u = -u;
    }
    if dab.flip_y {
        v = -v;
    }
    if !(-1.0..1.0).contains(&u) || !(-1.0..1.0).contains(&v) {
        return [0.0; 4];
    }
    let col = ((u + 1.0) * 32.0) as usize;
    let row = ((v + 1.0) * 32.0) as usize;
    load.get(row * 64 + col).copied().unwrap_or([0.0; 4])
}

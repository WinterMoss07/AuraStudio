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
}

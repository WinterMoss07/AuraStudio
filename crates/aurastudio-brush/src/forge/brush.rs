//! The [`ForgeBrush`] value: a full configuration consumed by the stroke engine.

use std::collections::BTreeMap;

use crate::forge::mapping::SettingValue;
use crate::forge::setting::{BrushSetting, NUM_SETTINGS};

/// A setting hokusai doesn't (yet) recognise. Held as raw JSON so a parsed
/// brush can be re-serialised without losing data — important for working
/// with brush packs authored against newer libmypaint than this crate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnknownSetting {
    pub base_value: f32,
    /// `(input_name, points)` — input names are kept as strings so we can
    /// passthrough inputs hokusai doesn't recognise either.
    pub inputs: BTreeMap<String, Vec<(f32, f32)>>,
}

#[derive(Debug, Clone)]
pub struct ForgeBrush {
    pub version: u32,
    pub group: Option<String>,
    pub parent_brush_name: Option<String>,
    pub comment: Option<String>,
    settings: Vec<SettingValue>,
    /// Settings hokusai didn't recognise on parse, kept by string key for
    /// lossless round-trip via `hokusai-brush`.
    pub unknown_settings: BTreeMap<String, UnknownSetting>,
    /// What Forge adds to libmypaint's model.
    pub options: ForgeOptions,
}

/// How a brush's dabs reach the layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Painting {
    /// Each dab blends onto the layer as it lands (MyPaint): paint builds
    /// up, smudge and blending read what is already there.
    #[default]
    BuildUp,
    /// Krita's wash: each dab pulls a stroke mask toward its opacity, never
    /// past it, and the mask is blended onto the layer as it was before
    /// the stroke, by the brush's blend mode. A stroke never gets more
    /// opaque than its strongest dab.
    Wash,
}

/// Distance between dabs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Spacing {
    /// libmypaint's dabs_per_actual_radius / basic_radius / second.
    #[default]
    Settings,
    /// Krita's auto spacing: `coefficient × √diameter` px (an even edge at
    /// any size, few dabs for big brushes).
    Auto(f32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ForgeOptions {
    pub painting: Painting,
    pub spacing: Spacing,
    /// A dab where the pen lands (Krita); libmypaint waits for a move.
    pub dab_on_press: bool,
}

impl ForgeBrush {
    pub fn new() -> Self {
        Self {
            version: 3,
            group: None,
            parent_brush_name: None,
            comment: None,
            settings: vec![SettingValue::default(); NUM_SETTINGS],
            unknown_settings: BTreeMap::new(),
            options: ForgeOptions::default(),
        }
    }

    /// A round brush in Krita's way: wash painting, auto spacing, opacity
    /// × pressure. Size, hardness and opacity come from the sliders.
    pub fn round() -> Self {
        use crate::forge::{BrushInput, Combine, InputMapping};
        let mut b = Self::new();
        b.get_mut(BrushSetting::Radius).base_value = 25.0_f32.ln();
        b.get_mut(BrushSetting::Hardness).base_value = 1.0;
        b.set(BrushSetting::Opaque, SettingValue {
            base_value: 1.0,
            inputs: vec![InputMapping { input: BrushInput::Pressure, points: vec![(0.0, 0.0), (1.0, 1.0)] }],
            combine: Combine::Multiply,
            ..SettingValue::default()
        });
        b.options = ForgeOptions { painting: Painting::Wash, spacing: Spacing::Auto(0.8), dab_on_press: true };
        b
    }

    #[inline]
    pub fn get(&self, s: BrushSetting) -> &SettingValue {
        &self.settings[s.index()]
    }

    #[inline]
    pub fn get_mut(&mut self, s: BrushSetting) -> &mut SettingValue {
        &mut self.settings[s.index()]
    }

    pub fn set(&mut self, s: BrushSetting, v: SettingValue) {
        self.settings[s.index()] = v;
    }

    pub fn settings(&self) -> &[SettingValue] {
        &self.settings
    }
}

impl Default for ForgeBrush {
    fn default() -> Self {
        Self::new()
    }
}

//! A brush: every setting's base value and input curves, and Forge's
//! options.

use crate::forge::mapping::SettingValue;
use crate::forge::setting::{BrushSetting, NUM_SETTINGS};

#[derive(Debug, Clone)]
pub struct ForgeBrush {
    settings: Vec<SettingValue>,
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
    /// Krita's wash (wash.rs).
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
    /// A fixed distance in px, whatever the size (stroke previews: the
    /// dabs stay in place while the size changes).
    Fixed(f32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ForgeOptions {
    pub painting: Painting,
    pub spacing: Spacing,
    /// A dab where the pen lands (Krita); libmypaint waits for a move.
    pub dab_on_press: bool,
    /// Stroke previews: random numbers by dab place (seed n × stride), so
    /// a size change keeps each dab's grain.
    pub seeds_by_place: Option<u32>,
}

impl ForgeBrush {
    pub fn new() -> Self {
        Self { settings: vec![SettingValue::default(); NUM_SETTINGS], options: ForgeOptions::default() }
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
        });
        b.options = ForgeOptions { painting: Painting::Wash, spacing: Spacing::Auto(0.8), dab_on_press: true, seeds_by_place: None };
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

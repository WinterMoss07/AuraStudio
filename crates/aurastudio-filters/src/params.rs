//! What each filter is, for menus, dialogs and files: its name, its
//! values and their sliders, its defaults, its stable code.

use crate::color::{HueRange, BW_DEFAULTS};

/// How a value is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Percent,
    Pixels,
    Degrees,
    /// A tone level, 0..255.
    Level,
    /// A plain number with this many decimals.
    Number(usize),
}

/// A value with a plain slider in the dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub name:  &'static str,
    /// Its place in `Filter::values`.
    pub index: usize,
    pub min:   f32,
    pub max:   f32,
    pub unit:  Unit,
}

const fn p(name: &'static str, index: usize, min: f32, max: f32, unit: Unit) -> Param { Param { name, index, min, max, unit } }

/// The filters, as the Filter menu lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    BrightnessContrast, Levels, HueSaturation, BlackWhite,
    Invert, Posterize, Threshold, Sepia, Opacity,
    GaussianBlur, UnsharpMask,
    Thicken, Thin, FindEdges, Emboss,
    DropShadow,
}

/// Menu groups, in order.
pub const GROUPS: &[(&str, &[Kind])] = &[
    ("Adjust", &[Kind::BrightnessContrast, Kind::Levels, Kind::HueSaturation, Kind::BlackWhite]),
    ("Tones", &[Kind::Invert, Kind::Posterize, Kind::Threshold, Kind::Sepia, Kind::Opacity]),
    ("Blur & sharpen", &[Kind::GaussianBlur, Kind::UnsharpMask]),
    ("Lines & edges", &[Kind::Thicken, Kind::Thin, Kind::FindEdges, Kind::Emboss]),
    ("Effects", &[Kind::DropShadow]),
];

/// Value places of the filters with many values.
pub mod idx {
    /// Brightness / Contrast: brightness, contrast, legacy (0 / 1).
    pub const LEGACY: usize = 2;
    /// Levels: channel (0 RGB, 1 R, 2 G, 3 B) × 5 + in black, gamma, in
    /// white, out black, out white.
    pub const fn levels(ch: usize) -> usize { ch * 5 }
    /// Hue / Saturation: hue, saturation, lightness, colorize; then each
    /// range: hue, saturation, lightness, its four bounds.
    pub const COLORIZE: usize = 3;
    pub const fn range(i: usize) -> usize { 4 + i * 7 }
    /// Black & White: six weights, then tint (0 / 1; the tint is the
    /// filter's color).
    pub const TINT: usize = 6;
    /// Minimum / Maximum: radius, round (0 / 1).
    pub const ROUND: usize = 1;
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::BrightnessContrast => "Brightness / Contrast",
            Kind::Levels => "Levels",
            Kind::HueSaturation => "Hue / Saturation",
            Kind::BlackWhite => "Black & White",
            Kind::Invert => "Invert",
            Kind::Posterize => "Posterize",
            Kind::Threshold => "Threshold",
            Kind::Sepia => "Sepia",
            Kind::Opacity => "Opacity",
            Kind::GaussianBlur => "Gaussian blur",
            Kind::UnsharpMask => "Unsharp mask",
            Kind::Thicken => "Thicken lines",
            Kind::Thin => "Thin lines",
            Kind::FindEdges => "Find edges",
            Kind::Emboss => "Emboss",
            Kind::DropShadow => "Drop shadow",
        }
    }

    /// One line under the dialog's title.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::BrightnessContrast => "Lighter, darker, more or less contrast",
            Kind::Levels => "Black point, white point, midtones",
            Kind::HueSaturation => "Hue, saturation, lightness",
            Kind::BlackWhite => "Grayscale, each color its own lightness",
            Kind::Invert => "Negative",
            Kind::Posterize => "A few flat tones",
            Kind::Threshold => "Black or white",
            Kind::Sepia => "Old-photo browns",
            Kind::Opacity => "Fade the layer",
            Kind::GaussianBlur => "Soft blur",
            Kind::UnsharpMask => "Crisper edges",
            Kind::Thicken => "Dark lines grow",
            Kind::Thin => "Dark lines shrink",
            Kind::FindEdges => "Outlines, dark on white",
            Kind::Emboss => "Gray relief",
            Kind::DropShadow => "A soft shadow under the layer",
        }
    }

    /// The values that get a plain slider (Levels, Hue / Saturation and
    /// Black & White have their own editors).
    pub fn params(self) -> &'static [Param] {
        use Unit::*;
        match self {
            Kind::BrightnessContrast => { const P: &[Param] = &[p("Brightness", 0, -150.0, 150.0, Number(0)), p("Contrast", 1, -50.0, 100.0, Number(0))]; P }
            Kind::Levels | Kind::HueSaturation | Kind::BlackWhite => &[],
            Kind::Invert | Kind::Sepia => { const P: &[Param] = &[p("Amount", 0, 0.0, 100.0, Percent)]; P }
            Kind::Opacity => { const P: &[Param] = &[p("Opacity", 0, 0.0, 100.0, Percent)]; P }
            Kind::Posterize => { const P: &[Param] = &[p("Levels", 0, 2.0, 64.0, Number(0))]; P }
            Kind::Threshold => { const P: &[Param] = &[p("Threshold level", 0, 1.0, 255.0, Level)]; P }
            Kind::GaussianBlur => { const P: &[Param] = &[p("Radius", 0, 0.0, 250.0, Pixels)]; P }
            Kind::UnsharpMask => { const P: &[Param] = &[p("Amount", 0, 1.0, 500.0, Percent), p("Radius", 1, 0.1, 100.0, Pixels), p("Threshold", 2, 0.0, 255.0, Level)]; P }
            Kind::Thicken | Kind::Thin => { const P: &[Param] = &[p("Radius", 0, 0.0, 50.0, Pixels)]; P }
            Kind::FindEdges => { const P: &[Param] = &[p("Strength", 0, 0.0, 100.0, Percent)]; P }
            Kind::Emboss => { const P: &[Param] = &[p("Height", 1, 1.0, 20.0, Pixels), p("Amount", 2, 1.0, 500.0, Percent)]; P }
            Kind::DropShadow => { const P: &[Param] = &[
                p("Distance", 1, 0.0, 300.0, Pixels),
                p("Spread", 2, 0.0, 100.0, Percent),
                p("Size", 3, 0.0, 250.0, Pixels),
                p("Opacity", 4, 0.0, 100.0, Percent),
            ]; P }
        }
    }

    /// Every filter, in menu order.
    pub fn all() -> impl Iterator<Item = Kind> { GROUPS.iter().flat_map(|g| g.1.iter().copied()) }

    /// Its code in files (stable: never reused, never renumbered).
    pub fn code(self) -> u8 {
        match self {
            Kind::BrightnessContrast => 1, Kind::Levels => 2, Kind::HueSaturation => 3, Kind::BlackWhite => 4,
            Kind::Invert => 5, Kind::Posterize => 6, Kind::Threshold => 7, Kind::Sepia => 8, Kind::Opacity => 9,
            Kind::GaussianBlur => 10, Kind::UnsharpMask => 11,
            Kind::Thicken => 12, Kind::Thin => 13, Kind::FindEdges => 14, Kind::Emboss => 15,
            Kind::DropShadow => 16,
        }
    }

    pub fn from_code(c: u8) -> Option<Kind> { Kind::all().find(|k| k.code() == c) }

    /// Each pixel on its own (no neighbor read): tone tables, colors,
    /// opacity.
    pub fn is_point(self) -> bool { matches!(self, Kind::BrightnessContrast | Kind::Levels | Kind::HueSaturation | Kind::BlackWhite | Kind::Invert | Kind::Posterize | Kind::Threshold | Kind::Sepia | Kind::Opacity) }

    pub fn defaults(self) -> Vec<f32> {
        match self {
            Kind::BrightnessContrast => vec![0.0, 0.0, 0.0],
            Kind::Levels => (0..4).flat_map(|_| [0.0, 1.0, 255.0, 0.0, 255.0]).collect(),
            Kind::HueSaturation => {
                let mut v = vec![0.0; 4];
                for i in 0..6 { let r = HueRange::neutral(i); v.extend([0.0, 0.0, 0.0, r.bounds[0], r.bounds[1], r.bounds[2], r.bounds[3]]); }
                v
            }
            Kind::BlackWhite => { let mut v = BW_DEFAULTS.to_vec(); v.push(0.0); v }
            Kind::Invert | Kind::Sepia | Kind::FindEdges => vec![100.0],
            Kind::Opacity => vec![50.0],
            Kind::Posterize | Kind::GaussianBlur => vec![4.0],
            Kind::Threshold => vec![128.0],
            Kind::UnsharpMask => vec![80.0, 2.0, 0.0],
            Kind::Thicken | Kind::Thin => vec![1.0, 0.0],
            Kind::Emboss => vec![135.0, 3.0, 100.0],
            Kind::DropShadow => vec![120.0, 12.0, 0.0, 16.0, 60.0],
        }
    }
}

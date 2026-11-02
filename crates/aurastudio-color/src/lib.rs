//! Color management. Two things kept apart: what the document's numbers
//! mean (its `Profile`: sRGB by default, Display P3, any ICC file; assign
//! and convert are the document's edits, conversions are here), and how
//! they are shown (`DisplaySetup` → a `core::DisplayLut` the GPU applies:
//! the monitor's profile, a view mode, a soft proof). The UI chrome is
//! not converted. moxcms does the ICC work.
//!
//! moxcms 0.9 has no black point compensation (its option is commented
//! out): not offered until it does.

// Color code indexes channels side by side.
#![allow(clippy::needless_range_loop)]

mod convert;
mod display;
mod lab;
mod monitor;
mod profile;

pub use convert::{convert_buffer, Converter};
pub use display::{Channel, DisplaySetup, Proof, ViewMode, LUT_SIZE, PROOF_LUT_SIZE};
pub use lab::{delta_e, Lab};
pub use monitor::{monitor_profile, MonitorChoice};
pub use profile::{Model, Profile};

/// What went wrong, in words for the log and the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorError(pub String);

impl std::fmt::Display for ColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

impl std::error::Error for ColorError {}

pub(crate) fn err(what: &str, e: impl std::fmt::Debug) -> ColorError { ColorError(format!("{what}: {e:?}")) }

/// How colors outside the destination's gamut are brought in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Intent {
    /// Fits the whole source gamut, keeping relations between colors.
    /// Only differs from relative colorimetric for LUT based profiles;
    /// matrix profiles have no perceptual table.
    #[default]
    Perceptual,
    /// Exact colors where the destination can show them, white matched.
    RelativeColorimetric,
    /// Vivid colors first (charts, flat colors).
    Saturation,
    /// Exact colors, white not matched (the paper's tint shows).
    AbsoluteColorimetric,
}

impl Intent {
    pub const ALL: [Intent; 4] = [Intent::Perceptual, Intent::RelativeColorimetric, Intent::Saturation, Intent::AbsoluteColorimetric];

    pub fn name(self) -> &'static str {
        match self {
            Intent::Perceptual => "Perceptual",
            Intent::RelativeColorimetric => "Relative colorimetric",
            Intent::Saturation => "Saturation",
            Intent::AbsoluteColorimetric => "Absolute colorimetric",
        }
    }

    pub fn code(self) -> u8 { self as u8 }
    pub fn from_code(c: u8) -> Option<Intent> { Intent::ALL.get(c as usize).copied() }

    pub(crate) fn to_moxcms(self) -> moxcms::RenderingIntent {
        use moxcms::RenderingIntent as R;
        match self {
            Intent::Perceptual => R::Perceptual,
            Intent::RelativeColorimetric => R::RelativeColorimetric,
            Intent::Saturation => R::Saturation,
            Intent::AbsoluteColorimetric => R::AbsoluteColorimetric,
        }
    }
}

#[cfg(test)]
mod tests;

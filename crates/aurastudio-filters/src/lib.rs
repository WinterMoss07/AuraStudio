//! Filters: tones, colors, sharpening, lines, edges and shadow as
//! Photoshop (PhotoCraft's measurements, MIT, see NOTICE); blur, sepia,
//! invert, opacity per the W3C Filter Effects spec. Straight 8-bit RGBA,
//! blurs premultiplied, edges repeated.
//!
//! `params`: what each filter is (menus, dialogs, files); `filter`: its
//! values and how it runs on pixels; `layer`: on surfaces (a layer inside
//! a selection, a mask's feather / grow / shrink), only where it changes
//! something; `histogram`: Levels' and Threshold's counts.

// Pixel code indexes channels side by side (p[c], table[c]...).
#![allow(clippy::needless_range_loop)]

pub mod blur;
pub mod color;
pub mod extreme;
mod filter;
mod histogram;
mod layer;
pub mod lut;
mod params;
pub mod spatial;
pub mod tone;

pub use color::{HueRange, BW_DEFAULTS, RANGE_NAMES};
pub use filter::{Filter, Prepared};
pub use histogram::Histogram;
pub use layer::{blur_mask, mix, spread_mask, StopFn, Stopped};
pub use lut::Lut8;
pub use params::{idx, Kind, Param, Unit, GROUPS};
pub use tone::LevelsChannel;

/// How far around the painted area (or the selection) a filter session
/// works at least, px (a 100 px blur, a shadow moved 200 px); a filter
/// reaching farther (`Filter::reach`) gets more.
pub const MARGIN: u32 = 512;

#[cfg(test)]
mod tests;

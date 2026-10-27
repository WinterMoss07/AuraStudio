//! Forge, the brush engine (overview in engine.rs). Its core comes from
//! hokusai (MIT OR Apache-2.0), a Rust port of libmypaint: see NOTICE.md.

pub mod brush;
pub mod brushmodes;
pub mod color;
mod engine;
pub mod evaluator;
pub mod fix15;
pub mod input;
pub mod mapping;
pub mod myb;
mod raster;
pub mod rng;
pub mod setting;
pub mod spectral;
pub mod state;
pub mod stroke;
pub mod surface;
pub mod tile;
mod wash;

pub use brush::{ForgeBrush, ForgeOptions, Painting, Spacing};
pub use engine::{Forge, PenSample};
pub use input::{BrushInput, NUM_INPUTS};
pub use mapping::{Combine, InputMapping, SettingValue};
pub use setting::{BrushSetting, NUM_SETTINGS};
pub use state::BrushState;
pub use surface::{Dab, TiledSurface};

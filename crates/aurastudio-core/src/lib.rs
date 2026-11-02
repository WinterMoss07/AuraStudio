//! The shared vocabulary of AuraStudio and the small pure helpers every
//! crate would otherwise copy. Entry rule: used (or about to be) by two
//! or more crates, pure, without storage or big computation. No
//! dependency, internal or external, and no cost at run time.

pub mod blend;
pub mod diag;
pub mod display;
pub mod doc;
pub mod geom;
pub mod math;
pub mod rng;
pub mod size;
pub mod surface;
pub mod time;

pub use blend::BlendMode;
pub use display::DisplayLut;
pub use doc::{DocType, PixelDepth};
pub use geom::{Affine, Point, Rect, Vec2};
pub use rng::{Fnv, Rng};
pub use size::HeapSize;
pub use surface::{FlatSurface, PaintSurface, Span};
pub use time::{Fps, FrameNo, Micros};

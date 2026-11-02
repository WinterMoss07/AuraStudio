//! What the tools do with the pen, apart from the window: shapes, the
//! multi-arc, the transform handles. No window, GPU or document here.
//! Points are `core::Point`, canvas pixels.

pub mod arc;
pub mod shapes;
pub mod xform;

pub use arc::{ArcPreview, ArcTool};
pub use xform::{Hit, Mods, Polylines, TransformMode, TransformNumbers, Xform};

//! The pixels of one surface (a layer, a mask), and nothing else: tiles,
//! their forms, the stroke in progress, undo patches, save and load. It
//! knows no layer, cel, document, brush or fill: other crates talk to it
//! through `Surface<P>`, rects, runs and chunks, and never name a tile.
//!
//! Tile forms change by themselves: the fill (nothing stored), one color
//! (no pixels), pixels (shared between copies until one writes).

mod patch;
mod save;
mod stroke;
mod surface;

pub use patch::{Patch, Snapshot};
pub use save::legacy;
pub use stroke::Stroke;
pub use surface::{Run, Surface, MAX_TILE, MIN_TILE};

/// A layer's pixels.
pub type Image = Surface<aurastudio_core::Rgba8>;
/// A one-channel surface: masks, selections, a stroke limit.
pub type Mask = Surface<u8>;

#[cfg(test)]
mod tests;

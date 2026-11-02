//! The pixels of one surface (a layer, a mask), and nothing else: tiles,
//! their forms, the stroke in progress, undo patches, save and load. It
//! knows no layer, cel, document, brush or fill: other crates talk to it
//! through `Surface<P>`, rects, runs, chunks and tile sets, and never name
//! a tile.
//!
//! Tile forms change by themselves: the fill (nothing stored), one color
//! (no pixels), one color with a coverage byte per pixel (line art: a
//! quarter of the bytes), pixels (shared between copies until one
//! writes), cold (compressed, for what is not shown).

mod dirty;
mod ops;
mod patch;
pub mod pool;
mod save;
mod slot;
mod stroke;
mod surface;

pub use dirty::DirtyTiles;
pub use ops::Combine;
pub use patch::{Patch, Snapshot};
pub use save::{legacy, SaveCache, SaveStats};
pub use slot::Summary;
pub use stroke::Stroke;
pub use surface::{Run, Stored, Surface, SummaryGrid, MAX_TILE, MIN_TILE};

/// A layer's pixels (8-bit documents).
pub type Image = Surface<aurastudio_core::Rgba8>;
/// A one-channel surface: masks, selections, a stroke limit.
pub type Mask = Surface<u8>;

#[cfg(test)]
mod tests;

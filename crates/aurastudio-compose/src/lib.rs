//! The merged image: what the tree (`layers`) and its surfaces (`raster`)
//! look like together over a rect. One routine for every merge (screen,
//! export, references, a filter's preview, the fill's view): pieces of
//! the canvas merged on all cores, in premultiplied floats, node by node
//! (W3C compositing; pass-through and isolated folders, clipping groups,
//! filter layers with their reach, onion ghosts), each run as the storage
//! holds it (a one-color run blends once), and nothing under an opaque
//! normal layer that covers a piece. Caches keyed by content (fingerprints
//! of the tiles involved): the stroke's under / over merges and isolated
//! folders' flattened content. Also: whether the GPU could merge the tree.

// Pixel code indexes channels side by side.
#![allow(clippy::needless_range_loop)]

mod extra;
mod key;
mod merge;
mod stroke;

pub use extra::{gpu_mergeable, merge_around, merge_split, preview, Merged};
pub use merge::{margin, merge, merge_into, needs_general, Backdrop, Opts};
pub use stroke::StrokeCache;

#[cfg(test)]
mod tests;

//! What the document tree IS (Krita / CSP): paint layers, folders, filter
//! layers and selection masks, bottom to top, each with a stable id and
//! its marks; the structure rules (where a node may go, one reference per
//! folder, clipping, locks), the fill tool's area, the tree's undo. Not
//! here: merging (`compose`), time (`animation`: this crate holds the
//! animation folders' data, never the current frame), commands and
//! history (`document`).
//!
//! Modules: `node` (the nodes), `anim` (animation folders as data),
//! `stack` (the tree: lookup, insert, take, ids, ghosts), `edit`
//! (structure edits), `paint` (pixels of paint layers), `rows` (the layer
//! list), `tree` (undo of tree edits), `fill` (the fill area).

// Pixel code indexes channels side by side.
#![allow(clippy::needless_range_loop)]

pub mod anim;
mod edit;
pub mod fill;
mod node;
mod paint;
mod rows;
mod stack;
mod tree;

pub use anim::{Anim, Cel, Column, FolderKind, Onto, Shift, Target, MAX_FRAME};
pub use edit::Place;
pub use fill::{Area, FillParams, FillSource, Refer};
pub use node::{ghosts_in, FilterLayer, Folder, FolderBlend, Footprint, Ghost, Layer, MaskLayer, Node, Pick, Props, LABELS};
pub use rows::Row;
pub use stack::{area_of, LayerStack, Start, DEFAULT_MARGIN, MAX_MARGIN};
pub use tree::TreeState;

#[cfg(test)]
mod tests;

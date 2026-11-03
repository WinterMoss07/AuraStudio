//! An open document: the layer tree, its time (`animation`'s Timeline,
//! settled after every edit), its history (patches of the tiles that
//! changed, tree states, selections), the selection, the filter and
//! transform sessions, copy / paste; the workspace of open documents.
//! Every change goes through `Document::apply(Edit)` and says what it
//! changed (`Done`). Saving and opening go through `file`'s neutral
//! `Snapshot`.

mod clip;
mod document;
mod edit;
mod filter;
mod history;
mod region;
mod saved;
mod select;
mod transform;
mod workspace;

pub use clip::Clip;
pub use document::{DocId, Document, DEFAULT_DPI};
pub use edit::Edit;
pub use history::{History, DEFAULT_BUDGET};
pub use region::{Done, Region};
pub use select::{SelectOp, Shape, Wand};
pub use transform::{Lift, LiftLayer};
pub use workspace::{SharedDoc, Workspace};
// What the interface shows of the pixels, the selection and the transform:
// it reaches them through the document.
pub use aurastudio_raster::{Image, Mask, Run};
pub use aurastudio_selection::{Mode as SelectMode, Segment, Selection};
pub use aurastudio_transform::{Interp, Warp};

#[cfg(test)]
mod tests;

//! `.aurs` documents (a zip like .kra / .ora, entries stored: mimetype,
//! document.json, one surface per layer, the selection, the document's
//! ICC profile, history, preview.png) and image import / export. Reads
//! and writes a neutral `Snapshot` the document fills and builds itself
//! back from: this crate knows no document, history or timeline. Reading
//! checks everything: a damaged file gives an error, never a crash or a
//! huge allocation.
//!
//! `schema`: document.json and its migrations; `write` / `read`: the
//! archive; `image_io`: pictures; `limits`: the bounds every read keeps.

mod image_io;
mod limits;
mod read;
mod schema;
mod snapshot;
mod write;

pub use image_io::{can_open, downscale, export_image, import_image, preview, thumbnail, EXPORT_EXTENSIONS, IMAGE_EXTENSIONS};
pub use limits::EXTENSION;
pub use read::{open, peek_origin, read};
pub use schema::FORMAT_VERSION;
pub use snapshot::{OnionMarks, Snapshot, Step, SAVED_UNDO_STEPS};
pub use write::{save, write, write_with, SaveCaches};

/// Document name shown for a file: its name without extension.
pub fn doc_name(path: &std::path::Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
}

#[cfg(test)]
mod tests;

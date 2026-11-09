//! What a save writes and an open gives back, neutral: the tree with its
//! surfaces (shared tiles: copying it is cheap, the document's lock is
//! short), the document's settings, the selection, the history's steps as
//! the document encoded them.

use std::path::PathBuf;

use aurastudio_core::{DocType, Fps, PixelDepth, Rgba8};
use aurastudio_layers::LayerStack;
use aurastudio_raster::Mask;

/// Undo steps kept in a saved file (the newest ones).
pub const SAVED_UNDO_STEPS: usize = 50;

/// One history step: its label, the layer it is about, the area (old
/// rect-based steps), its bytes (the document's encoding).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub label:  String,
    pub layer:  u64,
    pub rect:   Option<[i32; 4]>,
    pub data:   Vec<u8>,
    /// Undone with the step before it in the list, as one.
    pub joined: bool,
    /// It made a sheet first, undone with it: the sheet's id, its folder
    /// and its index there.
    pub sheet:  Option<[u64; 3]>,
}

/// The onion skin's marks (animation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OnionMarks {
    pub enabled: bool,
    pub mobile:  Vec<(i32, Option<f32>)>,
    pub fixed:   Vec<(u32, Option<f32>)>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub stack:     LayerStack,
    pub dpi:       f32,
    pub kind:      DocType,
    pub depth:     PixelDepth,
    /// The current frame and the frame rate (animation).
    pub frame:     u32,
    pub fps:       Fps,
    pub onion:     Option<OnionMarks>,
    /// The document's color profile (name, ICC bytes); `None`: sRGB.
    pub profile:   Option<(String, Vec<u8>)>,
    /// The active selection.
    pub selection: Option<Mask>,
    pub undo:      Vec<Step>,
    pub redo:      Vec<Step>,
    /// Merged image, small: (width, height, pixels).
    pub preview:   Option<(u32, u32, Vec<Rgba8>)>,
    /// Autosaves only: the file the work belongs to.
    pub origin:    Option<PathBuf>,
    /// Document revision this snapshot is of.
    pub revision:  u64,
}

impl Snapshot {
    /// A snapshot of `stack` with default settings.
    pub fn of(stack: LayerStack) -> Self {
        Self { stack, dpi: 300.0, kind: DocType::default(), depth: PixelDepth::default(), frame: 1, fps: Fps::default(), onion: None, profile: None, selection: None, undo: Vec::new(), redo: Vec::new(), preview: None, origin: None, revision: 0 }
    }
}

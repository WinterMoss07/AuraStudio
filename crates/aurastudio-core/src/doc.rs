//! Small document-level enums read by several crates.

/// What a document is made for: it turns features on or off (an animation
/// folder only in an Animation document…), the interface stays the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DocType {
    #[default]
    Illustration,
    Animation,
    Comics,
}

impl DocType {
    pub const ALL: [DocType; 3] = [DocType::Illustration, DocType::Animation, DocType::Comics];

    pub fn name(self) -> &'static str {
        match self { DocType::Illustration => "Illustration", DocType::Animation => "Animation", DocType::Comics => "Comics" }
    }

    /// From `name` (ignoring case); `None`: unknown.
    pub fn from_name(name: &str) -> Option<DocType> {
        DocType::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(name))
    }

    /// Side of the tiles new layers get by default (a Preference may
    /// change it): small brushes on HD / 4K animation canvases, B4 600 dpi
    /// pages in comics, large paintings in illustration.
    pub fn default_tile_size(self) -> u32 {
        match self { DocType::Animation => 16, DocType::Comics => 32, DocType::Illustration => 128 }
    }
}

/// Bits per channel of a document's painted layers, chosen at creation
/// (8 by default: enough for digital painting; 16 for lasso / gradient
/// heavy work).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PixelDepth {
    #[default]
    U8,
    U16,
    F16,
    F32,
}

impl PixelDepth {
    pub const ALL: [PixelDepth; 4] = [PixelDepth::U8, PixelDepth::U16, PixelDepth::F16, PixelDepth::F32];

    pub fn name(self) -> &'static str {
        match self { PixelDepth::U8 => "8 bit", PixelDepth::U16 => "16 bit", PixelDepth::F16 => "16 bit float", PixelDepth::F32 => "32 bit float" }
    }

    /// Bytes of one channel.
    pub fn channel_bytes(self) -> usize {
        match self { PixelDepth::U8 => 1, PixelDepth::U16 | PixelDepth::F16 => 2, PixelDepth::F32 => 4 }
    }
}

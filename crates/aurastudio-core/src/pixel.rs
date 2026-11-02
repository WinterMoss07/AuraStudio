//! Pixel types. A surface holds one pixel type for its whole life (fixed
//! at its creation); hot loops are written once, generic over it, and
//! compiled for each. Only 8-bit RGBA and one-channel 8-bit exist yet; 16
//! and 32 bit come without changing the code that uses `Pixel`.

/// The pixel types a surface can hold (saved in its header).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelKind {
    /// Straight (not premultiplied) RGBA, 8 bit per channel.
    Rgba8,
    /// One channel, 8 bit: masks, selections, coverage.
    Gray8,
}

impl PixelKind {
    pub fn code(self) -> u8 { match self { PixelKind::Rgba8 => 1, PixelKind::Gray8 => 2 } }

    pub fn from_code(c: u8) -> Option<PixelKind> {
        match c { 1 => Some(PixelKind::Rgba8), 2 => Some(PixelKind::Gray8), _ => None }
    }
}

pub trait Pixel: Copy + Default + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static {
    const KIND: PixelKind;
    /// Bytes of one pixel, as stored and saved.
    const BYTES: usize;
    /// Something shows (alpha > 0; a mask value > 0).
    fn visible(self) -> bool;
    /// Fully covering (alpha 255; a mask value 255).
    fn opaque(self) -> bool;
    /// Append its bytes (little endian).
    fn write_le(self, out: &mut Vec<u8>);
    /// From `BYTES` bytes (little endian); `None` if too short.
    fn read_le(b: &[u8]) -> Option<Self>;
}

/// Straight RGBA, 8 bit.
pub type Rgba8 = [u8; 4];

impl Pixel for [u8; 4] {
    const KIND: PixelKind = PixelKind::Rgba8;
    const BYTES: usize = 4;
    #[inline(always)]
    fn visible(self) -> bool { self[3] > 0 }
    #[inline(always)]
    fn opaque(self) -> bool { self[3] == 255 }
    fn write_le(self, out: &mut Vec<u8>) { out.extend_from_slice(&self); }
    fn read_le(b: &[u8]) -> Option<Self> { b.get(..4)?.try_into().ok() }
}

/// One channel, 8 bit.
pub type Gray8 = u8;

impl Pixel for u8 {
    const KIND: PixelKind = PixelKind::Gray8;
    const BYTES: usize = 1;
    #[inline(always)]
    fn visible(self) -> bool { self > 0 }
    #[inline(always)]
    fn opaque(self) -> bool { self == 255 }
    fn write_le(self, out: &mut Vec<u8>) { out.push(self); }
    fn read_le(b: &[u8]) -> Option<Self> { b.first().copied() }
}

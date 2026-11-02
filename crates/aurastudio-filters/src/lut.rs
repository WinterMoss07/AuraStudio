//! 8-bit tone tables: one input level → output level table per color
//! channel (tone filters, their curve in the dialog).

use aurastudio_core::{math, Rgba8};

/// One table of 256 levels.
pub type Table = [u8; 256];

/// A table per color channel (R, G, B); alpha untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lut8(pub [Table; 3]);

impl Lut8 {
    pub const IDENTITY: Lut8 = Lut8([identity(); 3]);

    /// The same table for the three channels.
    pub fn same(t: Table) -> Self { Lut8([t; 3]) }

    pub fn is_identity(&self) -> bool { *self == Self::IDENTITY }

    #[inline(always)]
    pub fn map(&self, p: Rgba8) -> Rgba8 { [self.0[0][p[0] as usize], self.0[1][p[1] as usize], self.0[2][p[2] as usize], p[3]] }
}

const fn identity() -> Table {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 { t[i] = i as u8; i += 1; }
    t
}

/// The table of a curve on 0..1 (rounded once, by `core::math`).
pub fn table(f: impl Fn(f32) -> f32) -> Table {
    std::array::from_fn(|i| math::to_u8(f(math::from_u8(i as u8)).clamp(0.0, 1.0)))
}

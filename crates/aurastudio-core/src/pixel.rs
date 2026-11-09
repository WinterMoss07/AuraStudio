//! Pixel types. A surface holds one pixel type for its whole life (fixed
//! at its creation); hot loops are written once, generic over `Pixel`, and
//! compiled for each type. Storage is straight alpha (an almost
//! transparent pixel keeps its exact color); calculations go through
//! premultiplied floats.

use crate::math::{div255, from_u8, to_u8};

/// The pixel types a surface can hold (saved in its header).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelKind {
    /// Straight RGBA, 8 bit per channel.
    Rgba8,
    /// One channel, 8 bit: masks, selections, coverage.
    Gray8,
    /// Straight RGBA, 16 bit integer per channel.
    Rgba16,
    /// Straight RGBA, 16 bit float per channel.
    RgbaF16,
    /// Straight RGBA, 32 bit float per channel.
    RgbaF32,
}

impl PixelKind {
    pub fn code(self) -> u8 {
        match self { PixelKind::Rgba8 => 1, PixelKind::Gray8 => 2, PixelKind::Rgba16 => 3, PixelKind::RgbaF16 => 4, PixelKind::RgbaF32 => 5 }
    }

    pub fn from_code(c: u8) -> Option<PixelKind> {
        match c { 1 => Some(PixelKind::Rgba8), 2 => Some(PixelKind::Gray8), 3 => Some(PixelKind::Rgba16), 4 => Some(PixelKind::RgbaF16), 5 => Some(PixelKind::RgbaF32), _ => None }
    }
}

/// Premultiplied RGBA, 0..1 (values above 1 allowed for float pixels).
pub type PremulF = [f32; 4];

pub trait Pixel: Copy + Default + PartialEq + Eq + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static {
    const KIND: PixelKind;
    /// Bytes of one pixel, as stored and saved.
    const BYTES: usize;
    /// Whether a tile of this type may be stored as one color plus one
    /// coverage byte per pixel (the alpha-only form): 8-bit RGBA only.
    const ALPHA_FORM: bool = false;

    /// Something shows (alpha > 0; a mask value > 0).
    fn visible(self) -> bool;
    /// Fully covering (alpha at its max; a mask value 255).
    fn opaque(self) -> bool;
    /// Append its bytes (little endian).
    fn write_le(self, out: &mut Vec<u8>);
    /// From `BYTES` bytes (little endian); `None` if too short.
    fn read_le(b: &[u8]) -> Option<Self>;
    /// As premultiplied floats (a mask value v is a white of alpha v).
    fn to_premul(self) -> PremulF;
    /// From premultiplied floats (alpha 0 gives the transparent default).
    fn from_premul(p: PremulF) -> Self;
    /// `color` (its alpha taken as a coverage too) over `self` with
    /// coverage `cov`, Normal blending: the fill tool's operation. (By
    /// default through premultiplied floats; 8 bit types have their own.)
    fn over(self, color: Self, cov: u8) -> Self {
        if cov == 0 || !color.visible() { return self; }
        Self::from_premul(premul_over(self.to_premul(), color.to_premul(), from_u8(cov)))
    }
    /// The alpha-only split: (color with its alpha at the max, alpha as a
    /// byte). Only meaningful when `ALPHA_FORM`.
    fn split_alpha(self) -> (Self, u8) { (self, 255) }
    /// The inverse of `split_alpha`.
    fn join_alpha(color: Self, _a: u8) -> Self { color }
}

/// `src` premultiplied with coverage `cov` over `dst`, premultiplied.
#[inline(always)]
fn premul_over(dst: PremulF, src: PremulF, cov: f32) -> PremulF {
    let k = src[3] * cov;
    let inv = 1.0 - k;
    let s = if src[3] > 0.0 { cov } else { 0.0 };
    [src[0] * s + dst[0] * inv, src[1] * s + dst[1] * inv, src[2] * s + dst[2] * inv, k + dst[3] * inv]
}

/// Straight RGBA, 8 bit.
pub type Rgba8 = [u8; 4];

impl Pixel for [u8; 4] {
    const KIND: PixelKind = PixelKind::Rgba8;
    const BYTES: usize = 4;
    const ALPHA_FORM: bool = true;
    #[inline(always)]
    fn visible(self) -> bool { self[3] > 0 }
    #[inline(always)]
    fn opaque(self) -> bool { self[3] == 255 }
    fn write_le(self, out: &mut Vec<u8>) { out.extend_from_slice(&self); }
    fn read_le(b: &[u8]) -> Option<Self> { b.get(..4)?.try_into().ok() }
    #[inline]
    fn to_premul(self) -> PremulF { crate::blend::to_premul(self) }
    #[inline]
    fn from_premul(p: PremulF) -> Self { crate::blend::to_straight_u8(p) }
    #[inline]
    fn over(self, color: Self, cov: u8) -> Self {
        let a = div255(color[3] as u32 * cov as u32) as u8;
        crate::blend::paint_pixel_u8(self, [color[0], color[1], color[2]], a, crate::BlendMode::Normal)
    }
    #[inline(always)]
    fn split_alpha(self) -> (Self, u8) { ([self[0], self[1], self[2], 255], self[3]) }
    #[inline(always)]
    fn join_alpha(c: Self, a: u8) -> Self { [c[0], c[1], c[2], a] }
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
    fn to_premul(self) -> PremulF { let v = from_u8(self); [v, v, v, v] }
    fn from_premul(p: PremulF) -> Self { to_u8(p[3]) }
    /// A mask filled: `v + (color − v) × cov`, never lower than before
    /// for a higher color (selection add).
    fn over(self, color: Self, cov: u8) -> Self {
        let (v, c, k) = (self as u32, color as u32, cov as u32);
        div255(v * (255 - k) + c * k) as u8
    }
}

/// Straight RGBA, 16 bit integer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba16(pub [u16; 4]);

impl Pixel for Rgba16 {
    const KIND: PixelKind = PixelKind::Rgba16;
    const BYTES: usize = 8;
    fn visible(self) -> bool { self.0[3] > 0 }
    fn opaque(self) -> bool { self.0[3] == u16::MAX }
    fn write_le(self, out: &mut Vec<u8>) { for c in self.0 { out.extend_from_slice(&c.to_le_bytes()); } }
    fn read_le(b: &[u8]) -> Option<Self> {
        let c = |i: usize| -> Option<u16> { Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?)) };
        Some(Rgba16([c(0)?, c(2)?, c(4)?, c(6)?]))
    }
    fn to_premul(self) -> PremulF {
        let k = 1.0 / 65535.0;
        let a = self.0[3] as f32 * k;
        [self.0[0] as f32 * k * a, self.0[1] as f32 * k * a, self.0[2] as f32 * k * a, a]
    }
    fn from_premul(p: PremulF) -> Self {
        let a = p[3].clamp(0.0, 1.0);
        if a <= 0.0 { return Rgba16([u16::MAX, u16::MAX, u16::MAX, 0]); }
        let q = |v: f32| ((v / a).clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
        Rgba16([q(p[0]), q(p[1]), q(p[2]), (a * 65535.0 + 0.5) as u16])
    }
}

/// IEEE half float from f32 (round to nearest even, overflow to infinity).
pub fn f16_from_f32(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff { return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 }; }
    let e = exp - 127 + 15;
    if e >= 0x1f { return sign | 0x7c00; }
    if e <= 0 {
        if e < -10 { return sign; }
        let m = (man | 0x80_0000) >> (1 - e);
        let round = (m >> 12) & 1 != 0 && (m & 0x17ff) != 0;
        return sign | ((m >> 13) as u16 + round as u16);
    }
    let h = sign | ((e as u16) << 10) | (man >> 13) as u16;
    let rest = man & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 != 0) { h + 1 } else { h }
}

/// f32 from an IEEE half float.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalize.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 { m <<= 1; e -= 1; }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// Straight RGBA, 16 bit float (the bits of each half float).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RgbaF16(pub [u16; 4]);

impl RgbaF16 {
    pub fn from_f32(c: [f32; 4]) -> Self { RgbaF16(c.map(f16_from_f32)) }
    pub fn to_f32(self) -> [f32; 4] { self.0.map(f16_to_f32) }
}

impl Pixel for RgbaF16 {
    const KIND: PixelKind = PixelKind::RgbaF16;
    const BYTES: usize = 8;
    fn visible(self) -> bool { f16_to_f32(self.0[3]) > 0.0 }
    fn opaque(self) -> bool { f16_to_f32(self.0[3]) >= 1.0 }
    fn write_le(self, out: &mut Vec<u8>) { for c in self.0 { out.extend_from_slice(&c.to_le_bytes()); } }
    fn read_le(b: &[u8]) -> Option<Self> { Rgba16::read_le(b).map(|p| RgbaF16(p.0)) }
    fn to_premul(self) -> PremulF { let c = self.to_f32(); [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]] }
    fn from_premul(p: PremulF) -> Self {
        if p[3] <= 0.0 { return RgbaF16::from_f32([1.0, 1.0, 1.0, 0.0]); }
        RgbaF16::from_f32([p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]])
    }
}

/// Straight RGBA, 32 bit float. Equality and hashing go by the bits.
#[derive(Clone, Copy, Debug, Default)]
pub struct RgbaF32(pub [f32; 4]);

impl PartialEq for RgbaF32 {
    fn eq(&self, o: &Self) -> bool { self.0.map(f32::to_bits) == o.0.map(f32::to_bits) }
}
impl Eq for RgbaF32 {}
impl std::hash::Hash for RgbaF32 {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) { self.0.map(f32::to_bits).hash(h) }
}

impl Pixel for RgbaF32 {
    const KIND: PixelKind = PixelKind::RgbaF32;
    const BYTES: usize = 16;
    fn visible(self) -> bool { self.0[3] > 0.0 }
    fn opaque(self) -> bool { self.0[3] >= 1.0 }
    fn write_le(self, out: &mut Vec<u8>) { for c in self.0 { out.extend_from_slice(&c.to_le_bytes()); } }
    fn read_le(b: &[u8]) -> Option<Self> {
        let c = |i: usize| -> Option<f32> { Some(f32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
        Some(RgbaF32([c(0)?, c(4)?, c(8)?, c(12)?]))
    }
    fn to_premul(self) -> PremulF { let c = self.0; [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]] }
    fn from_premul(p: PremulF) -> Self {
        if p[3] <= 0.0 { return RgbaF32([1.0, 1.0, 1.0, 0.0]); }
        RgbaF32([p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3]])
    }
}

/// A pixel known to be premultiplied: the type keeps the two alphas apart
/// (a premultiplied value never goes where a straight one is expected).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Premul<P>(pub P);

/// A pixel known to be straight (storage).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Straight<P>(pub P);

impl Straight<Rgba8> {
    #[inline]
    pub fn premultiplied(self) -> Premul<Rgba8> { Premul(crate::blend::premultiply_u8(self.0)) }
}

impl Premul<Rgba8> {
    #[inline]
    pub fn straight(self) -> Straight<Rgba8> { Straight(crate::blend::unpremultiply_u8(self.0)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_trip() {
        for v in [0.0f32, 1.0, -2.5, 0.5, 65504.0, 6.1e-5, 1e-7, 0.333_333] {
            let back = f16_to_f32(f16_from_f32(v));
            let tol = v.abs() * 1e-3 + 6e-8;
            assert!((back - v).abs() <= tol, "{v} -> {back}");
        }
        assert!(f16_to_f32(f16_from_f32(1e6)).is_infinite());
        for h in 0..0x7c00u16 { assert_eq!(f16_from_f32(f16_to_f32(h)), h, "bits {h:#x}"); }
    }

    fn check<P: Pixel>(samples: &[P]) {
        for p in samples {
            let mut b = Vec::new();
            p.write_le(&mut b);
            assert_eq!(b.len(), P::BYTES);
            assert_eq!(P::read_le(&b), Some(*p));
            // Through premultiplied and back: visible pixels survive.
            if p.opaque() { assert_eq!(P::from_premul(p.to_premul()), *p, "{p:?}"); }
            // Over with coverage 0 changes nothing; full coverage of an
            // opaque color gives it.
            assert_eq!(p.over(*p, 0), *p);
        }
    }

    #[test]
    fn every_pixel_type_round_trips() {
        check::<Rgba8>(&[[1, 2, 3, 255], [0, 0, 0, 0], [200, 100, 50, 128]]);
        check::<Gray8>(&[0, 77, 255]);
        check(&[Rgba16([1, 2, 3, u16::MAX]), Rgba16([0, 0, 0, 0])]);
        check(&[RgbaF16::from_f32([0.25, 0.5, 1.0, 1.0])]);
        check(&[RgbaF32([0.1, 0.2, 0.3, 1.0]), RgbaF32([2.0, 0.0, 0.0, 1.0])]);
        let red = [255u8, 0, 0, 255];
        assert_eq!([0u8, 0, 0, 255].over(red, 255), red);
        assert_eq!(10u8.over(250, 255), 250);
        assert_eq!(Rgba8::join_alpha(red.split_alpha().0, 7), [255, 0, 0, 7]);
        assert_eq!(PixelKind::from_code(PixelKind::RgbaF16.code()), Some(PixelKind::RgbaF16));
    }

    #[test]
    fn premul_wrappers() {
        let s = Straight([200u8, 100, 50, 128]);
        let p = s.premultiplied();
        assert_eq!(p.0[3], 128);
        let back = p.straight().0;
        for (b, o) in back.iter().zip(s.0).take(3) { assert!((*b as i32 - o as i32).abs() <= 2); }
    }
}

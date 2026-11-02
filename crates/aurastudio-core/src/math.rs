//! The single 8-bit conversions every crate uses (one rounding, so two
//! paths never differ by 1), and a few small helpers.

/// `x / 255` rounded to nearest, for `x` in `0..=65025` (a product of
/// two bytes): `((x + 128) * 257) >> 16`, identical to `(x + 127) / 255`
/// on that whole range (tested on every value) without a division.
#[inline(always)]
pub const fn div255(x: u32) -> u32 { ((x + 128) * 257) >> 16 }

/// `a × b / 255` rounded: one byte scaled by another (coverage, opacity).
#[inline(always)]
pub const fn mul255(a: u8, b: u8) -> u8 { div255(a as u32 * b as u32) as u8 }

/// 0..1 → 0..255 rounded to nearest; out-of-range values saturate, NaN
/// gives 0. (`+ 0.5` and a saturating cast: `f32::round` is a libm call
/// on x86-64.)
#[inline(always)]
pub fn to_u8(v: f32) -> u8 { (v * 255.0 + 0.5) as u8 }

/// 0..255 → 0..1.
#[inline(always)]
pub fn from_u8(v: u8) -> f32 { v as f32 * (1.0 / 255.0) }

#[inline(always)]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// `3t² - 2t³` of `t` clamped to 0..1.
#[inline(always)]
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A pen position rounded to 1/16 px (whole-pixel positions made stairs
/// in fine brushes; WinTab gives 1/16 at best).
#[inline]
pub fn quantize16(v: f64) -> f64 { (v * 16.0).round() / 16.0 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_is_exact_on_every_product() {
        for x in 0..=65025u32 {
            assert_eq!(div255(x), (x + 127) / 255, "x = {x}");
        }
    }

    #[test]
    fn to_u8_rounds_to_nearest_and_saturates() {
        for v in 0..=255u8 {
            assert_eq!(to_u8(from_u8(v)), v);
        }
        assert_eq!(to_u8(0.5 / 255.0 - 1e-6), 0);
        assert_eq!(to_u8(1.5 / 255.0), 2);
        assert_eq!(to_u8(-3.0), 0);
        assert_eq!(to_u8(7.0), 255);
        assert_eq!(to_u8(f32::NAN), 0);
    }

    #[test]
    fn helpers() {
        assert_eq!(mul255(255, 128), 128);
        assert_eq!(mul255(0, 200), 0);
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(quantize16(1.03), 1.0);
        assert_eq!(quantize16(1.04), 1.0625);
    }
}

//! Blend modes (W3C Compositing and Blending, on sRGB values like
//! Photoshop, Krita, CSP), for compositing and brushes.

use crate::math::{div255, from_u8, to_u8};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Add,
    Subtract,
}

impl BlendMode {
    pub const ALL: [BlendMode; 13] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Add,
        BlendMode::Subtract,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal     => "Normal",
            BlendMode::Multiply   => "Multiply",
            BlendMode::Screen     => "Screen",
            BlendMode::Overlay    => "Overlay",
            BlendMode::Darken     => "Darken",
            BlendMode::Lighten    => "Lighten",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::ColorBurn  => "Color Burn",
            BlendMode::HardLight  => "Hard Light",
            BlendMode::SoftLight  => "Soft Light",
            BlendMode::Difference => "Difference",
            BlendMode::Add        => "Add",
            BlendMode::Subtract   => "Subtract",
        }
    }

    /// B(cb, cs) for one channel, straight values in 0..1.
    #[inline]
    pub fn channel(self, b: f32, s: f32) -> f32 {
        match self {
            BlendMode::Normal     => s,
            BlendMode::Multiply   => b * s,
            BlendMode::Screen     => b + s - b * s,
            BlendMode::Overlay    => hard_light(s, b),
            BlendMode::Darken     => b.min(s),
            BlendMode::Lighten    => b.max(s),
            BlendMode::ColorDodge => {
                if b <= 0.0 { 0.0 } else if s >= 1.0 { 1.0 } else { (b / (1.0 - s)).min(1.0) }
            }
            BlendMode::ColorBurn  => {
                if b >= 1.0 { 1.0 } else if s <= 0.0 { 0.0 } else { 1.0 - ((1.0 - b) / s).min(1.0) }
            }
            BlendMode::HardLight  => hard_light(b, s),
            BlendMode::SoftLight  => {
                if s <= 0.5 {
                    b - (1.0 - 2.0 * s) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                    b + (2.0 * s - 1.0) * (d - b)
                }
            }
            BlendMode::Difference => (b - s).abs(),
            BlendMode::Add        => (b + s).min(1.0),
            BlendMode::Subtract   => (b - s).max(0.0),
        }
    }
}

#[inline]
fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 { 2.0 * b * s } else { let t = 2.0 * s - 1.0; b + t - b * t }
}

/// Premultiplied RGBA, 0..1.
pub type Premul = [f32; 4];

/// A straight color `cs` at alpha `a_s` over a premultiplied backdrop.
/// Normal is inlined, the other modes apart (all inline, LLVM stopped
/// inlining it: every loop 2.5-3× slower).
#[inline(always)]
pub fn blend_over(dst: &mut Premul, cs: [f32; 3], a_s: f32, mode: BlendMode) {
    if a_s <= 0.0 { return; }
    let a_b = dst[3];
    if mode == BlendMode::Normal || a_b <= 0.0 {
        for c in 0..3 {
            dst[c] = cs[c] * a_s + dst[c] * (1.0 - a_s);
        }
    } else {
        blend_over_mode(dst, cs, a_s, a_b, mode);
    }
    dst[3] = a_s + a_b * (1.0 - a_s);
}

/// `color` at coverages `cov` (0..1 each) over a run of premultiplied
/// pixels: the mode is chosen once for the run (Normal inline, the others
/// through one call each), not per pixel.
pub fn blend_run(dst: &mut [Premul], color: [f32; 3], cov: &[f32], mode: BlendMode) {
    let n = dst.len().min(cov.len());
    if mode == BlendMode::Normal {
        for i in 0..n {
            let a = cov[i];
            if a <= 0.0 { continue; }
            let d = &mut dst[i];
            for c in 0..3 { d[c] = color[c] * a + d[c] * (1.0 - a); }
            d[3] = a + d[3] * (1.0 - a);
        }
    } else {
        for i in 0..n { blend_over(&mut dst[i], color, cov[i], mode); }
    }
}

/// Color part of blend_over for the non-Normal modes (alpha set by caller).
#[inline(never)]
fn blend_over_mode(dst: &mut Premul, cs: [f32; 3], a_s: f32, a_b: f32, mode: BlendMode) {
    for c in 0..3 {
        let cb = dst[c] / a_b;
        dst[c] = cs[c] * a_s * (1.0 - a_b)
            + dst[c] * (1.0 - a_s)
            + a_s * a_b * mode.channel(cb, cs[c]).clamp(0.0, 1.0);
    }
}

/// Same for 8-bit straight-alpha pixels: the brush engines paint the
/// brush color with coverage `a_s` (opacity × pressure × edge) onto the
/// layer as it was before the stroke.
#[inline(always)]
pub fn blend_pixel_u8(backdrop: [u8; 4], color: [u8; 3], a_s: f32, mode: BlendMode) -> [u8; 4] {
    let mut p = to_premul(backdrop);
    let cs = [from_u8(color[0]), from_u8(color[1]), from_u8(color[2])];
    blend_over(&mut p, cs, a_s.clamp(0.0, 1.0), mode);
    to_straight_u8(p)
}

/// `color` at coverage `cov` over a straight pixel (blend_pixel_u8, with
/// integer fast paths for Normal over empty or opaque pixels).
#[inline(always)]
pub fn paint_pixel_u8(backdrop: [u8; 4], color: [u8; 3], cov: u8, mode: BlendMode) -> [u8; 4] {
    if mode == BlendMode::Normal {
        let ab = backdrop[3];
        if ab == 0 {
            return [color[0], color[1], color[2], cov];
        }
        if ab == 255 {
            let a = cov as u32;
            let ia = 255 - a;
            let mix = |c: u8, b: u8| div255(c as u32 * a + b as u32 * ia) as u8;
            return [mix(color[0], backdrop[0]), mix(color[1], backdrop[1]), mix(color[2], backdrop[2]), 255];
        }
    }
    blend_pixel_u8(backdrop, color, from_u8(cov), mode)
}

/// A straight 8-bit pixel as premultiplied 0..1.
#[inline(always)]
pub fn to_premul(p: [u8; 4]) -> Premul {
    let a = from_u8(p[3]);
    [from_u8(p[0]) * a, from_u8(p[1]) * a, from_u8(p[2]) * a, a]
}

#[inline(always)]
pub fn to_straight_u8(p: Premul) -> [u8; 4] {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return [255, 255, 255, 0];
    }
    let inv = 1.0 / a;
    let q = |v: f32| to_u8(v * inv);
    [q(p[0]), q(p[1]), q(p[2]), to_u8(a)]
}

/// Straight RGBA8 → premultiplied RGBA8 (`c × a / 255`, the single
/// rounding): what the GPU receives.
#[inline(always)]
pub fn premultiply_u8(p: [u8; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    [div255(p[0] as u32 * a) as u8, div255(p[1] as u32 * a) as u8, div255(p[2] as u32 * a) as u8, p[3]]
}

/// Premultiplied RGBA8 → straight RGBA8 (`c × 255 / a` rounded, clamped
/// to 255); alpha 0 gives transparent white, as `to_straight_u8`.
#[inline(always)]
pub fn unpremultiply_u8(p: [u8; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 { return [255, 255, 255, 0]; }
    let q = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
    [q(p[0]), q(p[1]), q(p[2]), p[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn normal_is_plain_over() {
        assert_eq!(blend_pixel_u8(W, [0, 0, 0], 0.2, BlendMode::Normal), [204, 204, 204, 255]);
    }

    #[test]
    fn multiply_on_white_is_the_color_and_screen_on_black_too() {
        assert_eq!(blend_pixel_u8(W, [200, 100, 50], 1.0, BlendMode::Multiply), [200, 100, 50, 255]);
        let black = [0, 0, 0, 255];
        assert_eq!(blend_pixel_u8(black, [200, 100, 50], 1.0, BlendMode::Screen), [200, 100, 50, 255]);
    }

    #[test]
    fn modes_paint_normally_on_transparent_backdrop() {
        let clear = [255, 255, 255, 0];
        for mode in BlendMode::ALL {
            assert_eq!(blend_pixel_u8(clear, [10, 20, 30], 1.0, mode), [10, 20, 30, 255], "{mode:?}");
        }
    }

    #[test]
    fn integer_paint_matches_float_blend() {
        // Fast paths must give the same bytes as the general path (±1).
        for backdrop in [[255, 255, 255, 0], [12, 200, 90, 255], [255, 255, 255, 255], [40, 50, 60, 128]] {
            for cov in [0u8, 1, 51, 77, 128, 200, 254, 255] {
                let fast = paint_pixel_u8(backdrop, [10, 120, 250], cov, BlendMode::Normal);
                let slow = blend_pixel_u8(backdrop, [10, 120, 250], cov as f32 / 255.0, BlendMode::Normal);
                for k in 0..4 {
                    // On an empty pixel the color of a fully transparent
                    // result doesn't matter.
                    if slow[3] == 0 && k < 3 { continue; }
                    assert!((fast[k] as i32 - slow[k] as i32).abs() <= 1, "{backdrop:?} cov {cov}: {fast:?} vs {slow:?}");
                }
            }
        }
    }

    #[test]
    fn premultiplied_round_trip() {
        // Opaque pixels survive exactly; others within the precision their
        // alpha leaves (a / 255 steps per channel).
        for a in [0u8, 1, 17, 128, 254, 255] {
            for c in [0u8, 1, 77, 128, 200, 255] {
                let p = [c, 255 - c, c / 2, a];
                let back = unpremultiply_u8(premultiply_u8(p));
                if a == 0 { assert_eq!(back, [255, 255, 255, 0]); continue; }
                let tol = (255.0 / a as f32).ceil() as i32;
                for k in 0..3 { assert!((back[k] as i32 - p[k] as i32).abs() <= tol, "{p:?} -> {back:?}"); }
                if a == 255 { assert_eq!(back, p); }
            }
        }
        assert_eq!(to_straight_u8(to_premul([10, 20, 30, 255])), [10, 20, 30, 255]);
    }

    #[test]
    fn known_channel_values() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(BlendMode::Overlay.channel(0.25, 0.5), 0.25));
        assert!(near(BlendMode::Difference.channel(0.2, 0.7), 0.5));
        assert!(near(BlendMode::Add.channel(0.7, 0.7), 1.0));
        assert!(near(BlendMode::Subtract.channel(0.2, 0.7), 0.0));
        assert!(near(BlendMode::ColorDodge.channel(0.5, 0.5), 1.0));
        assert!(near(BlendMode::ColorBurn.channel(0.5, 0.5), 0.0));
        assert!(near(BlendMode::SoftLight.channel(0.5, 0.5), 0.5));
    }
}

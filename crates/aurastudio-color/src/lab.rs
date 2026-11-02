//! A hub for measuring colors whatever the profile: colors go to ACES
//! 2065-1 linear (its primaries hold every visible color) through the ICC
//! machinery, then to XYZ and CIE Lab by plain matrices. Lightness for the
//! grayscale view, a / b for the Lab channel views, ΔE for the gamut
//! warning and the table's accuracy.

use moxcms::ColorProfile;

/// CIE L*a*b* (L 0..100), white = the hub's white.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lab { pub l: f32, pub a: f32, pub b: f32 }

/// The hub profile (linear, white D60).
pub(crate) fn hub() -> &'static ColorProfile {
    static P: std::sync::OnceLock<ColorProfile> = std::sync::OnceLock::new();
    P.get_or_init(ColorProfile::new_aces_aces_2065_1_linear)
}

/// ACES AP0 → XYZ (SMPTE ST 2065-1).
const AP0_TO_XYZ: [[f32; 3]; 3] = [
    [0.952_552_4, 0.0, 0.000_093_678_6],
    [0.343_966_45, 0.728_166_1, -0.072_132_55],
    [0.0, 0.0, 1.008_825_2],
];

/// The hub's white in XYZ (rows of the matrix summed).
fn white() -> [f32; 3] { AP0_TO_XYZ.map(|r| r[0] + r[1] + r[2]) }

/// Relative luminance of a hub color (0..1 for white).
#[inline]
pub(crate) fn luminance(c: [f32; 3]) -> f32 { let r = AP0_TO_XYZ[1]; r[0] * c[0] + r[1] * c[1] + r[2] * c[2] }

fn f(t: f32) -> f32 { const D: f32 = 6.0 / 29.0; if t > D * D * D { t.cbrt() } else { t / (3.0 * D * D) + 4.0 / 29.0 } }

impl Lab {
    /// From a hub color (linear AP0).
    pub(crate) fn of_hub(c: [f32; 3]) -> Lab {
        let w = white();
        let xyz: [f32; 3] = std::array::from_fn(|k| (AP0_TO_XYZ[k][0] * c[0] + AP0_TO_XYZ[k][1] * c[1] + AP0_TO_XYZ[k][2] * c[2]) / w[k]);
        let (fx, fy, fz) = (f(xyz[0]), f(xyz[1]), f(xyz[2]));
        Lab { l: 116.0 * fy - 16.0, a: 500.0 * (fx - fy), b: 200.0 * (fy - fz) }
    }
}

/// CIE76 distance (about 2.3 is "just noticeable").
pub fn delta_e(p: Lab, q: Lab) -> f32 { ((p.l - q.l).powi(2) + (p.a - q.a).powi(2) + (p.b - q.b).powi(2)).sqrt() }

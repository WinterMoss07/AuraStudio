//! Display color management: the canvas through the monitor's ICC profile
//! (moxcms, as a 3D table on the GPU); the UI isn't converted.

mod monitor;

use moxcms::{ColorProfile, Layout, RenderingIntent, TransformOptions};

pub use monitor::{monitor_profile, DisplayProfile};

/// Grid points per axis of the lookup table. 33 is the usual size for
/// display LUTs (DisplayCAL, OCIO): smooth with trilinear interpolation.
pub const LUT_SIZE: usize = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Intent {
    /// Fits the whole source gamut into the monitor's, keeping relations
    /// between colors. Only differs from relative colorimetric for LUT
    /// based monitor profiles; matrix profiles have no perceptual table.
    #[default]
    Perceptual,
    /// Exact colors where the monitor can show them, white point matched.
    RelativeColorimetric,
}

impl Intent {
    pub fn name(self) -> &'static str {
        match self {
            Intent::Perceptual => "Perceptual",
            Intent::RelativeColorimetric => "Relative colorimetric",
        }
    }

    fn to_moxcms(self) -> RenderingIntent {
        match self {
            Intent::Perceptual => RenderingIntent::Perceptual,
            Intent::RelativeColorimetric => RenderingIntent::RelativeColorimetric,
        }
    }
}

/// sRGB -> monitor transform, as a lookup table.
pub struct DisplayTransform {
    /// RGBA (A = 1), red varying fastest, then green, then blue: the layout
    /// of a 3D texture of LUT_SIZE^3 texels.
    lut: Vec<[f32; 4]>,
    /// The monitor profile is a plain matrix/curves one (most are).
    pub matrix_shaper: bool,
}

impl DisplayTransform {
    /// Build the transform from the monitor's ICC profile bytes.
    pub fn new(monitor_icc: &[u8], intent: Intent) -> Result<Self, String> {
        let display = ColorProfile::new_from_slice(monitor_icc).map_err(|e| format!("bad ICC profile: {e:?}"))?;
        Self::from_profile(&display, intent)
    }

    fn from_profile(display: &ColorProfile, intent: Intent) -> Result<Self, String> {
        let srgb = ColorProfile::new_srgb();
        let options = TransformOptions { rendering_intent: intent.to_moxcms(), ..Default::default() };
        let transform = srgb
            .create_transform_f32(Layout::Rgb, display, Layout::Rgb, options)
            .map_err(|e| format!("no transform to this profile: {e:?}"))?;

        let n = LUT_SIZE;
        let step = 1.0 / (n - 1) as f32;
        let mut src = Vec::with_capacity(n * n * n * 3);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    src.extend_from_slice(&[r as f32 * step, g as f32 * step, b as f32 * step]);
                }
            }
        }
        let mut dst = vec![0.0f32; src.len()];
        transform.transform(&src, &mut dst).map_err(|e| format!("transform failed: {e:?}"))?;

        let lut = dst.chunks_exact(3)
            .map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), 1.0])
            .collect();
        Ok(Self { lut, matrix_shaper: display.is_matrix_shaper() })
    }

    /// The table for the GPU: LUT_SIZE^3 RGBA texels.
    pub fn lut(&self) -> &[[f32; 4]] {
        &self.lut
    }

    /// True when the monitor profile is (close to) sRGB: nothing to do.
    pub fn is_identity(&self) -> bool {
        let n = LUT_SIZE;
        let step = 1.0 / (n - 1) as f32;
        self.lut.iter().enumerate().all(|(i, c)| {
            let (r, g, b) = (i % n, (i / n) % n, i / (n * n));
            let want = [r as f32 * step, g as f32 * step, b as f32 * step];
            (0..3).all(|k| (c[k] - want[k]).abs() < 0.5 / 255.0)
        })
    }

    /// Convert one sRGB color for display (trilinear lookup, like the GPU).
    /// For UI color swatches and gradients.
    pub fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        let n = LUT_SIZE;
        let scale = (n - 1) as f32;
        let pos = rgb.map(|v| v as f32 / 255.0 * scale);
        let i0 = pos.map(|p| (p as usize).min(n - 2));
        let f = [pos[0] - i0[0] as f32, pos[1] - i0[1] as f32, pos[2] - i0[2] as f32];
        let at = |r: usize, g: usize, b: usize| self.lut[r + g * n + b * n * n];
        let mut out = [0.0f32; 3];
        for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
            for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
                for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
                    let c = at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                    let w = wr * wg * wb;
                    for k in 0..3 { out[k] += c[k] * w; }
                }
            }
        }
        out.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_monitor_is_identity() {
        let srgb = ColorProfile::new_srgb().encode().unwrap();
        let t = DisplayTransform::new(&srgb, Intent::Perceptual).unwrap();
        assert!(t.is_identity());
        for c in [[0, 0, 0], [128, 128, 128], [200, 30, 90], [255, 255, 255]] {
            let o = t.apply(c);
            for k in 0..3 { assert!((o[k] as i32 - c[k] as i32).abs() <= 1, "{c:?} -> {o:?}"); }
        }
    }

    /// A wide-gamut (Display P3) monitor: sRGB red must be sent as a less
    /// saturated P3 red, but grays must stay exactly neutral.
    #[test]
    fn wide_gamut_monitor_keeps_grays_neutral() {
        let p3 = ColorProfile::new_display_p3().encode().unwrap();
        for intent in [Intent::Perceptual, Intent::RelativeColorimetric] {
            let t = DisplayTransform::new(&p3, intent).unwrap();
            assert!(!t.is_identity());
            let red = t.apply([255, 0, 0]);
            assert!(red[0] > 220 && red[1] > 30 && red[2] > 20, "{red:?}");
            for v in [32u8, 64, 128, 192, 240] {
                let g = t.apply([v, v, v]);
                assert!((g[0] as i32 - g[1] as i32).abs() <= 1 && (g[1] as i32 - g[2] as i32).abs() <= 1,
                    "gray {v} -> {g:?}");
            }
        }
    }

    #[test]
    fn garbage_profile_is_an_error() {
        assert!(DisplayTransform::new(b"not an icc profile", Intent::Perceptual).is_err());
    }
}

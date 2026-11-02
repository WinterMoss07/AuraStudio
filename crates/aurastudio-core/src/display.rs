//! The display lookup table: made by `aurastudio-color`, applied by
//! `aurastudio-gpu`; neither knows the other.

/// A 3D table of `size`³ RGBA texels (A = 1), red varying fastest, then
/// green, then blue: the layout of a 3D texture. Sampled trilinearly with
/// the document's encoded values.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayLut {
    pub size:   usize,
    pub texels: Vec<[f32; 4]>,
}

impl DisplayLut {
    /// The table that changes nothing.
    pub fn identity(size: usize) -> Self {
        let n = size.max(2);
        let k = 1.0 / (n - 1) as f32;
        let mut texels = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n { texels.push([r as f32 * k, g as f32 * k, b as f32 * k, 1.0]); }
            }
        }
        Self { size: n, texels }
    }

    /// Changes nothing (within half an 8-bit level): the GPU can skip it.
    pub fn is_identity(&self) -> bool {
        let n = self.size;
        let k = 1.0 / (n - 1) as f32;
        self.texels.iter().enumerate().all(|(i, c)| {
            let want = [(i % n) as f32 * k, ((i / n) % n) as f32 * k, (i / (n * n)) as f32 * k];
            (0..3).all(|j| (c[j] - want[j]).abs() < 0.5 / 255.0)
        })
    }

    /// An 8-bit color through the table (UI swatches, gradients).
    pub fn apply_u8(&self, rgb: [u8; 3]) -> [u8; 3] { self.apply(rgb.map(crate::math::from_u8)).map(|v| crate::math::to_u8(v.clamp(0.0, 1.0))) }

    /// One color through the table (swatches, tests), 0..1 in and out.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let at = |v: f32| { let x = v.clamp(0.0, 1.0) * s; let i = (x as usize).min(n - 2); (i, x - i as f32) };
        let ((r0, fr), (g0, fg), (b0, fb)) = (at(rgb[0]), at(rgb[1]), at(rgb[2]));
        let t = |r: usize, g: usize, b: usize| self.texels[(b * n + g) * n + r];
        let mut out = [0.0f32; 3];
        for (db, wb) in [(0, 1.0 - fb), (1, fb)] {
            for (dg, wg) in [(0, 1.0 - fg), (1, fg)] {
                for (dr, wr) in [(0, 1.0 - fr), (1, fr)] {
                    let c = t(r0 + dr, g0 + dg, b0 + db);
                    let w = wr * wg * wb;
                    for k in 0..3 { out[k] += c[k] * w; }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_changes_nothing() {
        let l = DisplayLut::identity(33);
        assert_eq!(l.texels.len(), 33 * 33 * 33);
        for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.2, 0.71, 0.333]] {
            let o = l.apply(c);
            for k in 0..3 { assert!((o[k] - c[k]).abs() < 1e-5); }
        }
        assert!(l.is_identity());
        assert_eq!(l.apply_u8([200, 30, 90]), [200, 30, 90]);
    }
}

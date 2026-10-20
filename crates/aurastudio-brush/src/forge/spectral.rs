// libmypaint's constants, digits as written there.
#![allow(clippy::excessive_precision)]

//! libmypaint's pigment mixing: colors as 10 spectral channels mixed by a
//! weighted geometric mean (blue + yellow give green). Tables from
//! libmypaint 1.6.1's helpers.c.

const WGM_EPSILON: f32 = 0.001;

/// libmypaint's fastlog2 (fastapprox).
#[inline]
fn fastlog2(x: f32) -> f32 {
    let vx = x.to_bits();
    let mx = f32::from_bits((vx & 0x007F_FFFF) | 0x3F00_0000);
    let y = vx as f32 * 1.192_092_9e-7;
    y - 124.225_52 - 1.498_030_3 * mx - 1.725_88 / (0.352_088_7 + mx)
}

/// Approximate `2^p` matching libmypaint's `fastpow2`.
#[inline]
fn fastpow2(p: f32) -> f32 {
    let offset = if p < 0.0 { 1.0 } else { 0.0 };
    let clipp = if p < -126.0 { -126.0 } else { p };
    let w = clipp as i32;
    let z = clipp - w as f32 + offset;
    let bits = ((1 << 23) as f32
        * (clipp + 121.274_055 + 27.728_022 / (4.842_525_7 - z) - 1.490_129_1 * z))
        as u32;
    f32::from_bits(bits)
}

/// libmypaint's fastpow (~2 % error, same as libmypaint's).
#[inline]
pub fn fastpow(x: f32, p: f32) -> f32 {
    fastpow2(p * fastlog2(x))
}

/// `fastlog2` of each spectral channel. The brush color's logs are the
/// same for every pixel of a dab: computed once (AuraStudio patch).
pub fn fastlog2_all(spec: &[f32; 10]) -> [f32; 10] {
    let mut out = [0.0_f32; 10];
    for i in 0..10 {
        out[i] = fastlog2(spec[i]);
    }
    out
}

/// fastpow2, branch-free (the 10 channels vectorize); the same results
/// for p ≤ 127.
#[inline(always)]
fn fastpow2_wgm(p: f32) -> f32 {
    let offset = if p < 0.0 { 1.0 } else { 0.0 };
    let clipp = p.clamp(-126.0, 127.0);
    // SAFETY: clipp is finite, in [-126, 127].
    let w = unsafe { clipp.to_int_unchecked::<i32>() };
    let z = clipp - w as f32 + offset;
    let v = (1 << 23) as f32 * (clipp + 121.274_055 + 27.728_022 / (4.842_525_7 - z) - 1.490_129_1 * z);
    // SAFETY: z is in [0, 1], so v is in (0, 2^31) for clipp in range.
    f32::from_bits(unsafe { v.to_int_unchecked::<u32>() })
}

/// libmypaint's per-pixel WGM: `fastpow(a[i], fac_a) * fastpow(b[i],
/// fac_b)` for the 10 channels, with `log_a` = `fastlog2_all(a)` (same
/// result, fewer logs, vectorized; AuraStudio patch).
#[inline]
pub fn wgm_mix(log_a: &[f32; 10], fac_a: f32, b: &[f32; 10], fac_b: f32) -> [f32; 10] {
    let mut mix = [0.0_f32; 10];
    for i in 0..10 {
        mix[i] = fastpow2_wgm(fac_a * log_a[i]) * fastpow2_wgm(fac_b * fastlog2(b[i]));
    }
    mix
}

const SPECTRAL_R: [f32; 10] = [
    0.009281362787953,
    0.009732627042016,
    0.011254252737167,
    0.015105578649573,
    0.024797924177217,
    0.083622585502406,
    0.977865045723212,
    1.000000000000000,
    0.999961046144372,
    0.999999992756822,
];

const SPECTRAL_G: [f32; 10] = [
    0.002854127435775,
    0.003917589679914,
    0.012132151699187,
    0.748259205918013,
    1.000000000000000,
    0.865695937531795,
    0.037477469241101,
    0.022816789725717,
    0.021747419446456,
    0.021384940572308,
];

const SPECTRAL_B: [f32; 10] = [
    0.537052150373386,
    0.546646402401469,
    0.575501819073983,
    0.258778829633924,
    0.041709923751716,
    0.012662638828324,
    0.007485593127390,
    0.006766900622462,
    0.006699764779016,
    0.006676219883241,
];

const T_MATRIX: [[f32; 10]; 3] = [
    [
        0.026595621243689,
        0.049779426257903,
        0.022449850859496,
        -0.218453689278271,
        -0.256894883201278,
        0.445881722194840,
        0.772365886289756,
        0.194498761382537,
        0.014038157587820,
        0.007687264480513,
    ],
    [
        -0.032601672674412,
        -0.061021043498478,
        -0.052490001018404,
        0.206659098273522,
        0.572496335158169,
        0.317837248815438,
        -0.021216624031211,
        -0.019387668756117,
        -0.001521339050858,
        -0.000835181622534,
    ],
    [
        0.339475473216284,
        0.635401374177222,
        0.771520797089589,
        0.113222640692379,
        -0.055251113343776,
        -0.048222578468680,
        -0.012966666339586,
        -0.001523814504223,
        -0.000094718948810,
        -0.000051604594741,
    ],
];

/// Upsample straight-alpha linear sRGB `(r, g, b)` into 10 spectral
/// channels. `WGM_EPSILON` is mixed in to avoid the all-zero / all-one
/// degenerate cases that `pow(x, fac)` can't handle later.
pub fn rgb_to_spectral(r: f32, g: f32, b: f32) -> [f32; 10] {
    let offset = 1.0 - WGM_EPSILON;
    let r = r * offset + WGM_EPSILON;
    let g = g * offset + WGM_EPSILON;
    let b = b * offset + WGM_EPSILON;
    let mut out = [0.0_f32; 10];
    for i in 0..10 {
        out[i] = SPECTRAL_R[i] * r + SPECTRAL_G[i] * g + SPECTRAL_B[i] * b;
    }
    out
}

/// 10 spectral channels back to RGB, 0..1.
pub fn spectral_to_rgb(spec: &[f32; 10]) -> (f32, f32, f32) {
    let offset = 1.0 - WGM_EPSILON;
    let mut tmp = [0.0_f32; 3];
    for i in 0..10 {
        tmp[0] += T_MATRIX[0][i] * spec[i];
        tmp[1] += T_MATRIX[1][i] * spec[i];
        tmp[2] += T_MATRIX[2][i] * spec[i];
    }
    let out = |v: f32| ((v - WGM_EPSILON) / offset).clamp(0.0, 1.0);
    (out(tmp[0]), out(tmp[1]), out(tmp[2]))
}

/// libmypaint's mix_colors: `a` weighted `fac` with `b`, spectral by
/// `paint_mode`, linear for the rest. Straight RGBA.
pub fn mix_colors(a: [f32; 4], b: [f32; 4], fac: f32, paint_mode: f32) -> [f32; 4] {
    let opa_a = fac;
    let opa_b = 1.0 - opa_a;
    let out_a = (opa_a * a[3] + opa_b * b[3]).clamp(0.0, 1.0);

    let sfac_a = if a[3] == 0.0 {
        0.0
    } else {
        opa_a * a[3] / (a[3] + b[3] * opa_b)
    };
    let sfac_b = 1.0 - sfac_a;

    let mut result = [0.0_f32; 4];
    if paint_mode > 0.0 {
        let spec_a = rgb_to_spectral(a[0], a[1], a[2]);
        let spec_b = rgb_to_spectral(b[0], b[1], b[2]);
        let mut mix = [0.0_f32; 10];
        for i in 0..10 {
            // The real powf, as helpers.c (fastpow's error drifted the smudge colors).
            mix[i] = spec_a[i].powf(sfac_a) * spec_b[i].powf(sfac_b);
        }
        let (r, g, b_) = spectral_to_rgb(&mix);
        result[0] = r;
        result[1] = g;
        result[2] = b_;
    }
    if paint_mode < 1.0 {
        for i in 0..3 {
            let lin = a[i] * opa_a + b[i] * opa_b;
            result[i] = result[i] * paint_mode + (1.0 - paint_mode) * lin;
        }
    }
    result[3] = out_a;
    result
}

/// Additive to spectral blending by the canvas alpha.
pub fn spectral_blend_factor(x: f32) -> f32 {
    const VER_FAC: f32 = 1.65;
    const HOR_FAC: f32 = 8.0;
    const HOR_OFFS: f32 = 3.0;
    let b = x * HOR_FAC - HOR_OFFS;
    (0.5 + b / (1.0 + b.abs() * VER_FAC)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-tripping the primaries should give back something close
    /// to the input. With the WGM offset there's a small contraction.
    #[test]
    fn primaries_round_trip_approximately() {
        for &(r, g, b) in &[(1.0_f32, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0)] {
            let s = rgb_to_spectral(r, g, b);
            let (r2, g2, b2) = spectral_to_rgb(&s);
            assert!((r2 - r).abs() < 0.05, "r: {r} → {r2}");
            assert!((g2 - g).abs() < 0.05, "g: {g} → {g2}");
            assert!((b2 - b).abs() < 0.05, "b: {b} → {b2}");
        }
    }

    /// Blue + yellow ≈ green (a classic pigment-mixing test). With
    /// fac_a = fac_b = 0.5 the WGM blend should look much more green
    /// than the linear-RGB midpoint would.
    #[test]
    fn blue_plus_yellow_is_greenish() {
        let blue = rgb_to_spectral(0.0, 0.0, 1.0);
        let yellow = rgb_to_spectral(1.0, 1.0, 0.0);
        let mut mix = [0.0_f32; 10];
        for i in 0..10 {
            mix[i] = blue[i].powf(0.5) * yellow[i].powf(0.5);
        }
        let (r, g, b) = spectral_to_rgb(&mix);
        // Green channel should dominate, with red and blue both lower.
        assert!(g > r, "green {g} should beat red {r}");
        assert!(g > b, "green {g} should beat blue {b}");
    }

    #[test]
    fn wgm_mix_matches_fastpow() {
        let mut seed = 12345u32;
        let mut rnd = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) as f32 / (1 << 24) as f32 };
        for _ in 0..20000 {
            let a = rgb_to_spectral(rnd(), rnd(), rnd());
            let b = rgb_to_spectral(rnd(), rnd(), rnd());
            let fa = rnd();
            let fb = 1.0 - fa;
            let m = wgm_mix(&fastlog2_all(&a), fa, &b, fb);
            for i in 0..10 {
                let want = fastpow(a[i], fa) * fastpow(b[i], fb);
                assert_eq!(m[i].to_bits(), want.to_bits(), "{i} {fa}");
            }
        }
    }
}

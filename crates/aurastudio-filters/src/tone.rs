//! Tone curves measured on Photoshop: Brightness / Contrast (modern and
//! legacy), Levels with its midtone toe, Posterize, Threshold.
//!
//! Adapted from PhotoCraft (`crates/compose/src/adjust.rs`),
//! Copyright (c) 2026 ArtCraft Team and the PhotoCraft contributors,
//! MIT License (see NOTICE). Changes: 8-bit tables, our parameter layout.

/// Output level of the Brightness curve's knee.
const BRIGHTNESS_KNEE: f32 = 126.0 / 255.0;
const LEGACY_PIVOT: f32 = 128.0 / 255.0;

/// Modern Brightness curve (`brightness` in -150..150): pins black and
/// white; raised, a line of slope 1.375^(b/50) up to a knee at 126/255,
/// then a roll-off to (1, 1); lowered, the inverse of the raised curve.
struct Brightness { s: f32, e: f32, x0: f32, alpha: f32, hermite: bool }

impl Brightness {
    fn new(brightness: f32) -> Self {
        let b = brightness.abs();
        let s = 1.375f32.powf(b / 50.0);
        let e = 1.0 / (1.0 + 0.084 * b);
        let x0 = (BRIGHTNESS_KNEE / s).min(1.0);
        let secant = (1.0 - BRIGHTNESS_KNEE) / (1.0 - x0).max(1e-6);
        let (alpha, beta) = (s / secant, e / secant);
        Self { s, e, x0, alpha, hermite: alpha * alpha + beta * beta <= 9.0 }
    }

    fn raised(&self, x: f32) -> f32 {
        let Self { s, e, x0, alpha, hermite } = *self;
        let y0 = BRIGHTNESS_KNEE;
        if x <= x0 { return y0.min(s * x); }
        let h = (1.0 - x0).max(1e-6);
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let y = if hermite {
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * s + (-2.0 * t3 + 3.0 * t2) + (t3 - t2) * h * e
        } else {
            1.0 - (1.0 - y0) * (1.0 - t).powf(alpha)
        };
        y.clamp(0.0, 1.0)
    }

    fn eval(&self, v: f32, raise: bool) -> f32 {
        let v = v.clamp(0.0, 1.0);
        if raise { return self.raised(v); }
        if v <= 0.0 || v >= 1.0 { return v; }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = 0.5 * (lo + hi);
            if self.raised(mid) < v { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    }
}

/// Modern Contrast (`contrast` in -50..100): a symmetric Hermite S-curve
/// around mid gray; end slopes 1 - c/128, center slope 1 + c/128.
fn modern_contrast(v: f32, contrast: f32) -> f32 {
    if contrast == 0.0 { return v; }
    let k = contrast / 128.0;
    let (end, mid) = (1.0 - k, 1.0 + k);
    let half = |t: f32| end * 0.5 * (t * t * t - 2.0 * t * t + t) + (-2.0 * t * t * t + 3.0 * t * t) * 0.5 + mid * 0.5 * (t * t * t - t * t);
    let v = v.clamp(0.0, 1.0);
    if v <= 0.5 { half(v / 0.5) } else { 1.0 - half((1.0 - v) / 0.5) }
}

/// Legacy (before CS3) Brightness / Contrast: contrast scales around
/// 128, brightness added before it when contrast is raised, after when
/// lowered.
fn legacy(v: f32, brightness: f32, contrast: f32) -> f32 {
    let b = brightness / 255.0;
    let c = contrast.clamp(-100.0, 99.0);
    let k = if c >= 0.0 { 1.0 / (1.0 - c / 100.0) } else { 1.0 + c / 100.0 };
    let (pre, post) = if c >= 0.0 { (b, 0.0) } else { (0.0, b) };
    ((v + pre - LEGACY_PIVOT) * k + LEGACY_PIVOT + post).clamp(0.0, 1.0)
}

pub fn brightness_contrast(brightness: f32, contrast: f32, legacy_mode: bool) -> [u8; 256] {
    let curve = Brightness::new(brightness);
    table(|v| {
        if legacy_mode { return legacy(v, brightness, contrast); }
        let b = if brightness == 0.0 { v } else { curve.eval(v, brightness > 0.0) };
        modern_contrast(b, contrast)
    })
}

/// One Levels channel: input black, gamma, input white, output black,
/// output white (levels 0..255, gamma as is).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsChannel { pub in_black: f32, pub gamma: f32, pub in_white: f32, pub out_black: f32, pub out_white: f32 }

impl LevelsChannel {
    pub const IDENTITY: Self = Self { in_black: 0.0, gamma: 1.0, in_white: 255.0, out_black: 0.0, out_white: 255.0 };

    pub fn from(v: &[f32]) -> Self {
        let g = |i: usize, d: f32| v.get(i).copied().filter(|x| x.is_finite()).unwrap_or(d);
        let in_black = g(0, 0.0).clamp(0.0, 253.0);
        Self {
            in_black,
            gamma: g(1, 1.0).clamp(0.01, 9.99),
            in_white: g(2, 255.0).clamp(in_black + 2.0, 255.0),
            out_black: g(3, 0.0).clamp(0.0, 255.0),
            out_white: g(4, 255.0).clamp(0.0, 255.0),
        }
    }

    /// Photoshop's three whole-level steps: input stretch, midtone gamma,
    /// output range.
    pub fn eval(&self, v: f32) -> f32 {
        let q = 255.0;
        let (ib, iw, ob, ow) = (self.in_black / q, self.in_white / q, self.out_black / q, self.out_white / q);
        let whole = |x: f32| (x * q + 0.5).floor() / q;
        let v = (v * q).round() / q;
        let u = (((v - ib) / (iw - ib).max(1e-6)).clamp(0.0, 1.0) * q + 0.5 + 1e-3).floor() / q;
        let t = whole(levels_gamma(u, self.gamma));
        whole(ob + t * (ow - ob))
    }
}

const LEVELS_TOE_POWER: f32 = 2.2;

/// Levels' midtone gamma: u^(1/g); above 1, Photoshop starts along a toe
/// a·u − b·u^q (slope 2^g at black) that joins the power curve smoothly.
pub fn levels_gamma(u: f32, g: f32) -> f32 {
    let c = 1.0 / g;
    if g <= 1.0 { return u.powf(c); }
    let (a, q) = (2f32.powf(g), LEVELS_TOE_POWER);
    let u1 = ((1.0 - c / q) / (a * (1.0 - 1.0 / q))).powf(1.0 / (1.0 - c));
    if u >= u1 || u1 <= 0.0 { return u.powf(c); }
    let b = (a * u1 - u1.powf(c)) / u1.powf(q);
    a * u - b * u.powf(q)
}

/// Where the gamma handle sits: the input level that comes out mid gray.
pub fn gamma_pos(black: f32, white: f32, gamma: f32) -> f32 { black + (white - black) * 0.5f32.powf(gamma) }

pub fn gamma_from_pos(black: f32, white: f32, x: f32) -> f32 {
    let t = ((x - black) / (white - black).max(1.0)).clamp(0.01, 0.99);
    (t.ln() / 0.5f32.ln()).clamp(0.01, 9.99)
}

/// Black and white input levels clipping 0.1 % at each end (Auto).
pub fn auto_levels(h: &[u32; 256]) -> (f32, f32) {
    let total: u64 = h.iter().map(|&x| x as u64).sum();
    let clip = total / 1000;
    let (mut acc, mut lo) = (0u64, 0usize);
    while lo < 253 && acc + h[lo] as u64 <= clip { acc += h[lo] as u64; lo += 1; }
    let (mut acc, mut hi) = (0u64, 255usize);
    while hi > lo + 2 && acc + h[hi] as u64 <= clip { acc += h[hi] as u64; hi -= 1; }
    (lo as f32, hi as f32)
}

/// Levels: per channel, then the RGB (master) channel.
pub fn levels(master: LevelsChannel, chans: [LevelsChannel; 3]) -> [[u8; 256]; 3] {
    std::array::from_fn(|c| table(|v| master.eval(chans[c].eval(v))))
}

/// Photoshop's Posterize: n equal input bins, output floor(k·255/(n−1)).
pub fn posterize(levels: u32) -> [u8; 256] {
    let n = levels.clamp(2, 255) as f32;
    let mut t = [0u8; 256];
    for (x, o) in t.iter_mut().enumerate() {
        let bin = (x as f32 * n / 256.0).floor().min(n - 1.0);
        *o = (bin * 255.0 / (n - 1.0)).floor() as u8;
    }
    t
}

/// Threshold's split: white when the luminosity 30 R + 59 G + 11 B
/// reaches the level (Photoshop's, not Rec. 601 gray).
pub fn threshold_white(c: [u8; 3], level: f32) -> bool {
    let t = level.round();
    let sum = 30.0 * c[0] as f32 + 59.0 * c[1] as f32 + 11.0 * c[2] as f32;
    sum > 100.0 * t - 50.0
}

/// The luminosity Threshold splits on (0..255).
pub fn luminosity(c: [u8; 3]) -> u8 { ((30 * c[0] as u32 + 59 * c[1] as u32 + 11 * c[2] as u32 + 50) / 100) as u8 }

fn table(f: impl Fn(f32) -> f32) -> [u8; 256] {
    std::array::from_fn(|i| (f(i as f32 / 255.0).clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photoshop_values() {
        // Neutral settings are the identity.
        let id = brightness_contrast(0.0, 0.0, false);
        assert!(id.iter().enumerate().all(|(i, &v)| v as usize == i));
        // Modern brightness pins black and white.
        let b = brightness_contrast(80.0, 0.0, false);
        assert_eq!((b[0], b[255]), (0, 255));
        assert!(b[128] > 160);
        // Legacy: +40 with contrast +30 maps 128 to 185 (measured).
        assert!((brightness_contrast(40.0, 30.0, true)[128] as i32 - 185).abs() <= 1);
        // Posterize 3: 0, 127, 255.
        let p = posterize(3);
        assert_eq!((p[0], p[128], p[255]), (0, 127, 255));
        // Levels 44..214: whole-level staircase.
        let l = LevelsChannel { in_black: 44.0, in_white: 214.0, ..LevelsChannel::IDENTITY };
        assert_eq!((0..4).map(|k| (l.eval((45 + k) as f32 / 255.0) * 255.0).round() as u8).collect::<Vec<_>>(), [2, 3, 5, 6]);
        // Threshold 128 on luminosity.
        assert!(threshold_white([128, 128, 128], 128.0) && !threshold_white([127, 127, 127], 128.0));
    }

    #[test]
    fn gamma_handle_round_trips() {
        for g in [0.5f32, 1.0, 1.8] {
            let x = gamma_pos(10.0, 240.0, g);
            assert!((gamma_from_pos(10.0, 240.0, x) - g).abs() < 0.01);
        }
    }
}

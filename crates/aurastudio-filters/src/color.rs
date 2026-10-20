//! Hue / Saturation (six ranges) and Black & White, measured on Photoshop,
//! adapted from PhotoCraft (MIT, see NOTICE); the W3C sepia matrix.

/// Photoshop's default Black & White weights (%): reds, yellows, greens,
/// cyans, blues, magentas.
pub const BW_DEFAULTS: [f32; 6] = [40.0, 60.0, 40.0, 60.0, 20.0, 80.0];

pub const RANGE_NAMES: [&str; 6] = ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];

/// One of Hue / Saturation's color ranges: its edits (hue °, saturation
/// and lightness %) and the four hue degrees where it fades in, is fully
/// on, starts fading out, ends (may wrap past 0 / 360).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HueRange { pub hue: f32, pub saturation: f32, pub lightness: f32, pub bounds: [f32; 4] }

impl HueRange {
    /// Range `i` (0 reds … 5 magentas), Photoshop's default extent: fully
    /// on 30° around its center, fading 30° each side.
    pub fn neutral(i: usize) -> Self {
        let c = 60.0 * i as f32;
        Self { hue: 0.0, saturation: 0.0, lightness: 0.0, bounds: [c - 45.0, c - 15.0, c + 15.0, c + 45.0] }
    }

    pub fn is_neutral(&self) -> bool { self.hue == 0.0 && self.saturation == 0.0 && self.lightness == 0.0 }

    /// How much the range applies at hue `deg` (0..1).
    pub fn weight(&self, deg: f32) -> f32 {
        let [a, b, c, d] = self.bounds;
        let rel = |x: f32| if x.is_finite() { (x - a).rem_euclid(360.0) } else { 0.0 };
        let b = rel(b);
        let c = rel(c).max(b);
        let d = rel(d).max(c);
        let h = rel(deg);
        if h <= b {
            if b <= 0.0 { 1.0 } else { h / b }
        } else if h <= c {
            1.0
        } else if h <= d {
            if d <= c { 1.0 } else { (d - h) / (d - c) }
        } else {
            0.0
        }
    }

    /// Increasing, within one turn, center in -30..330.
    pub fn canonical_bounds(b: [f32; 4]) -> [f32; 4] {
        if b.iter().any(|v| !v.is_finite()) { return [-45.0, -15.0, 15.0, 45.0]; }
        let mut out = [b[0]; 4];
        for k in 1..4 { out[k] = (out[k - 1] + (b[k] - out[k - 1]).rem_euclid(360.0)).min(out[0] + 359.0); }
        let centre = (out[1] + out[2]) / 2.0;
        let shift = (centre + 30.0).rem_euclid(360.0) - 30.0 - centre;
        out.map(|v| v + shift)
    }

    /// Bounds after dragging handle `k` to `deg` (the outer handles keep
    /// within 180° of the inner ones).
    pub fn move_handle(bounds: [f32; 4], k: usize, deg: f32) -> [f32; 4] {
        let mut b = Self::canonical_bounds(bounds);
        if k >= 4 { return b; }
        if k == 0 {
            let delta = (deg - b[0] + 180.0).rem_euclid(360.0) - 180.0;
            b[0] = (b[0] + delta).min(b[1]).max(b[1] - 180.0);
        } else {
            let rel = (deg - b[0]).rem_euclid(360.0);
            let hi = if k == 3 { b[0] + 359.0 } else { b[k + 1] };
            b[k] = (b[0] + rel).clamp(b[k - 1], hi);
        }
        b
    }
}

/// Hue / Saturation: master hue (°), saturation and lightness (-1..1),
/// colorize, and the ranges (ignored when colorizing).
pub struct HueSat { pub hue: f32, pub saturation: f32, pub lightness: f32, pub colorize: bool, tables: Option<Box<[[f32; 3]; 360]>> }

impl HueSat {
    pub fn new(hue: f32, saturation: f32, lightness: f32, colorize: bool, ranges: &[HueRange; 6]) -> Self {
        let tables = (!colorize && ranges.iter().any(|r| !r.is_neutral())).then(|| {
            Box::new(std::array::from_fn(|k| {
                ranges.iter().filter(|r| !r.is_neutral()).fold([0.0f32; 3], |acc, r| {
                    let w = r.weight(k as f32);
                    [acc[0] + w * r.hue, acc[1] + w * r.saturation / 100.0, acc[2] + w * r.lightness / 100.0]
                })
            }))
        });
        Self { hue, saturation: (saturation / 100.0).clamp(-1.0, 1.0), lightness: (lightness / 100.0).clamp(-1.0, 1.0), colorize, tables }
    }

    pub fn is_identity(&self) -> bool { self.hue == 0.0 && self.saturation == 0.0 && self.lightness == 0.0 && !self.colorize && self.tables.is_none() }

    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let (dh, ds, dl) = match &self.tables {
            Some(t) => {
                // Ranges follow the pixel's original hue; grays stay gray.
                let h0 = rgb_to_hsl(c).0 * 360.0;
                let k = (h0.round() as usize) % 360;
                (t[k][0], t[k][1], t[k][2])
            }
            None => (0.0, 0.0, 0.0),
        };
        let out = hue_saturation(c, self.hue + dh, (self.saturation + ds).clamp(-1.0, 1.0), self.lightness, self.colorize);
        range_lightness(out, dl)
    }
}

/// Lightness first, then the hue turned and the saturation scaled
/// (raising divides by 1 - s: +50 % doubles it). Colorize sets hue and
/// saturation at the lightened lightness.
fn hue_saturation(c: [f32; 3], hue: f32, s: f32, l: f32, colorize: bool) -> [f32; 3] {
    let lighten = |v: f32| if l > 0.0 { v + (1.0 - v) * l } else { v * (1.0 + l) };
    if colorize {
        let (_, _, ll) = rgb_to_hsl(c);
        return hsl_to_rgb(hue.rem_euclid(360.0) / 360.0, s.abs().min(1.0), lighten(ll));
    }
    let (hh, ss, ll) = rgb_to_hsl(c.map(lighten));
    let hh = (hh + hue / 360.0).rem_euclid(1.0);
    let ss = if s > 0.0 { ss / (1.0 - s).max(1.0 / 512.0) } else { ss * (1.0 + s) }.clamp(0.0, 1.0);
    hsl_to_rgb(hh, ss, ll)
}

/// A range's Lightness: each channel toward the pixel's brightest one
/// (raised) or darkest one (lowered), not toward white or black.
fn range_lightness(c: [f32; 3], l: f32) -> [f32; 3] {
    if l == 0.0 { return c; }
    let l = l.clamp(-1.0, 1.0);
    let target = if l > 0.0 { c[0].max(c[1]).max(c[2]) } else { c[0].min(c[1]).min(c[2]) };
    c.map(|v| v + (target - v) * l.abs())
}

/// Black & White's gray: the color's gray part (smallest channel) plus
/// its secondary part (yellow, cyan or magenta) and primary part (red,
/// green or blue), each weighted by its slider (%).
pub fn black_white_gray(c: [f32; 3], weights: &[f32; 6]) -> f32 {
    let (r, g, b) = (c[0], c[1], c[2]);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let mid = r + g + b - max - min;
    let primary = if r >= g && r >= b { 0 } else if g >= b { 2 } else { 4 };
    let secondary = if b <= r && b <= g { 1 } else if r <= g { 3 } else { 5 };
    (min + (mid - min) * weights[secondary] / 100.0 + (max - mid) * weights[primary] / 100.0).clamp(0.0, 1.0)
}

pub fn rgb_to_hsl(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-7 { return (0.0, 0.0, l); }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    (h / 6.0, s, l)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= 0.0 { return [l; 3]; }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

pub type Matrix = [[f32; 3]; 3];

/// The W3C spec's sepia() matrix.
pub fn sepia(a: f32) -> Matrix {
    let k = 1.0 - a.clamp(0.0, 1.0);
    [
        [0.393 + 0.607 * k, 0.769 - 0.769 * k, 0.189 - 0.189 * k],
        [0.349 - 0.349 * k, 0.686 + 0.314 * k, 0.168 - 0.168 * k],
        [0.272 - 0.272 * k, 0.534 - 0.534 * k, 0.131 + 0.869 * k],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photoshop_hue_saturation() {
        // Hue +40, Saturation +25, Lightness -10: (240, 160, 30) → (173, 243, 0).
        let hs = HueSat::new(40.0, 25.0, -10.0, false, &std::array::from_fn(HueRange::neutral));
        let o = hs.apply([240.0 / 255.0, 160.0 / 255.0, 30.0 / 255.0]).map(|v| (v * 255.0).round() as i32);
        assert!((o[0] - 173).abs() <= 1 && (o[1] - 243).abs() <= 1 && o[2] <= 1, "{o:?}");
        // Blues range lightness +50: (0, 29, 115) → (57, 71, 115).
        let mut r: [HueRange; 6] = std::array::from_fn(HueRange::neutral);
        r[4].lightness = 50.0;
        let o = HueSat::new(0.0, 0.0, 0.0, false, &r).apply([0.0, 29.0 / 255.0, 115.0 / 255.0]).map(|v| (v * 255.0).round() as i32);
        assert!((o[0] - 57).abs() <= 1 && (o[1] - 71).abs() <= 1 && o[2] == 115, "{o:?}");
        // Neutral range weights: reds are full at 0°, none at 90°.
        assert_eq!((HueRange::neutral(0).weight(0.0), HueRange::neutral(0).weight(90.0)), (1.0, 0.0));
    }

    #[test]
    fn black_white_defaults() {
        // Pure red at 40 %, pure yellow at 60 %, white stays white.
        assert!((black_white_gray([1.0, 0.0, 0.0], &BW_DEFAULTS) - 0.4).abs() < 1e-6);
        assert!((black_white_gray([1.0, 1.0, 0.0], &BW_DEFAULTS) - 0.6).abs() < 1e-6);
        assert_eq!(black_white_gray([1.0, 1.0, 1.0], &BW_DEFAULTS), 1.0);
    }
}

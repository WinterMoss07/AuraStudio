//! Filters: tones, colors, sharpening, lines, edges and shadow as
//! Photoshop (PhotoCraft's measurements, MIT, see NOTICE); blur, sepia,
//! invert, opacity per the W3C Filter Effects spec. Straight 8-bit RGBA,
//! blurs premultiplied, edges repeated.

// Pixel code indexes channels side by side (p[c], table[c]...).
#![allow(clippy::needless_range_loop)]

pub mod color;
pub mod spatial;
pub mod tone;

pub use color::{HueRange, BW_DEFAULTS, RANGE_NAMES};
pub use tone::LevelsChannel;

/// How far around the painted area (or the selection) the filters work
/// at least, px: a filter reaching farther (`Filter::reach`) gets more.
pub const MARGIN: u32 = 512;

/// How a value is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Percent,
    Pixels,
    Degrees,
    /// A tone level, 0..255.
    Level,
    /// A plain number with this many decimals.
    Number(usize),
}

/// A value with a plain slider in the dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub name:  &'static str,
    /// Its place in `Filter::values`.
    pub index: usize,
    pub min:   f32,
    pub max:   f32,
    pub unit:  Unit,
}

const fn p(name: &'static str, index: usize, min: f32, max: f32, unit: Unit) -> Param { Param { name, index, min, max, unit } }

/// The filters, as the Filter menu lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    BrightnessContrast, Levels, HueSaturation, BlackWhite,
    Invert, Posterize, Threshold, Sepia, Opacity,
    GaussianBlur, UnsharpMask,
    Thicken, Thin, FindEdges, Emboss,
    DropShadow,
}

/// Menu groups, in order.
pub const GROUPS: &[(&str, &[Kind])] = &[
    ("Adjust", &[Kind::BrightnessContrast, Kind::Levels, Kind::HueSaturation, Kind::BlackWhite]),
    ("Tones", &[Kind::Invert, Kind::Posterize, Kind::Threshold, Kind::Sepia, Kind::Opacity]),
    ("Blur & sharpen", &[Kind::GaussianBlur, Kind::UnsharpMask]),
    ("Lines & edges", &[Kind::Thicken, Kind::Thin, Kind::FindEdges, Kind::Emboss]),
    ("Effects", &[Kind::DropShadow]),
];

/// Value places of the filters with many values.
pub mod idx {
    /// Brightness / Contrast: brightness, contrast, legacy (0 / 1).
    pub const LEGACY: usize = 2;
    /// Levels: channel (0 RGB, 1 R, 2 G, 3 B) × 5 + in black, gamma, in
    /// white, out black, out white.
    pub const fn levels(ch: usize) -> usize { ch * 5 }
    /// Hue / Saturation: hue, saturation, lightness, colorize; then each
    /// range: hue, saturation, lightness, its four bounds.
    pub const COLORIZE: usize = 3;
    pub const fn range(i: usize) -> usize { 4 + i * 7 }
    /// Black & White: six weights, then tint (0 / 1; the tint is the
    /// filter's color).
    pub const TINT: usize = 6;
    /// Minimum / Maximum: radius, round (0 / 1).
    pub const ROUND: usize = 1;
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::BrightnessContrast => "Brightness / Contrast",
            Kind::Levels => "Levels",
            Kind::HueSaturation => "Hue / Saturation",
            Kind::BlackWhite => "Black & White",
            Kind::Invert => "Invert",
            Kind::Posterize => "Posterize",
            Kind::Threshold => "Threshold",
            Kind::Sepia => "Sepia",
            Kind::Opacity => "Opacity",
            Kind::GaussianBlur => "Gaussian blur",
            Kind::UnsharpMask => "Unsharp mask",
            Kind::Thicken => "Thicken lines",
            Kind::Thin => "Thin lines",
            Kind::FindEdges => "Find edges",
            Kind::Emboss => "Emboss",
            Kind::DropShadow => "Drop shadow",
        }
    }


    /// One line under the dialog's title.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::BrightnessContrast => "Lighter, darker, more or less contrast",
            Kind::Levels => "Black point, white point, midtones",
            Kind::HueSaturation => "Hue, saturation, lightness",
            Kind::BlackWhite => "Grayscale, each color its own lightness",
            Kind::Invert => "Negative",
            Kind::Posterize => "A few flat tones",
            Kind::Threshold => "Black or white",
            Kind::Sepia => "Old-photo browns",
            Kind::Opacity => "Fade the layer",
            Kind::GaussianBlur => "Soft blur",
            Kind::UnsharpMask => "Crisper edges",
            Kind::Thicken => "Dark lines grow",
            Kind::Thin => "Dark lines shrink",
            Kind::FindEdges => "Outlines, dark on white",
            Kind::Emboss => "Gray relief",
            Kind::DropShadow => "A soft shadow under the layer",
        }
    }

    /// The values that get a plain slider (Levels and Hue / Saturation
    /// have their own editors).
    pub fn params(self) -> &'static [Param] {
        use Unit::*;
        match self {
            Kind::BrightnessContrast => { const P: &[Param] = &[p("Brightness", 0, -150.0, 150.0, Number(0)), p("Contrast", 1, -50.0, 100.0, Number(0))]; P }
            Kind::Levels | Kind::HueSaturation => &[],
            Kind::BlackWhite => &[],
            Kind::Invert | Kind::Sepia => { const P: &[Param] = &[p("Amount", 0, 0.0, 100.0, Percent)]; P }
            Kind::Opacity => { const P: &[Param] = &[p("Opacity", 0, 0.0, 100.0, Percent)]; P }
            Kind::Posterize => { const P: &[Param] = &[p("Levels", 0, 2.0, 64.0, Number(0))]; P }
            Kind::Threshold => { const P: &[Param] = &[p("Threshold level", 0, 1.0, 255.0, Level)]; P }
            Kind::GaussianBlur => { const P: &[Param] = &[p("Radius", 0, 0.0, 250.0, Pixels)]; P }
            Kind::UnsharpMask => { const P: &[Param] = &[p("Amount", 0, 1.0, 500.0, Percent), p("Radius", 1, 0.1, 100.0, Pixels), p("Threshold", 2, 0.0, 255.0, Level)]; P }
            Kind::Thicken | Kind::Thin => { const P: &[Param] = &[p("Radius", 0, 0.0, 50.0, Pixels)]; P }
            Kind::FindEdges => { const P: &[Param] = &[p("Strength", 0, 0.0, 100.0, Percent)]; P }
            Kind::Emboss => { const P: &[Param] = &[p("Height", 1, 1.0, 20.0, Pixels), p("Amount", 2, 1.0, 500.0, Percent)]; P }
            Kind::DropShadow => { const P: &[Param] = &[
                p("Distance", 1, 0.0, 300.0, Pixels),
                p("Spread", 2, 0.0, 100.0, Percent),
                p("Size", 3, 0.0, 250.0, Pixels),
                p("Opacity", 4, 0.0, 100.0, Percent),
            ]; P }
        }
    }

    pub fn defaults(self) -> Vec<f32> {
        match self {
            Kind::BrightnessContrast => vec![0.0, 0.0, 0.0],
            Kind::Levels => (0..4).flat_map(|_| [0.0, 1.0, 255.0, 0.0, 255.0]).collect(),
            Kind::HueSaturation => {
                let mut v = vec![0.0; 4];
                for i in 0..6 { let r = HueRange::neutral(i); v.extend([0.0, 0.0, 0.0, r.bounds[0], r.bounds[1], r.bounds[2], r.bounds[3]]); }
                v
            }
            Kind::BlackWhite => { let mut v = BW_DEFAULTS.to_vec(); v.push(0.0); v }
            Kind::Invert | Kind::Sepia => vec![100.0],
            Kind::Opacity => vec![50.0],
            Kind::Posterize => vec![4.0],
            Kind::Threshold => vec![128.0],
            Kind::GaussianBlur => vec![4.0],
            Kind::UnsharpMask => vec![80.0, 2.0, 0.0],
            Kind::Thicken | Kind::Thin => vec![1.0, 0.0],
            Kind::FindEdges => vec![100.0],
            Kind::Emboss => vec![135.0, 3.0, 100.0],
            Kind::DropShadow => vec![120.0, 12.0, 0.0, 16.0, 60.0],
        }
    }

}

/// A filter with its values (`Kind::defaults` layout), ready to apply.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub kind:   Kind,
    pub values: Vec<f32>,
    /// Drop shadow color, Black & White tint.
    pub color:  [u8; 3],
}

impl Filter {
    pub fn new(kind: Kind) -> Self {
        let color = match kind { Kind::BlackWhite => [225, 211, 179], _ => [0, 0, 0] };
        Self { kind, values: kind.defaults(), color }
    }

    /// Value `i`, finite (a missing one: its default).
    pub fn v(&self, i: usize) -> f32 {
        let v = self.values.get(i).copied().filter(|v| v.is_finite());
        let v = v.unwrap_or_else(|| self.kind.defaults().get(i).copied().unwrap_or(0.0));
        match self.kind.params().iter().find(|p| p.index == i) { Some(p) => v.clamp(p.min, p.max), None => v }
    }

    fn on(&self, i: usize) -> bool { self.v(i) >= 0.5 }

    pub fn levels(&self, ch: usize) -> LevelsChannel { LevelsChannel::from(self.values.get(idx::levels(ch)..idx::levels(ch) + 5).unwrap_or(&[])) }

    pub fn range(&self, i: usize) -> HueRange {
        let b = idx::range(i);
        HueRange { hue: self.v(b).clamp(-180.0, 180.0), saturation: self.v(b + 1).clamp(-100.0, 100.0), lightness: self.v(b + 2).clamp(-100.0, 100.0), bounds: [self.v(b + 3), self.v(b + 4), self.v(b + 5), self.v(b + 6)] }
    }

    /// The same filter for an image reduced `k` times (a zoomed-out
    /// preview): sizes in pixels scaled by `k`.
    pub fn scaled(&self, k: f32) -> Filter {
        let mut f = self.clone();
        let px: &[usize] = match self.kind {
            Kind::GaussianBlur | Kind::Thicken | Kind::Thin => &[0],
            Kind::UnsharpMask => &[1],
            Kind::Emboss => &[1],
            Kind::DropShadow => &[1, 3],
            _ => &[],
        };
        for &i in px { if let Some(v) = f.values.get_mut(i) { *v *= k; } }
        f
    }

    /// How far (px) a pixel's result reaches out to read others.
    pub fn reach(&self) -> u32 {
        match self.kind {
            Kind::GaussianBlur => spatial::blur_reach(self.v(0)),
            Kind::UnsharpMask => spatial::blur_reach(self.v(1)),
            Kind::FindEdges => 1,
            Kind::Emboss => self.v(1).round() as u32 + 1,
            Kind::Thicken | Kind::Thin => self.v(0).ceil() as u32,
            Kind::DropShadow => {
                let (size, spread) = (self.v(3), self.v(2) / 100.0);
                self.v(1).ceil() as u32 + (size * spread).round() as u32 + spatial::blur_reach(0.4 * size * (1.0 - spread))
            }
            _ => 0,
        }
    }

    /// The tone tables (R, G, B: input level → output) of the filters that
    /// are one, to draw their curve.
    pub fn tone_tables(&self) -> Option<[[u8; 256]; 3]> {
        Some(match self.kind {
            Kind::BrightnessContrast => [tone::brightness_contrast(self.v(0), self.v(1), self.on(idx::LEGACY)); 3],
            Kind::Levels => tone::levels(self.levels(0), [self.levels(1), self.levels(2), self.levels(3)]),
            Kind::Posterize => [tone::posterize(self.v(0).round() as u32); 3],
            Kind::Invert => { let a = self.v(0) / 100.0; [std::array::from_fn(|i| { let c = i as f32 / 255.0; ((a + c * (1.0 - 2.0 * a)) * 255.0).round() as u8 }); 3] }
            Kind::Threshold => { let t = self.v(0); [std::array::from_fn(|i| if tone::threshold_white([i as u8; 3], t) { 255 } else { 0 }); 3] }
            _ => return None,
        })
    }

    /// The filtered pixels of a `w` × `h` image.
    pub fn apply(&self, src: &[u8], w: usize, h: usize) -> Vec<u8> {
        assert_eq!(src.len(), w * h * 4);
        if w == 0 || h == 0 { return src.to_vec(); }
        let mut out = src.to_vec();
        match self.kind {
            Kind::BrightnessContrast | Kind::Levels | Kind::Posterize | Kind::Invert => {
                let t = self.tone_tables().unwrap();
                spatial::par_rows(&mut out, w, |_, row| for p in row.chunks_exact_mut(4) { for c in 0..3 { p[c] = t[c][p[c] as usize]; } });
            }
            Kind::Threshold => {
                let t = self.v(0);
                spatial::par_rows(&mut out, w, |_, row| for p in row.chunks_exact_mut(4) {
                    let v = if tone::threshold_white([p[0], p[1], p[2]], t) { 255 } else { 0 };
                    p[..3].fill(v);
                });
            }
            Kind::HueSaturation => {
                let ranges: [HueRange; 6] = std::array::from_fn(|i| self.range(i));
                let hs = color::HueSat::new(self.v(0), self.v(1), self.v(2), self.on(idx::COLORIZE), &ranges);
                if !hs.is_identity() { per_color(&mut out, w, |c| hs.apply(c)); }
            }
            Kind::BlackWhite => {
                let wt: [f32; 6] = std::array::from_fn(|i| self.v(i).clamp(-200.0, 300.0));
                let tint = self.on(idx::TINT).then(|| { let (h, s, _) = color::rgb_to_hsl(self.color.map(|v| v as f32 / 255.0)); (h, s) });
                per_color(&mut out, w, |c| {
                    let g = color::black_white_gray(c, &wt);
                    match tint {
                        // Tint: the color's hue and saturation at the gray's lightness.
                        Some((h, s)) => color::hsl_to_rgb(h, s, g),
                        None => [g; 3],
                    }
                });
            }
            Kind::Sepia => {
                let m = color::sepia(self.v(0) / 100.0);
                per_color(&mut out, w, |c| std::array::from_fn(|k| m[k][0] * c[0] + m[k][1] * c[1] + m[k][2] * c[2]));
            }
            Kind::Opacity => { let a = self.v(0) / 100.0; for px in out.chunks_exact_mut(4) { px[3] = (px[3] as f32 * a).round() as u8; } }
            Kind::GaussianBlur => out = spatial::blur_rgba(src, w, h, self.v(0)),
            Kind::UnsharpMask => out = spatial::unsharp(src, w, h, self.v(0) / 100.0, self.v(1), self.v(2)),
            Kind::Thicken | Kind::Thin => out = spatial::lines(src, w, h, self.v(0), self.on(idx::ROUND), self.kind == Kind::Thicken),
            Kind::FindEdges => out = spatial::find_edges(src, w, h, self.v(0) / 100.0),
            Kind::Emboss => out = spatial::emboss(src, w, h, self.v(0), self.v(1), self.v(2) / 100.0),
            Kind::DropShadow => out = spatial::drop_shadow(src, w, h, self.v(0), self.v(1), self.v(2) / 100.0, self.v(3), self.v(4) / 100.0, self.color),
        }
        out
    }
}

/// `f` on every pixel's straight color (0..1), alpha unchanged; colors
/// already seen are remembered (a painting has far fewer colors than
/// pixels).
fn per_color(px: &mut [u8], w: usize, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    spatial::par_rows(px, w, |_, row| {
        let mut last = ([0u8; 3], None::<[u8; 3]>);
        for p in row.chunks_exact_mut(4) {
            if p[3] == 0 { continue; }
            let c = [p[0], p[1], p[2]];
            let o = match last {
                (k, Some(o)) if k == c => o,
                _ => {
                    let o = f(c.map(|v| v as f32 / 255.0)).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                    last = (c, Some(o));
                    o
                }
            };
            p[..3].copy_from_slice(&o);
        }
    });
}

/// Counts of each level among the painted pixels of a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    /// R, G and B together (Levels' RGB channel).
    pub rgb:   [u32; 256],
    pub chan:  [[u32; 256]; 3],
    /// 30 R + 59 G + 11 B (what Threshold splits on).
    pub lum:   [u32; 256],
    pub count: u64,
}

impl Histogram {
    /// The histogram of the pixels `keep` accepts (by index) that aren't
    /// transparent.
    pub fn of(px: &[u8], keep: impl Fn(usize) -> bool) -> Self {
        let mut h = Histogram { rgb: [0; 256], chan: [[0; 256]; 3], lum: [0; 256], count: 0 };
        for (i, p) in px.chunks_exact(4).enumerate() {
            if p[3] == 0 || !keep(i) { continue; }
            for c in 0..3 { h.chan[c][p[c] as usize] += 1; }
            h.lum[tone::luminosity([p[0], p[1], p[2]]) as usize] += 1;
            h.count += 1;
        }
        for i in 0..256 { h.rgb[i] = h.chan[0][i] + h.chan[1][i] + h.chan[2][i]; }
        h
    }

    /// Levels' channel `ch` (0 RGB, 1..3 R, G, B).
    pub fn channel(&self, ch: usize) -> &[u32; 256] { if ch == 0 { &self.rgb } else { &self.chan[ch - 1] } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h { for x in 0..w { v.extend_from_slice(&f(x, y)); } }
        v
    }

    fn with(kind: Kind, values: &[f32]) -> Filter { Filter { kind, values: values.to_vec(), color: [0, 0, 0] } }

    #[test]
    fn neutral_settings_change_nothing() {
        let src = img(16, 8, |x, y| [(x * 15) as u8, (y * 30) as u8, 77, 255]);
        for f in [with(Kind::BrightnessContrast, &[0.0, 0.0, 0.0]), Filter::new(Kind::Levels), Filter::new(Kind::HueSaturation),
                  with(Kind::Invert, &[0.0]), with(Kind::Sepia, &[0.0]), with(Kind::Opacity, &[100.0]),
                  with(Kind::GaussianBlur, &[0.0]), with(Kind::Thicken, &[0.0, 0.0]), with(Kind::UnsharpMask, &[1.0, 2.0, 255.0])] {
            let out = f.apply(&src, 16, 8);
            let worst = out.iter().zip(&src).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(worst <= 1, "{:?}: {worst}", f.kind);
        }
    }

    #[test]
    fn blur_keeps_the_total_and_spreads() {
        let (w, h) = (64, 64);
        let src = img(w, h, |x, y| if (28..36).contains(&x) && (28..36).contains(&y) { [255, 0, 0, 255] } else { [0, 0, 0, 0] });
        for sigma in [0.8f32, 3.0, 8.0] {
            let out = with(Kind::GaussianBlur, &[sigma]).apply(&src, w, h);
            let total = |v: &[u8]| v.chunks(4).map(|p| p[3] as u64).sum::<u64>();
            let (a, b) = (total(&src) as f64, total(&out) as f64);
            assert!((a - b).abs() / a < 0.03, "σ {sigma}: alpha {a} -> {b}");
            assert!(out[(27 * w + 32) * 4 + 3] > 0, "σ {sigma}: nothing spread");
            let p = &out[(26 * w + 32) * 4..][..4];
            if p[3] > 10 { assert!(p[0] > 240 && p[1] < 10, "σ {sigma}: {p:?}"); }
        }
    }

    #[test]
    fn thicken_and_thin_work_on_paper_and_on_line_art() {
        let (w, h) = (32, 32);
        let dark = |v: &[u8], y: usize| { let p = &v[(y * w + 5) * 4..][..4]; p[3] > 0 && p[0] < 128 };
        let rows = |v: &[u8]| (0..h).filter(|&y| dark(v, y)).count();
        // A black line on a transparent layer.
        let art = img(w, h, |_, y| if y == 16 { [0, 0, 0, 255] } else { [255, 255, 255, 0] });
        assert_eq!(rows(&with(Kind::Thicken, &[2.0, 0.0]).apply(&art, w, h)), 5);
        // A black line on white paper.
        let paper = img(w, h, |_, y| if (12..20).contains(&y) { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        assert_eq!(rows(&with(Kind::Thicken, &[2.0, 1.0]).apply(&paper, w, h)), 12);
        assert_eq!(rows(&with(Kind::Thin, &[2.0, 0.0]).apply(&paper, w, h)), 4);
        let thick = img(w, h, |_, y| if (12..20).contains(&y) { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
        assert_eq!(rows(&with(Kind::Thin, &[2.0, 0.0]).apply(&thick, w, h)), 4);
    }

    #[test]
    fn threshold_posterize_and_levels() {
        let src = img(4, 1, |x, _| { let v = [10u8, 100, 160, 250][x]; [v, v, v, 255] });
        let t = with(Kind::Threshold, &[128.0]).apply(&src, 4, 1);
        assert_eq!(t.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), [0, 0, 255, 255]);
        let p = with(Kind::Posterize, &[2.0]).apply(&src, 4, 1);
        assert!(p.chunks(4).all(|q| q[0] == 0 || q[0] == 255));
        // Red channel's white point at 128: red doubles, green stays.
        let mut l = Filter::new(Kind::Levels);
        l.values[idx::levels(1) + 2] = 128.0;
        let o = l.apply(&[64, 64, 64, 255], 1, 1);
        assert!((o[0] as i32 - 128).abs() <= 1 && o[1] == 64, "{o:?}");
    }

    #[test]
    fn drop_shadow_lands_away_from_the_light() {
        let (w, h) = (32, 32);
        let src = img(w, h, |x, y| if (8..16).contains(&x) && (8..16).contains(&y) { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
        // Light from 135° (top left), 6 px, hard: shadow below right.
        let f = Filter { kind: Kind::DropShadow, values: vec![135.0, 8.5, 0.0, 0.0, 100.0], color: [0, 0, 255] };
        let out = f.apply(&src, w, h);
        let at = |x: usize, y: usize| &out[(y * w + x) * 4..][..4];
        assert_eq!(at(10, 10), [255, 255, 255, 255], "source on top");
        assert_eq!(at(19, 19), [0, 0, 255, 255], "shadow below right");
        assert_eq!(at(4, 4)[3], 0);
    }

    #[test]
    fn find_edges_outlines_line_art() {
        let (w, h) = (16, 16);
        let art = img(w, h, |x, _| if x == 8 { [0, 0, 0, 255] } else { [255, 255, 255, 0] });
        let out = Filter::new(Kind::FindEdges).apply(&art, w, h);
        let p = &out[(8 * w + 7) * 4..][..4];
        assert!(p[3] > 200 && p[0] < 60, "an edge beside the line: {p:?}");
        assert_eq!(out[(8 * w + 2) * 4 + 3], 0, "nothing far from it");
    }

    #[test]
    fn histogram_counts_painted_pixels() {
        let src = img(4, 1, |x, _| [x as u8 * 80, 0, 0, if x == 3 { 0 } else { 255 }]);
        let h = Histogram::of(&src, |_| true);
        assert_eq!(h.count, 3);
        assert_eq!((h.chan[0][0], h.chan[0][80], h.chan[0][240]), (1, 1, 0));
        assert_eq!(h.rgb[0], 7);
    }

    #[test]
    fn every_filter_runs_on_its_defaults_and_scales() {
        let (w, h) = (24, 20);
        let src = img(w, h, |x, y| [(x * 10) as u8, (y * 12) as u8, 128, if x > 3 { 255 } else { 0 }]);
        for (_, kinds) in GROUPS {
            for &k in *kinds {
                let f = Filter::new(k);
                assert_eq!(f.apply(&src, w, h).len(), src.len(), "{k:?}");
                assert_eq!(f.scaled(0.5).apply(&src, w, h).len(), src.len(), "{k:?}");
                for p in k.params() { assert!(p.index < f.values.len(), "{k:?} {}", p.name); }
            }
        }
    }
}

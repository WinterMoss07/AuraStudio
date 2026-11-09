//! A filter with its values, and how it runs on pixels: prepared once
//! (tables, Hue / Saturation ranges), then run on any number of pieces.

use aurastudio_core::{math, Rgba8};
use rayon::prelude::*;

use crate::color::{self, HueRange, HueSat, Matrix};
use crate::lut::{table, Lut8};
use crate::params::{idx, Kind};
use crate::tone::{self, LevelsChannel};
use crate::{blur, spatial};

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

    /// Value `i`, finite (a missing one: its default), within its slider.
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
    /// preview, ½ or ¼ while a slider is dragged): sizes in pixels scaled.
    pub fn scaled(&self, k: f32) -> Filter {
        let mut f = self.clone();
        let px: &[usize] = match self.kind {
            Kind::GaussianBlur | Kind::Thicken | Kind::Thin => &[0],
            Kind::UnsharpMask | Kind::Emboss => &[1],
            Kind::DropShadow => &[1, 3],
            _ => &[],
        };
        for &i in px { if let Some(v) = f.values.get_mut(i) { *v *= k; } }
        f
    }

    /// How far (px) a pixel's result reaches out to read others.
    pub fn reach(&self) -> u32 {
        match self.kind {
            Kind::GaussianBlur => blur::reach(self.v(0)),
            Kind::UnsharpMask => blur::reach(self.v(1)),
            Kind::FindEdges => 1,
            Kind::Emboss => self.v(1).round() as u32 + 1,
            Kind::Thicken | Kind::Thin => self.v(0).ceil() as u32,
            Kind::DropShadow => {
                let (size, spread) = (self.v(3), self.v(2) / 100.0);
                self.v(1).ceil() as u32 + (size * spread).round() as u32 + blur::reach(0.4 * size * (1.0 - spread))
            }
            _ => 0,
        }
    }

    /// The tone tables of the filters that are one, to draw their curve.
    pub fn tone_tables(&self) -> Option<Lut8> {
        Some(match self.kind {
            Kind::BrightnessContrast => Lut8::same(tone::brightness_contrast(self.v(0), self.v(1), self.on(idx::LEGACY))),
            Kind::Levels => tone::levels(self.levels(0), [self.levels(1), self.levels(2), self.levels(3)]),
            Kind::Posterize => Lut8::same(tone::posterize(self.v(0).round() as u32)),
            Kind::Invert => { let a = self.v(0) / 100.0; Lut8::same(table(|c| a + c * (1.0 - 2.0 * a))) }
            Kind::Threshold => { let t = self.v(0); Lut8::same(std::array::from_fn(|i| if tone::threshold_white([i as u8; 3], t) { 255 } else { 0 })) }
            _ => return None,
        })
    }

    /// Everything worked out from the values once (tables, ranges), to
    /// run on many pieces.
    pub fn prepare(&self) -> Prepared {
        let op = match self.kind {
            Kind::BrightnessContrast | Kind::Levels | Kind::Posterize | Kind::Invert => match self.tone_tables() {
                Some(t) if !t.is_identity() => Op::Lut(Box::new(t)),
                _ => Op::Identity,
            },
            Kind::Threshold => Op::Threshold(self.v(0)),
            Kind::HueSaturation => {
                let ranges: [HueRange; 6] = std::array::from_fn(|i| self.range(i));
                let hs = HueSat::new(self.v(0), self.v(1), self.v(2), self.on(idx::COLORIZE), &ranges);
                if hs.is_identity() { Op::Identity } else { Op::HueSat(hs) }
            }
            Kind::BlackWhite => {
                let wt: [f32; 6] = std::array::from_fn(|i| self.v(i).clamp(-200.0, 300.0));
                let tint = self.on(idx::TINT).then(|| { let (h, s, _) = color::rgb_to_hsl(self.color.map(math::from_u8)); (h, s) });
                Op::BlackWhite(wt, tint)
            }
            Kind::Sepia => if self.v(0) <= 0.0 { Op::Identity } else { Op::Sepia(color::sepia(self.v(0) / 100.0)) },
            Kind::Opacity => if self.v(0) >= 100.0 { Op::Identity } else { Op::Opacity(self.v(0) / 100.0) },
            Kind::GaussianBlur => if self.v(0) <= 0.0 { Op::Identity } else { Op::Blur(self.v(0)) },
            Kind::UnsharpMask => Op::Unsharp { amount: self.v(0) / 100.0, sigma: self.v(1), threshold: self.v(2) },
            Kind::Thicken | Kind::Thin => Op::Lines { radius: self.v(0), round: self.on(idx::ROUND), grow: self.kind == Kind::Thicken },
            Kind::FindEdges => Op::Edges(self.v(0) / 100.0),
            Kind::Emboss => Op::Emboss { angle: self.v(0), height: self.v(1), amount: self.v(2) / 100.0 },
            Kind::DropShadow => Op::Shadow { angle: self.v(0), distance: self.v(1), spread: self.v(2) / 100.0, size: self.v(3), opacity: self.v(4) / 100.0 },
        };
        Prepared { op, color: self.color, reach: self.reach() }
    }

    /// The filtered pixels of a `w` × `h` image (rows of straight RGBA),
    /// edges repeated.
    pub fn apply(&self, src: &[Rgba8], w: usize, h: usize) -> Vec<Rgba8> { self.prepare().run(src, w, h) }
}

pub(crate) enum Op {
    Identity,
    Lut(Box<Lut8>),
    Threshold(f32),
    HueSat(HueSat),
    BlackWhite([f32; 6], Option<(f32, f32)>),
    Sepia(Matrix),
    Opacity(f32),
    Blur(f32),
    Unsharp { amount: f32, sigma: f32, threshold: f32 },
    Lines { radius: f32, round: bool, grow: bool },
    Edges(f32),
    Emboss { angle: f32, height: f32, amount: f32 },
    Shadow { angle: f32, distance: f32, spread: f32, size: f32, opacity: f32 },
}

/// A filter ready to run (`Filter::prepare`).
pub struct Prepared {
    pub(crate) op:    Op,
    color:            [u8; 3],
    pub(crate) reach: u32,
}

impl Prepared {
    /// Changes nothing anywhere.
    pub fn is_identity(&self) -> bool { matches!(self.op, Op::Identity) }

    pub fn reach(&self) -> u32 { self.reach }

    /// The filtered pixels of a `w` × `h` image, edges repeated.
    pub fn run(&self, src: &[Rgba8], w: usize, h: usize) -> Vec<Rgba8> {
        if src.len() != w * h || w == 0 || h == 0 { return src.to_vec(); }
        aurastudio_core::zone!("filter");
        // Spatial filters make their own result; the others work on a copy.
        match &self.op {
            Op::Blur(s) => return blur::blur_rgba(src, w, h, *s),
            Op::Unsharp { amount, sigma, threshold } => return spatial::unsharp(src, w, h, *amount, *sigma, *threshold),
            Op::Lines { radius, round, grow } => return spatial::lines(src, w, h, *radius, *round, *grow),
            Op::Edges(s) => return spatial::find_edges(src, w, h, *s),
            Op::Emboss { angle, height, amount } => return spatial::emboss(src, w, h, *angle, *height, *amount),
            Op::Shadow { angle, distance, spread, size, opacity } => return spatial::drop_shadow(src, w, h, *angle, *distance, *spread, *size, *opacity, self.color),
            _ => {}
        }
        let mut out = src.to_vec();
        match &self.op {
            Op::Lut(t) => out.par_iter_mut().with_min_len(4096).for_each(|p| *p = t.map(*p)),
            Op::Threshold(t) => out.par_iter_mut().with_min_len(4096).for_each(|p| {
                let v = if tone::threshold_white([p[0], p[1], p[2]], *t) { 255 } else { 0 };
                p[..3].fill(v);
            }),
            Op::HueSat(hs) => per_color(&mut out, |c| hs.apply(c)),
            Op::BlackWhite(wt, tint) => per_color(&mut out, |c| {
                let g = color::black_white_gray(c, wt);
                match tint {
                    // Tint: the color's hue and saturation at the gray's lightness.
                    Some((h, s)) => color::hsl_to_rgb(*h, *s, g),
                    None => [g; 3],
                }
            }),
            Op::Sepia(m) => per_color(&mut out, |c| std::array::from_fn(|k| m[k][0] * c[0] + m[k][1] * c[1] + m[k][2] * c[2])),
            Op::Opacity(a) => out.par_iter_mut().with_min_len(4096).for_each(|p| p[3] = (p[3] as f32 * a + 0.5) as u8),
            _ => {}
        }
        out
    }

    /// What an area of one color `c` (as wide as the reach around) becomes:
    /// one color too. Invisible stays invisible as it was (a transparent
    /// pixel's color is no change worth storing).
    pub fn on_uniform(&self, c: Rgba8) -> Rgba8 {
        let g = match &self.op {
            Op::Identity => c,
            // The shadow of an even area is that area's alpha, right under it.
            Op::Shadow { opacity, .. } => spatial::shadow_pixel(c, math::from_u8(c[3]) * opacity, self.color),
            _ => self.run(&[c], 1, 1).first().copied().unwrap_or(c),
        };
        if c[3] == 0 && g[3] == 0 { c } else { g }
    }
}

/// `f` on every pixel's straight color (0..1), alpha unchanged. Colors
/// already seen are remembered (a painting has far fewer colors than
/// pixels; a line art tile has one).
fn per_color(px: &mut [Rgba8], f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    const SLOTS: usize = 1024;
    px.par_chunks_mut(16 * 1024).for_each(|part| {
        let mut keys = [u32::MAX; SLOTS];
        let mut vals = [[0u8; 3]; SLOTS];
        for p in part {
            if p[3] == 0 { continue; }
            let k = u32::from_le_bytes([p[0], p[1], p[2], 0]);
            let s = (k.wrapping_mul(0x9E37_79B1) >> 22) as usize % SLOTS;
            if keys[s] != k {
                keys[s] = k;
                vals[s] = f([p[0], p[1], p[2]].map(math::from_u8)).map(|v| math::to_u8(v.clamp(0.0, 1.0)));
            }
            p[..3].copy_from_slice(&vals[s]);
        }
    });
}

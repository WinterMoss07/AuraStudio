//! Image filters, following the W3C Filter Effects Module Level 1
//! (https://www.w3.org/TR/filter-effects-1/), the filter counterpart of
//! the blend modes' Compositing and Blending spec: the CSS filter
//! functions (brightness, contrast, saturate, hue-rotate, grayscale,
//! sepia, invert, opacity, blur, drop-shadow) with the matrices and
//! transfer functions the spec gives for them, and filter primitives
//! (feComponentTransfer, feConvolveMatrix, feMorphology, feComposite)
//! for levels, posterize, threshold, sharpen, emboss, edges, and
//! thickening / thinning lines.
//!
//! As in the spec: color matrices and transfer functions work on
//! straight (non-premultiplied) colors, the others on premultiplied ones.
//! Values are the layer's sRGB-encoded ones (`color-interpolation-filters:
//! sRGB`, what painting programs do). Outside the image, edge pixels
//! repeat (`edgeMode="duplicate"`): no dark or transparent fringe at the
//! canvas border.
//!
//! Pixels are straight RGBA, 8 bits, rows tightly packed.

/// How a parameter is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    /// 0..1 shown as 0..100 %.
    Percent,
    Pixels,
    Degrees,
    /// A plain number with this many decimals.
    Number(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub name:    &'static str,
    pub min:     f32,
    pub max:     f32,
    pub default: f32,
    pub unit:    Unit,
}

const fn p(name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Param {
    Param { name, min, max, default, unit }
}

/// The filters, as the Filter menu lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Brightness, Contrast, Saturate, HueRotate, Grayscale, Sepia, Invert, Opacity,
    Levels, Posterize, Threshold,
    GaussianBlur, Sharpen,
    Emboss, EdgeDetect,
    Dilate, Erode,
    DropShadow,
}

/// Menu groups, in order.
pub const GROUPS: &[(&str, &[Kind])] = &[
    ("Adjust", &[Kind::Brightness, Kind::Contrast, Kind::Saturate, Kind::HueRotate, Kind::Levels]),
    ("Color", &[Kind::Grayscale, Kind::Sepia, Kind::Invert, Kind::Posterize, Kind::Threshold, Kind::Opacity]),
    ("Blur & sharpen", &[Kind::GaussianBlur, Kind::Sharpen]),
    ("Lines & edges", &[Kind::Dilate, Kind::Erode, Kind::EdgeDetect, Kind::Emboss]),
    ("Effects", &[Kind::DropShadow]),
];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Brightness => "Brightness",
            Kind::Contrast => "Contrast",
            Kind::Saturate => "Saturation",
            Kind::HueRotate => "Hue rotate",
            Kind::Grayscale => "Grayscale",
            Kind::Sepia => "Sepia",
            Kind::Invert => "Invert",
            Kind::Opacity => "Opacity",
            Kind::Levels => "Levels",
            Kind::Posterize => "Posterize",
            Kind::Threshold => "Threshold",
            Kind::GaussianBlur => "Gaussian blur",
            Kind::Sharpen => "Sharpen (unsharp mask)",
            Kind::Emboss => "Emboss",
            Kind::EdgeDetect => "Edge detect",
            Kind::Dilate => "Thicken lines",
            Kind::Erode => "Thin lines",
            Kind::DropShadow => "Drop shadow",
        }
    }

    /// One line on what it does (and where it comes from in the spec).
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Brightness => "brightness(): every channel scaled (linear transfer)",
            Kind::Contrast => "contrast(): channels pushed away from or toward mid gray",
            Kind::Saturate => "saturate(): color matrix, 0 % gray to 400 % vivid",
            Kind::HueRotate => "hue-rotate(): colors turned around the hue circle",
            Kind::Grayscale => "grayscale(): toward luminance (Rec. 709 weights)",
            Kind::Sepia => "sepia(): toward old-photo browns",
            Kind::Invert => "invert(): negative (table transfer)",
            Kind::Opacity => "opacity(): the pixels' alpha scaled",
            Kind::Levels => "Black and white points and gamma (feComponentTransfer)",
            Kind::Posterize => "A few flat levels per channel (discrete transfer)",
            Kind::Threshold => "Black or white from the luminance: clean line art",
            Kind::GaussianBlur => "blur(): feGaussianBlur, three box blurs as the spec says",
            Kind::Sharpen => "Source plus the difference from a blurred copy (feComposite arithmetic)",
            Kind::Emboss => "Relief from a 3x3 kernel (feConvolveMatrix)",
            Kind::EdgeDetect => "Outlines from a 3x3 Laplacian kernel (feConvolveMatrix)",
            Kind::Dilate => "feMorphology dilate: lines and shapes grow",
            Kind::Erode => "feMorphology erode: lines and shapes shrink",
            Kind::DropShadow => "drop-shadow(): a blurred, offset shadow in the brush color",
        }
    }

    pub fn params(self) -> &'static [Param] {
        use Unit::*;
        match self {
            Kind::Brightness => { const P: &[Param] = &[p("Amount", 0.0, 3.0, 1.2, Percent)]; P }
            Kind::Contrast => { const P: &[Param] = &[p("Amount", 0.0, 3.0, 1.2, Percent)]; P }
            Kind::Saturate => { const P: &[Param] = &[p("Amount", 0.0, 4.0, 1.5, Percent)]; P }
            Kind::HueRotate => { const P: &[Param] = &[p("Angle", -180.0, 180.0, 30.0, Degrees)]; P }
            Kind::Grayscale | Kind::Sepia | Kind::Invert => { const P: &[Param] = &[p("Amount", 0.0, 1.0, 1.0, Percent)]; P }
            Kind::Opacity => { const P: &[Param] = &[p("Amount", 0.0, 1.0, 0.5, Percent)]; P }
            Kind::Levels => { const P: &[Param] = &[p("Black", 0.0, 1.0, 0.0, Percent), p("White", 0.0, 1.0, 1.0, Percent), p("Gamma", 0.1, 5.0, 1.0, Number(2))]; P }
            Kind::Posterize => { const P: &[Param] = &[p("Levels", 2.0, 32.0, 4.0, Number(0))]; P }
            Kind::Threshold => { const P: &[Param] = &[p("Level", 0.0, 1.0, 0.5, Percent)]; P }
            Kind::GaussianBlur => { const P: &[Param] = &[p("Radius", 0.0, 100.0, 4.0, Pixels)]; P }
            Kind::Sharpen => { const P: &[Param] = &[p("Amount", 0.0, 5.0, 1.0, Percent), p("Radius", 0.0, 50.0, 2.0, Pixels)]; P }
            Kind::Emboss | Kind::EdgeDetect => { const P: &[Param] = &[p("Strength", 0.0, 1.0, 1.0, Percent)]; P }
            Kind::Dilate | Kind::Erode => { const P: &[Param] = &[p("Radius", 1.0, 50.0, 1.0, Pixels)]; P }
            Kind::DropShadow => { const P: &[Param] = &[
                p("Offset x", -200.0, 200.0, 8.0, Pixels),
                p("Offset y", -200.0, 200.0, 8.0, Pixels),
                p("Blur", 0.0, 100.0, 6.0, Pixels),
                p("Opacity", 0.0, 1.0, 0.6, Percent),
            ]; P }
        }
    }

    pub fn defaults(self) -> Vec<f32> { self.params().iter().map(|p| p.default).collect() }
}

/// A filter with its values (`Kind::params` order), ready to apply.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub kind:   Kind,
    pub values: Vec<f32>,
    /// Drop shadow color.
    pub color:  [u8; 3],
}

impl Filter {
    pub fn new(kind: Kind) -> Self { Self { kind, values: kind.defaults(), color: [0, 0, 0] } }

    fn v(&self, i: usize) -> f32 {
        let p = self.kind.params()[i];
        self.values.get(i).copied().unwrap_or(p.default).clamp(p.min, p.max)
    }

    /// How far (px) a pixel's result reaches out to read others.
    pub fn reach(&self) -> u32 {
        match self.kind {
            Kind::GaussianBlur => blur_reach(self.v(0)),
            Kind::Sharpen => blur_reach(self.v(1)),
            Kind::Emboss | Kind::EdgeDetect => 1,
            Kind::Dilate | Kind::Erode => self.v(0).round() as u32,
            Kind::DropShadow => blur_reach(self.v(2)) + self.v(0).abs().max(self.v(1).abs()).ceil() as u32,
            _ => 0,
        }
    }

    /// The filtered pixels of a `w` × `h` image.
    pub fn apply(&self, src: &[u8], w: usize, h: usize) -> Vec<u8> {
        assert_eq!(src.len(), w * h * 4);
        let mut out = src.to_vec();
        match self.kind {
            Kind::Brightness => transfer(&mut out, |c| self.v(0) * c),
            Kind::Contrast => { let a = self.v(0); transfer(&mut out, |c| a * c - 0.5 * a + 0.5) }
            Kind::Invert => { let a = self.v(0); transfer(&mut out, |c| a + c * (1.0 - 2.0 * a)) }
            Kind::Levels => {
                let (b, wt, g) = (self.v(0), self.v(1), self.v(2));
                let span = (wt - b).abs().max(1e-3) * if wt >= b { 1.0 } else { -1.0 };
                transfer(&mut out, |c| ((c - b) / span).clamp(0.0, 1.0).powf(1.0 / g));
            }
            Kind::Posterize => {
                let n = self.v(0).round().max(2.0);
                transfer(&mut out, |c| (c * n).floor().min(n - 1.0) / (n - 1.0));
            }
            Kind::Saturate => color_matrix(&mut out, saturate(self.v(0))),
            Kind::HueRotate => color_matrix(&mut out, hue_rotate(self.v(0))),
            Kind::Grayscale => color_matrix(&mut out, grayscale(self.v(0))),
            Kind::Sepia => color_matrix(&mut out, sepia(self.v(0))),
            Kind::Opacity => { let a = self.v(0); for px in out.chunks_exact_mut(4) { px[3] = (px[3] as f32 * a).round() as u8; } }
            Kind::Threshold => {
                let t = self.v(0) * 255.0;
                for px in out.chunks_exact_mut(4) {
                    let l = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
                    let v = if l >= t { 255 } else { 0 };
                    px[..3].fill(v);
                }
            }
            Kind::GaussianBlur => {
                let mut planes = premultiply(src);
                let sigma = self.v(0);
                each_plane(&mut planes, |p| gaussian_blur(p, w, h, sigma));
                out = unpremultiply(&planes);
            }
            Kind::Sharpen => {
                let mut planes = premultiply(src);
                let (k, sigma) = (self.v(0), self.v(1));
                each_plane(&mut planes, |p| {
                    let mut blur = p.clone();
                    gaussian_blur(&mut blur, w, h, sigma);
                    // feComposite arithmetic: k2 = 1 + amount, k3 = -amount.
                    for (v, &b) in p.iter_mut().zip(&blur) {
                        *v = ((1.0 + k) * *v as f32 - k * b as f32 + 0.5).clamp(0.0, 65535.0) as u16;
                    }
                });
                clamp_to_alpha(&mut planes);
                out = unpremultiply(&planes);
            }
            Kind::Emboss => convolve(&mut out, src, w, h, [-2.0, -1.0, 0.0, -1.0, 1.0, 1.0, 0.0, 1.0, 2.0], 0.0, self.v(0)),
            Kind::EdgeDetect => convolve(&mut out, src, w, h, [-1.0, -1.0, -1.0, -1.0, 8.0, -1.0, -1.0, -1.0, -1.0], 0.0, self.v(0)),
            Kind::Dilate | Kind::Erode => {
                let mut planes = premultiply(src);
                let (r, dilate) = (self.v(0).round() as usize, self.kind == Kind::Dilate);
                each_plane(&mut planes, |p| morphology(p, w, h, r, dilate));
                out = unpremultiply(&planes);
            }
            Kind::DropShadow => out = drop_shadow(src, w, h, (self.v(0), self.v(1)), self.v(2), self.v(3), self.color),
        }
        out
    }
}

// ---- Transfer functions and color matrices (straight colors) ----

/// feComponentTransfer on R, G and B with the same function (0..1 in and
/// out), through a 256-entry table.
fn transfer(px: &mut [u8], f: impl Fn(f32) -> f32) {
    let lut: Vec<u8> = (0..256).map(|i| (f(i as f32 / 255.0).clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    for p in px.chunks_exact_mut(4) {
        for c in &mut p[..3] { *c = lut[*c as usize]; }
    }
}

type Matrix = [[f32; 3]; 3];

/// feColorMatrix on R, G, B (alpha unchanged), through a table per
/// matrix entry (fixed point, 1/4096).
fn color_matrix(px: &mut [u8], m: Matrix) {
    let lut: Vec<[i32; 256]> = m.iter().flatten().map(|&k| {
        let mut t = [0i32; 256];
        for (i, v) in t.iter_mut().enumerate() { *v = (k * i as f32 * 4096.0).round() as i32; }
        t
    }).collect();
    for p in px.chunks_exact_mut(4) {
        let c = [p[0] as usize, p[1] as usize, p[2] as usize];
        for k in 0..3 {
            let v = lut[k * 3][c[0]] + lut[k * 3 + 1][c[1]] + lut[k * 3 + 2][c[2]];
            p[k] = ((v + 2048) >> 12).clamp(0, 255) as u8;
        }
    }
}

/// The spec's saturate matrix.
fn saturate(s: f32) -> Matrix {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

/// The spec's hueRotate matrix (degrees).
fn hue_rotate(deg: f32) -> Matrix {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ]
}

/// The spec's grayscale() matrix.
fn grayscale(a: f32) -> Matrix {
    let k = 1.0 - a.clamp(0.0, 1.0);
    [
        [0.2126 + 0.7874 * k, 0.7152 - 0.7152 * k, 0.0722 - 0.0722 * k],
        [0.2126 - 0.2126 * k, 0.7152 + 0.2848 * k, 0.0722 - 0.0722 * k],
        [0.2126 - 0.2126 * k, 0.7152 - 0.7152 * k, 0.0722 + 0.9278 * k],
    ]
}

/// The spec's sepia() matrix.
fn sepia(a: f32) -> Matrix {
    let k = 1.0 - a.clamp(0.0, 1.0);
    [
        [0.393 + 0.607 * k, 0.769 - 0.769 * k, 0.189 - 0.189 * k],
        [0.349 - 0.349 * k, 0.686 + 0.314 * k, 0.168 - 0.168 * k],
        [0.272 - 0.272 * k, 0.534 - 0.534 * k, 0.131 + 0.869 * k],
    ]
}

/// feConvolveMatrix, 3x3, `preserveAlpha="true"` (the colors, straight,
/// are convolved; alpha stays), divisor 1; mixed with the source by
/// `strength`.
fn convolve(out: &mut [u8], src: &[u8], w: usize, h: usize, k: [f32; 9], bias: f32, strength: f32) {
    let at = |x: isize, y: isize, c: usize| {
        let (x, y) = (x.clamp(0, w as isize - 1) as usize, y.clamp(0, h as isize - 1) as usize);
        src[(y * w + x) * 4 + c] as f32
    };
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    s += kv * at(x as isize + (j % 3) as isize - 1, y as isize + (j / 3) as isize - 1, c);
                }
                let v = s + bias * 255.0;
                let src_v = src[i + c] as f32;
                out[i + c] = (src_v + (v - src_v) * strength).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

// ---- Premultiplied work: one plane per channel, 16 bits (65535 = 1.0) ----

type Planes = [Vec<u16>; 4];

fn premultiply(src: &[u8]) -> Planes {
    let n = src.len() / 4;
    let mut planes: Planes = Default::default();
    for p in &mut planes { p.reserve_exact(n); }
    for px in src.chunks_exact(4) {
        let a = px[3] as u32;
        for c in 0..3 { planes[c].push(((px[c] as u32 * a * 257 + 127) / 255) as u16); }
        planes[3].push((a * 257) as u16);
    }
    planes
}

fn unpremultiply(planes: &Planes) -> Vec<u8> {
    let n = planes[3].len();
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let a = planes[3][i] as f32;
        if a < 129.0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let k = 255.0 / a;
        for p in &planes[..3] { out.push((p[i] as f32 * k + 0.5).min(255.0) as u8); }
        out.push((a / 257.0 + 0.5) as u8);
    }
    out
}

/// `f` on the four planes, each on a thread of its own (a filter isn't a
/// brush stroke: its channels don't depend on each other).
fn each_plane(planes: &mut Planes, f: impl Fn(&mut Vec<u16>) + Sync) {
    std::thread::scope(|s| {
        for p in planes.iter_mut() {
            let f = &f;
            s.spawn(move || f(p));
        }
    });
}

/// Premultiplied colors can't exceed their alpha.
fn clamp_to_alpha(planes: &mut Planes) {
    let (colors, alpha) = planes.split_at_mut(3);
    for p in colors {
        for (c, &a) in p.iter_mut().zip(&alpha[0]) { *c = (*c).min(a); }
    }
}

fn blur_reach(sigma: f32) -> u32 { (3.0 * sigma).ceil() as u32 + 2 }

/// feGaussianBlur with standard deviation `sigma` px on one channel. From
/// 2 px, the spec's three box blurs: d = floor(σ × 3 × √(2π) / 4 + 0.5);
/// odd d: three boxes of d centered; even d: two of d (centered half a
/// pixel left, then right) and one of d + 1. Below 2 px, the Gaussian
/// itself. Rows, then columns (a whole row of column sums at a time:
/// memory read in order).
pub fn gaussian_blur(px: &mut [u16], w: usize, h: usize, sigma: f32) {
    if sigma <= 0.0 || w == 0 || h == 0 { return; }
    if sigma >= 2.0 {
        let d = (sigma * 3.0 * (2.0 * std::f32::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize;
        let boxes = if d % 2 == 1 {
            [(d / 2, d / 2); 3]
        } else {
            [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)]
        };
        let mut tmp = vec![0u16; w];
        for row in px.chunks_exact_mut(w) {
            for &(l, r) in &boxes { box_row(row, &mut tmp, l, r); }
        }
        let mut prev = vec![0u16; px.len()];
        for &(l, r) in &boxes { box_columns(px, &mut prev, w, h, l, r); }
        return;
    }
    let r = (3.0 * sigma).ceil() as usize;
    let k: Vec<f32> = (0..=2 * r).map(|i| { let x = i as f32 - r as f32; (-x * x / (2.0 * sigma * sigma)).exp() }).collect();
    let total: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / total).collect();
    let mut tmp = vec![0u16; w];
    for row in px.chunks_exact_mut(w) {
        tmp.copy_from_slice(row);
        for (i, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() { s += kv * tmp[(i + j).saturating_sub(r).min(w - 1)] as f32; }
            *o = (s + 0.5) as u16;
        }
    }
    let prev = px.to_vec();
    let mut acc = vec![0f32; w];
    for y in 0..h {
        acc.fill(0.0);
        for (j, kv) in k.iter().enumerate() {
            let sy = (y + j).saturating_sub(r).min(h - 1);
            for (a, &v) in acc.iter_mut().zip(&prev[sy * w..(sy + 1) * w]) { *a += kv * v as f32; }
        }
        for (o, a) in px[y * w..(y + 1) * w].iter_mut().zip(&acc) { *o = (a + 0.5) as u16; }
    }
}

/// One box blur along a row: the mean of the `l + 1 + r` values around
/// each (edges repeat).
fn box_row(row: &mut [u16], tmp: &mut [u16], l: usize, r: usize) {
    let w = row.len();
    tmp.copy_from_slice(row);
    let inv = 1.0 / (l + 1 + r) as f32;
    let at = |x: isize| tmp[x.clamp(0, w as isize - 1) as usize] as u32;
    let mut sum: u32 = (-(l as isize)..=r as isize).map(at).sum();
    for (i, o) in row.iter_mut().enumerate() {
        *o = (sum as f32 * inv + 0.5) as u16;
        sum = sum + at(i as isize + r as isize + 1) - at(i as isize - l as isize);
    }
}

/// One box blur down every column at once.
fn box_columns(px: &mut [u16], prev: &mut [u16], w: usize, h: usize, l: usize, r: usize) {
    prev.copy_from_slice(px);
    let inv = 1.0 / (l + 1 + r) as f32;
    let row = |y: isize| { let y = y.clamp(0, h as isize - 1) as usize; y * w..(y + 1) * w };
    let mut sum = vec![0u32; w];
    for j in -(l as isize)..=r as isize {
        for (s, &v) in sum.iter_mut().zip(&prev[row(j)]) { *s += v as u32; }
    }
    for y in 0..h {
        for (o, &s) in px[y * w..(y + 1) * w].iter_mut().zip(&sum) { *o = (s as f32 * inv + 0.5) as u16; }
        let (add, sub) = (row(y as isize + r as isize + 1), row(y as isize - l as isize));
        for ((s, &a), &b) in sum.iter_mut().zip(&prev[add]).zip(&prev[sub]) { *s = *s + a as u32 - b as u32; }
    }
}

/// feMorphology on one channel: each value becomes the max (dilate) or
/// min (erode) of the (2r + 1)² square around it. Along rows, then the
/// rows of the transposed plane (van Herk / Gil-Werman: three operations
/// per value whatever the radius).
fn morphology(px: &mut [u16], w: usize, h: usize, r: usize, dilate: bool) {
    if r == 0 || w == 0 || h == 0 { return; }
    let op = if dilate { u16::max } else { u16::min };
    let mut buf = Vec::new();
    for row in px.chunks_exact_mut(w) { extreme_line(row, r, op, &mut buf); }
    let mut t = transpose(px, w, h);
    for row in t.chunks_exact_mut(h) { extreme_line(row, r, op, &mut buf); }
    px.copy_from_slice(&transpose(&t, h, w));
}

fn transpose(src: &[u16], w: usize, h: usize) -> Vec<u16> {
    let mut out = vec![0u16; src.len()];
    const B: usize = 64;
    for by in (0..h).step_by(B) {
        for bx in (0..w).step_by(B) {
            for y in by..(by + B).min(h) {
                for x in bx..(bx + B).min(w) { out[x * h + y] = src[y * w + x]; }
            }
        }
    }
    out
}

/// Max (or min) over a window of `2r + 1` along a line, edges repeated:
/// running values from each block's start (g) and end (h); the window at i
/// is op(h[i - r], g[i + r]).
fn extreme_line(line: &mut [u16], r: usize, op: fn(u16, u16) -> u16, buf: &mut Vec<u16>) {
    let n = line.len();
    let k = 2 * r + 1;
    let padded = n + 2 * r;
    let len = padded.div_ceil(k) * k;
    buf.clear();
    buf.resize(len * 3, 0);
    let (v, rest) = buf.split_at_mut(len);
    let (g, hh) = rest.split_at_mut(len);
    for (i, x) in v.iter_mut().enumerate() { *x = line[i.saturating_sub(r).min(n - 1)]; }
    for b in (0..len).step_by(k) {
        g[b] = v[b];
        for i in b + 1..b + k { g[i] = op(g[i - 1], v[i]); }
        hh[b + k - 1] = v[b + k - 1];
        for i in (b..b + k - 1).rev() { hh[i] = op(hh[i + 1], v[i]); }
    }
    for (i, o) in line.iter_mut().enumerate() {
        // Window [i, i + 2r] in the padded line.
        *o = op(hh[i], g[i + 2 * r]);
    }
}

/// drop-shadow(): the source's alpha blurred, moved, flooded with `color`
/// at `opacity`, under the source (feMerge).
fn drop_shadow(src: &[u8], w: usize, h: usize, offset: (f32, f32), sigma: f32, opacity: f32, color: [u8; 3]) -> Vec<u8> {
    let mut alpha: Vec<u16> = src.chunks_exact(4).map(|p| p[3] as u16 * 257).collect();
    gaussian_blur(&mut alpha, w, h, sigma);
    let (dx, dy) = (offset.0.round() as isize, offset.1.round() as isize);
    let mut out = Vec::with_capacity(src.len());
    for y in 0..h as isize {
        for x in 0..w as isize {
            let (sx, sy) = (x - dx, y - dy);
            let sa = if sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize { alpha[(sy as usize) * w + sx as usize] as f32 / 65535.0 * opacity } else { 0.0 };
            let i = ((y as usize) * w + x as usize) * 4;
            let a = src[i + 3] as f32 / 255.0;
            // Source over shadow.
            let oa = a + sa * (1.0 - a);
            if oa <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            for k in 0..3 {
                let c = (src[i + k] as f32 * a + color[k] as f32 * sa * (1.0 - a)) / oa;
                out.push(c.round().clamp(0.0, 255.0) as u8);
            }
            out.push((oa * 255.0).round() as u8);
        }
    }
    out
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
    fn identity_settings_change_nothing() {
        let src = img(16, 8, |x, y| [(x * 15) as u8, (y * 30) as u8, 77, 255]);
        for f in [with(Kind::Brightness, &[1.0]), with(Kind::Contrast, &[1.0]), with(Kind::Saturate, &[1.0]),
                  with(Kind::HueRotate, &[0.0]), with(Kind::Grayscale, &[0.0]), with(Kind::Sepia, &[0.0]),
                  with(Kind::Invert, &[0.0]), with(Kind::Opacity, &[1.0]), with(Kind::Levels, &[0.0, 1.0, 1.0]),
                  with(Kind::GaussianBlur, &[0.0]), with(Kind::Sharpen, &[0.0, 2.0])] {
            let out = f.apply(&src, 16, 8);
            let worst = out.iter().zip(&src).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(worst <= 1, "{:?}: {worst}", f.kind);
        }
    }

    #[test]
    fn spec_values() {
        let px = [[200u8, 100, 50, 255]];
        let one = |f: Filter| f.apply(&px.concat(), 1, 1);
        // invert(1): 255 - c.
        assert_eq!(one(with(Kind::Invert, &[1.0]))[..3], [55, 155, 205]);
        // brightness(0.5): halved.
        assert_eq!(one(with(Kind::Brightness, &[0.5]))[..3], [100, 50, 25]);
        // grayscale(1): Rec. 709 luminance in every channel.
        let g = one(with(Kind::Grayscale, &[1.0]));
        let l = (0.2126f32 * 200.0 + 0.7152 * 100.0 + 0.0722 * 50.0).round() as u8;
        assert!(g[..3].iter().all(|&v| (v as i32 - l as i32).abs() <= 1), "{g:?} vs {l}");
        // saturate(0) is grayscale with the saturate matrix's weights.
        let s = one(with(Kind::Saturate, &[0.0]));
        assert!(s[0] == s[1] && s[1] == s[2]);
        // hue-rotate(360) is (nearly) the identity.
        let h = one(with(Kind::HueRotate, &[180.0]));
        let back = with(Kind::HueRotate, &[-180.0]).apply(&h, 1, 1);
        assert!(back[..3].iter().zip(&px[0][..3]).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 3), "{back:?}");
        // opacity(0.5).
        assert_eq!(one(with(Kind::Opacity, &[0.5]))[3], 128);
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
            // Spread: the corner of the square got lighter, outside got some.
            assert!(out[(27 * w + 32) * 4 + 3] > 0, "σ {sigma}: nothing spread");
            // Premultiplied: a half-covered pixel stays red, not dark.
            let p = &out[(26 * w + 32) * 4..][..4];
            if p[3] > 10 { assert!(p[0] > 240 && p[1] < 10, "σ {sigma}: {p:?}"); }
        }
    }

    #[test]
    fn dilate_and_erode_grow_and_shrink_a_line() {
        let (w, h) = (32, 32);
        let src = img(w, h, |_, y| if y == 16 { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
        let rows = |v: &[u8]| (0..h).filter(|&y| v[(y * w + 5) * 4 + 3] > 0).count();
        assert_eq!(rows(&with(Kind::Dilate, &[2.0]).apply(&src, w, h)), 5);
        let thick = img(w, h, |_, y| if (12..20).contains(&y) { [0, 0, 0, 255] } else { [0, 0, 0, 0] });
        assert_eq!(rows(&with(Kind::Erode, &[2.0]).apply(&thick, w, h)), 4);
    }

    #[test]
    fn threshold_and_posterize() {
        let src = img(4, 1, |x, _| { let v = [10u8, 100, 160, 250][x]; [v, v, v, 255] });
        let t = with(Kind::Threshold, &[0.5]).apply(&src, 4, 1);
        assert_eq!(t.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), [0, 0, 255, 255]);
        let p = with(Kind::Posterize, &[2.0]).apply(&src, 4, 1);
        assert!(p.chunks(4).all(|q| q[0] == 0 || q[0] == 255));
    }

    #[test]
    fn drop_shadow_lands_offset_under_the_shape() {
        let (w, h) = (32, 32);
        let src = img(w, h, |x, y| if (8..16).contains(&x) && (8..16).contains(&y) { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
        let f = Filter { kind: Kind::DropShadow, values: vec![6.0, 6.0, 0.0, 1.0], color: [0, 0, 255] };
        let out = f.apply(&src, w, h);
        let at = |x: usize, y: usize| &out[(y * w + x) * 4..][..4];
        assert_eq!(at(10, 10), [255, 255, 255, 255], "source on top");
        assert_eq!(at(19, 19), [0, 0, 255, 255], "shadow below right");
        assert_eq!(at(4, 4)[3], 0);
        assert_eq!(f.reach(), 2 + 6);
    }

    #[test]
    fn every_filter_runs_on_its_defaults() {
        let (w, h) = (24, 20);
        let src = img(w, h, |x, y| [(x * 10) as u8, (y * 12) as u8, 128, if x > 3 { 255 } else { 0 }]);
        for (_, kinds) in GROUPS {
            for &k in *kinds {
                let out = Filter::new(k).apply(&src, w, h);
                assert_eq!(out.len(), src.len(), "{k:?}");
            }
        }
    }
}


//! Filters on surfaces: a layer (inside a selection or not) and masks
//! (the selection's feather, grow and shrink). Only what can change is
//! worked: the stored tiles, grown by the filter's reach, or the
//! selection; one-color areas are mapped once without reading a pixel;
//! the far fill (the unbounded canvas) changes with the fill color.
//! Pieces of whole tiles go to the cores, with the reach read around.

use aurastudio_core::{Pixel, Rect, Rgba8};
use aurastudio_raster::{Image, Mask, Surface};
use rayon::prelude::*;

use crate::filter::Prepared;
use crate::{blur, extreme, Filter};

/// Pixels read at once by one batch of pieces (halos included): about
/// 256 MB of 8-bit RGBA at most in flight.
const BATCH_PX: u64 = 64 << 20;

/// The side of a piece: whole tiles, wide next to the halo read around.
fn piece_side(reach: u32) -> u32 { reach.saturating_mul(4).clamp(256, 4096) }

/// `b` over `a` by `m` (0: `a` exactly, 255: `b`), premultiplied.
#[inline]
pub fn mix<P: Pixel>(a: P, b: P, m: u8) -> P {
    match m {
        0 => a,
        255 => b,
        _ => {
            let (pa, pb, t) = (a.to_premul(), b.to_premul(), m as f32 / 255.0);
            P::from_premul(std::array::from_fn(|k| pa[k] + (pb[k] - pa[k]) * t))
        }
    }
}

/// A filter on a window of pixels (rows, width, height), edges repeated.
type RunFn<'a, P> = &'a (dyn Fn(&[P], usize, usize) -> Vec<P> + Sync);
/// What an area of one color becomes.
type UniformFn<'a, P> = &'a (dyn Fn(P) -> P + Sync);
/// Asked between pieces: true stops the work (a newer preview waits).
pub type StopFn<'a> = &'a (dyn Fn() -> bool + Sync);

/// The work was stopped before the end: the surface is half done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

/// What a piece became.
enum Out<P> {
    Uniform(P),
    Pixels(Vec<P>),
    /// Left as it is (its tiles stay shared).
    Same,
}

/// One filter over a surface: `run` on a window of pixels (edges
/// repeated), `uniform` on an area of one color, inside `sel`.
struct Job<'a, P: Pixel> {
    src:     &'a Surface<P>,
    sel:     Option<&'a Mask>,
    reach:   i32,
    run:     RunFn<'a, P>,
    uniform: UniformFn<'a, P>,
    stop:    StopFn<'a>,
    /// The fill is unchanged: a piece outside the selection is left alone.
    keep:    bool,
}

impl<P: Pixel> Job<'_, P> {
    fn piece(&self, c: Rect) -> Out<P> {
        let win = c.grown(self.reach);
        let m = self.sel.map(|s| s.uniform_in(c));
        // Outside the selection: as it was.
        if m == Some(Some(0)) {
            if self.keep { return Out::Same; }
            return match self.src.uniform_in(c) { Some(u) => Out::Uniform(u), None => Out::Pixels(self.src.read(c)) };
        }
        if let Some(u) = self.src.uniform_in(win) {
            let g = (self.uniform)(u);
            match m {
                None => return Out::Uniform(g),
                Some(Some(mv)) => return Out::Uniform(mix(u, g, mv)),
                Some(None) => {}
            }
        }
        let data = self.src.read(win);
        let res = (self.run)(&data, win.w() as usize, win.h() as usize);
        let (ww, cw) = (win.w() as usize, c.w() as usize);
        let (ox, oy) = ((c.x0 - win.x0) as usize, (c.y0 - win.y0) as usize);
        let at = |x: usize, y: usize| (oy + y) * ww + ox + x;
        let mut out = if self.reach == 0 { res } else {
            let mut v = Vec::with_capacity(cw * c.h() as usize);
            for y in 0..c.h() as usize { v.extend_from_slice(&res[at(0, y)..at(cw, y)]); }
            v
        };
        if let Some(s) = self.sel {
            let mk = s.read(c);
            for (i, (o, &m)) in out.iter_mut().zip(&mk).enumerate() { *o = mix(data[at(i % cw, i / cw)], *o, m); }
        }
        Out::Pixels(out)
    }

    /// Work `area` from `src` into `dst`, batch after batch of pieces.
    fn drive(&self, dst: &mut Surface<P>, area: Rect) -> Result<(), Stopped> {
        let pieces = self.src.chunks(area, piece_side(self.reach as u32));
        let win = |c: &Rect| c.grown(self.reach).area();
        let mut i = 0;
        while i < pieces.len() {
            let (mut j, mut px) = (i, 0u64);
            while j < pieces.len() && (j == i || px + win(&pieces[j]) <= BATCH_PX) { px += win(&pieces[j]); j += 1; }
            let outs: Option<Vec<Out<P>>> = pieces[i..j].par_iter().map(|c| (!(self.stop)()).then(|| self.piece(*c))).collect();
            let outs = outs.ok_or(Stopped)?;
            for (c, o) in pieces[i..j].iter().zip(outs) {
                match o { Out::Uniform(p) => dst.fill_rect(*c, p), Out::Pixels(v) => dst.write(*c, &v), Out::Same => {} }
            }
            i = j;
        }
        Ok(())
    }
}

/// Where a filter of `reach` changes `img` inside `sel` (stored pixels;
/// past them only the fill, changed as a whole).
fn area_of<P: Pixel>(img: &Surface<P>, sel: Option<&Mask>, reach: i32) -> Option<Rect> {
    let r = match sel {
        None => img.stored_bounds()?.grown(reach),
        // A selection hides the rest: its painted part only.
        Some(s) if s.fill() == 0 => s.pixel_bounds()?,
        Some(s) => {
            let a = img.stored_bounds().unwrap_or(Rect::EMPTY).union(&s.stored_bounds().unwrap_or(Rect::EMPTY));
            if a.is_empty() { return None; }
            a.grown(reach)
        }
    };
    (!r.is_empty()).then_some(r)
}

/// Run `job`'s filter over `img`: the far fill first (from the fill's own
/// result), then the area from a copy of the pixels before. Returns what
/// changed (the frame when the fill did).
fn apply_job<P: Pixel>(img: &mut Surface<P>, sel: Option<&Mask>, reach: u32, run: RunFn<'_, P>, uniform: UniformFn<'_, P>, stop: StopFn<'_>) -> Result<Option<Rect>, Stopped> {
    let reach = reach.min(i32::MAX as u32 / 4) as i32;
    let area = area_of(img, sel, reach);
    let fill = img.fill();
    let new_fill = match sel { None => uniform(fill), Some(s) => mix(fill, uniform(fill), s.fill()) };
    if area.is_none() && new_fill == fill { return Ok(None); }
    // Tiles are shared until written: the copy costs the tile table.
    let src = img.clone();
    if new_fill != fill { img.set_fill(new_fill); }
    if let Some(a) = area {
        let job = Job { src: &src, sel, reach, run, uniform, stop, keep: new_fill == fill };
        job.drive(img, a)?;
    }
    let frame = if new_fill != fill { img.rect() } else { Rect::EMPTY };
    let changed = area.unwrap_or(Rect::EMPTY).union(&frame);
    Ok((!changed.is_empty()).then_some(changed))
}

/// Never stops.
fn never() -> bool { false }

impl Filter {
    /// Where applying it to `img` within `sel` would change stored pixels
    /// (the fill past them may change too); `None`: nowhere.
    pub fn area(&self, img: &Image, sel: Option<&Mask>) -> Option<Rect> { area_of(img, sel, self.reach() as i32) }

    /// Apply it to a layer, inside `sel` (a selection's coverage mixes the
    /// result with the layer as it was). Returns the area that changed.
    pub fn apply_to(&self, img: &mut Image, sel: Option<&Mask>) -> Option<Rect> { self.prepare().apply_to(img, sel) }

    /// `apply_to`, asking `stop` between pieces.
    pub fn apply_until(&self, img: &mut Image, sel: Option<&Mask>, stop: StopFn<'_>) -> Result<Option<Rect>, Stopped> { self.prepare().apply_until(img, sel, stop) }
}

impl Prepared {
    /// `Filter::apply_to`, prepared once (a preview applied again and
    /// again).
    pub fn apply_to(&self, img: &mut Image, sel: Option<&Mask>) -> Option<Rect> { self.apply_until(img, sel, &never).ok().flatten() }

    /// `apply_to`, asking `stop` between pieces.
    pub fn apply_until(&self, img: &mut Image, sel: Option<&Mask>, stop: StopFn<'_>) -> Result<Option<Rect>, Stopped> {
        if self.is_identity() { return Ok(None); }
        aurastudio_core::zone!("filter layer");
        let run = |px: &[Rgba8], w: usize, h: usize| self.run(px, w, h);
        let uniform = |c: Rgba8| self.on_uniform(c);
        apply_job(img, sel, self.reach, &run, &uniform, stop)
    }
}

/// Gaussian blur of a mask (a selection's feather), edges of the stored
/// part spread into the fill around. Returns the area that changed.
pub fn blur_mask(mask: &mut Mask, sigma: f32) -> Option<Rect> {
    if sigma <= 0.0 { return None; }
    aurastudio_core::zone!("filter blur mask");
    let run = |px: &[u8], w: usize, h: usize| blur::blur_u8(px, w, h, sigma);
    apply_job(mask, None, blur::reach(sigma), &run, &|c| c, &never).ok().flatten()
}

/// Grow (max) or shrink (min) a mask by `radius` px, square or round (a
/// selection's grow / shrink; Thicken / Thin on one channel).
pub fn spread_mask(mask: &mut Mask, radius: f32, round: bool, grow: bool) -> Option<Rect> {
    if radius < 0.5 { return None; }
    aurastudio_core::zone!("filter spread mask");
    let op: fn(u8, u8) -> u8 = if grow { u8::max } else { u8::min };
    let run = |px: &[u8], w: usize, h: usize| {
        let mut v = px.to_vec();
        if round { extreme::disc(&mut v, w, h, radius, op) } else { extreme::square(&mut v, w, h, radius.round() as usize, op) }
        v
    };
    apply_job(mask, None, radius.ceil() as u32, &run, &|c| c, &never).ok().flatten()
}

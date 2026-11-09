//! The engine as the paint thread drives it: pen samples in, the layer's
//! pixels painted through `core::PaintSurface`, one stroke at a time.
//! Normal brushes go through the stroke buffer (`stroke.rs`); a Mixer Brush
//! paints dab by dab, each one picking up what is on the layer now.

use aurastudio_core::math::to_u8;
use aurastudio_core::{PaintSurface, Rect, Rgba8, Span};

use crate::brush::BrushSettings;
use crate::dynamics::DabGenerator;
use crate::mixer::{MixerState, PICKUP_RATE};
use crate::render::BrushContext;
use crate::render::parallel;
use crate::stroke::{CoverageMap, StrokeRenderer};
use crate::{Dab, StrokePoint};

/// One pen position: canvas pixels, pressure 0..1, time in seconds (the
/// input clock), tilt in degrees −90..90 toward +x / +y, barrel rotation
/// in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub x:        f32,
    pub y:        f32,
    pub pressure: f32,
    pub time:     f64,
    pub tilt:     (f32, f32),
    pub rotation: f32,
}

impl PenSample {
    pub fn new(x: f32, y: f32, pressure: f32, time: f64) -> Self {
        Self { x, y, pressure: pressure.clamp(0.0, 1.0), time, tilt: (0.0, 0.0), rotation: 0.0 }
    }

    /// With the pen's tilt and barrel rotation.
    pub fn with_pen(self, tilt: (f32, f32), rotation: f32) -> Self { Self { tilt, rotation, ..self } }

    fn point(self) -> StrokePoint {
        StrokePoint {
            x: self.x as f64,
            y: self.y as f64,
            pressure: self.pressure,
            tilt_x: self.tilt.0,
            tilt_y: self.tilt.1,
            rotation: self.rotation,
            wheel: 1.0,
            time: self.time * 1000.0,
        }
    }
}

/// A Mixer Brush stroke: its dabs, painted one by one (each on all cores
/// when big); the 2nd tip's coverage kept for the whole stroke.
struct MixerRun {
    ctx:       BrushContext,
    generator: DabGenerator,
    dabs:      Vec<Dab>,
    duals:     Vec<Dab>,
    cov:       Vec<f32>,
    dual:      Option<CoverageMap>,
}

/// What a Mixer Brush that samples all layers sees around the layer it
/// paints, for a rectangle: the merge of what is under that layer and of
/// what is over it (transparent where nothing is), straight RGBA rows.
pub type Sampler = std::sync::Arc<dyn Fn(Rect) -> (Vec<Rgba8>, Vec<Rgba8>) + Send + Sync>;

/// The brush engine for one brush, stroke after stroke.
pub struct Forge {
    brush:   BrushSettings,
    stroke:  Option<StrokeRenderer>,
    mixer:   Option<MixerRun>,
    /// Paint carried between Mixer Brush strokes.
    state:   MixerState,
    /// "Sample all layers": the other layers, seen by the pickup.
    sampler: Option<Sampler>,
    /// The view's zoom (smoothing measures its string on screen).
    zoom:    f32,
    /// Strokes painted: each one's jitter seed.
    strokes: u64,
}

impl Forge {
    pub fn new(brush: BrushSettings) -> Self {
        Self { brush, stroke: None, mixer: None, state: MixerState::default(), sampler: None, zoom: 1.0, strokes: 0 }
    }

    pub fn brush(&self) -> &BrushSettings { &self.brush }

    /// Bytes the stroke under way holds (its coverage tiles).
    pub fn stroke_bytes(&self) -> usize { self.stroke.as_ref().map_or(0, StrokeRenderer::bytes) }


    /// The view's zoom for the next stroke.
    pub fn set_zoom(&mut self, zoom: f32) { if zoom.is_finite() && zoom > 0.0 { self.zoom = zoom; } }

    /// A Mixer Brush's paint (Load / Clean, the swatch).
    pub fn mixer_state(&mut self) -> &mut MixerState { &mut self.state }

    /// With the paint a Mixer Brush carried from the strokes before.
    pub fn with_mixer_state(self, state: MixerState) -> Self { Self { state, ..self } }

    /// The paint it carries now (for the next brush).
    pub fn take_mixer_state(&mut self) -> MixerState { std::mem::take(&mut self.state) }

    /// The other layers the pickup sees ("Sample all layers"), for the
    /// stroke under way.
    pub fn set_sampler(&mut self, s: Option<Sampler>) { self.sampler = s; }

    /// The brush for this stroke: its own seed when it has none (a
    /// stroke's jitter differs from the last one's, a replay's doesn't).
    fn stroke_brush(&mut self) -> BrushSettings {
        self.strokes += 1;
        let mut b = self.brush.clone();
        b.seed = crate::rng::stroke_seed(b.seed, self.strokes);
        b
    }

    /// Paint from the last sample to `p` (the first one starts the
    /// stroke). Returns the area painted.
    pub fn stroke_to<S: PaintSurface<Rgba8>>(&mut self, s: &mut S, p: PenSample) -> Option<Rect> { self.stroke_through(s, &[p]) }

    /// Paint through every sample of `ps` at once: their dabs as one by
    /// one, the layer painted once (samples that waited are caught up in
    /// one go). Returns the area painted.
    pub fn stroke_through<S: PaintSurface<Rgba8>>(&mut self, s: &mut S, ps: &[PenSample]) -> Option<Rect> {
        if ps.is_empty() { return None; }
        let pts: Vec<StrokePoint> = ps.iter().map(|p| p.point()).collect();
        if self.brush.mixer.enabled && !self.brush.erase {
            if self.mixer.is_none() {
                let b = self.stroke_brush();
                self.state.begin_stroke(b.color);
                let dual = b.dual_brush.enabled.then(|| CoverageMap::new(0));
                self.mixer = Some(MixerRun { ctx: BrushContext::new(&b), generator: DabGenerator::new(&b, self.zoom), dabs: Vec::new(), duals: Vec::new(), cov: Vec::new(), dual });
            }
            let run = self.mixer.as_mut()?;
            run.generator.push(&pts, &mut run.dabs, &mut run.duals);
            let r = run.paint(s, &mut self.state, self.sampler.as_ref());
            return (!r.is_empty()).then_some(r);
        }
        if self.stroke.is_none() {
            let b = self.stroke_brush();
            self.stroke = Some(StrokeRenderer::new(&b, self.zoom));
        }
        let st = self.stroke.as_mut()?;
        st.push(&pts);
        let r = st.paint(s, false);
        (!r.is_empty()).then_some(r)
    }

    /// The stroke is over: what smoothing still owed is painted, the next
    /// stroke starts afresh.
    pub fn end_stroke<S: PaintSurface<Rgba8>>(&mut self, s: &mut S) -> Option<Rect> {
        if let Some(mut run) = self.mixer.take() {
            run.generator.finish(&mut run.dabs, &mut run.duals);
            let r = run.paint(s, &mut self.state, self.sampler.as_ref());
            self.sampler = None;
            let m = &self.brush.mixer;
            self.state.end_stroke(self.brush.color, m.clean_after_stroke, m.load_after_stroke);
            return (!r.is_empty()).then_some(r);
        }
        let mut st = self.stroke.take()?;
        st.finish();
        let r = st.paint(s, false);
        (!r.is_empty()).then_some(r)
    }

}

impl MixerRun {
    /// The dabs waiting, painted one after the other (the 2nd tip's first).
    fn paint<S: PaintSurface<Rgba8>>(&mut self, s: &mut S, state: &mut MixerState, around: Option<&Sampler>) -> Rect {
        let mut done = Rect::EMPTY;
        let dabs = std::mem::take(&mut self.dabs);
        let duals = std::mem::take(&mut self.duals);
        if let Some(dm) = self.dual.as_mut() {
            for d in &duals {
                let rect = self.ctx.dab_rect(d, true);
                self.ctx.rasterize(d, true, rect, &mut self.cov);
                dm.accumulate(rect, &self.cov, 1.0, None, None);
            }
        }
        for d in &dabs {
            let rect = self.ctx.dab_rect(d, false);
            if rect.is_empty() { continue; }
            self.ctx.rasterize(d, false, rect, &mut self.cov);
            mixer_dab(s, &self.cov, rect, d, &self.ctx, self.dual.as_ref(), state, around);
            done = done.union(&rect);
        }
        (self.dabs, self.duals) = (dabs, duals);
        self.dabs.clear();
        self.duals.clear();
        done
    }
}

/// Every run of `rect`, in order, one by one through `span_mut`.
/// (Only inside the surface's frame: an empty run ends its row.)
pub(crate) fn for_runs<S: PaintSurface<Rgba8>>(s: &mut S, rect: Rect, mut f: impl FnMut(&mut Span<'_, Rgba8>)) {
    let rect = rect.intersect(&s.frame());
    for y in rect.y0..rect.y1 {
        let mut x = rect.x0;
        while x < rect.x1 {
            let mut sp = s.span_mut(x, y, (rect.x1 - x) as usize);
            let n = sp.len();
            if n == 0 { break; }
            f(&mut sp);
            x += n as i32;
        }
    }
}

/// What a Mixer dab reads of the layer under it: colour weighted by
/// coverage and alpha, the coverage, the alpha.
#[derive(Clone, Copy, Default)]
struct Sampled { acc: [f32; 3], wsum: f32, asum: f32 }

impl Sampled {
    fn add(self, o: Sampled) -> Sampled { Sampled { acc: std::array::from_fn(|c| self.acc[c] + o.acc[c]), wsum: self.wsum + o.wsum, asum: self.asum + o.asum } }
}

/// One Mixer Brush dab (`cov` over `rect`): the canvas under it moves the
/// pickup well toward its color by Wet, then the reservoir and the pickup,
/// mixed by Mix, land at Flow, times the dab's opacity (pen, Transfer)
/// and the stroke's texture and 2nd tip. PhotoCraft's model (see
/// NOTICE.md) on the layer's runs; a selection scales the paint by its
/// limit. A big dab reads and paints on all cores. With `around`
/// ("Sample all layers") it reads the layer between what is under it and
/// what is over it, as the canvas shows it (normal blending).
#[allow(clippy::too_many_arguments)]
fn mixer_dab<S: PaintSurface<Rgba8>>(s: &mut S, cov: &[f32], rect: Rect, d: &Dab, ctx: &BrushContext, dual: Option<&CoverageMap>, state: &mut MixerState, around: Option<&Sampler>) {
    let b = &ctx.brush;
    let m = &b.mixer;
    let w = rect.w() as usize;
    let at = |x: i32, y: i32| cov[(y - rect.y0) as usize * w + (x - rect.x0) as usize];
    let (wet, mix) = (m.effective_wet(d), m.mix.clamp(0.0, 1.0) * d.mix.clamp(0.0, 1.0));
    let (flow, load) = (m.flow.clamp(0.0, 1.0), m.load.clamp(0.0, 1.0));
    let strength = (d.opacity * b.opacity).clamp(0.0, 1.0);
    let masked = dual.is_some() || ctx.stroke_texture();
    let res = state.reservoir;
    let mut amount = if res.is_some() { state.amount.clamp(0.0, 1.0) } else { 0.0 };
    let mut pickup = state.pickup;
    // The other layers, read once for the dab (only when it picks up).
    let others = around.filter(|_| wet > 0.0).map(|f| f(rect));
    let others = others.as_ref().filter(|(u, o)| u.len() == cov.len() && o.len() == cov.len());
    // What a run reads, and how much paint lands on a pixel.
    let sample = |sp: &Span<'_, Rgba8>| {
        let mut o = Sampled::default();
        for i in 0..sp.len() {
            let (x, y) = (sp.x + i as i32, sp.y);
            let k = at(x, y);
            if k <= 0.0 { continue; }
            let p = match others {
                Some((under, over)) => {
                    let j = (y - rect.y0) as usize * w + (x - rect.x0) as usize;
                    over_straight(over[j], over_straight(sp.pixels[i], under[j]))
                }
                None => sp.pixels[i],
            };
            let a = p[3] as f32 / 255.0;
            for (c, v) in o.acc.iter_mut().enumerate() { *v += p[c] as f32 / 255.0 * k * a; }
            o.wsum += k;
            o.asum += k * a;
        }
        o
    };
    let amount_at = |x: i32, y: i32, dep: f32| {
        let mut k = at(x, y);
        if k <= 0.0 { return 0.0; }
        if masked { k = ctx.stroke_mask_wet(k, None, dual.map(|m| m.value(x, y)), x, y); }
        k * dep * strength
    };
    let paint_run = |sp: &mut Span<'_, Rgba8>, paint8: [u8; 3], dep: f32| {
        for i in 0..sp.len() {
            let mut k = amount_at(sp.x + i as i32, sp.y, dep);
            if let Some(l) = sp.limit { k *= l[i] as f32 / 255.0; }
            let k8 = to_u8(k);
            if k8 == 0 { continue; }
            let px = sp.pixels[i];
            // (The paint builds on what is there, not on the base.)
            sp.pixels[i] = aurastudio_core::blend::paint_pixel_u8(px, paint8, k8, aurastudio_core::BlendMode::Normal);
        }
    };
    // The paint from what was read: the colour, how much of it lands.
    let mut mixed = |got: Sampled| -> Option<([u8; 3], f32)> {
        if wet > 0.0 && got.asum > 1e-6 {
            let ca = got.asum / got.wsum.max(1e-6);
            let canvas = [got.acc[0] / got.asum, got.acc[1] / got.asum, got.acc[2] / got.asum, 1.0];
            pickup = Some(match pickup {
                None => canvas,
                Some(p) => std::array::from_fn(|c| p[c] + (canvas[c] - p[c]) * PICKUP_RATE * wet * ca),
            });
        }
        let wr = if res.is_some() { amount * (1.0 - mix * wet) } else { 0.0 };
        let wp = if pickup.is_some() { mix * wet } else { 0.0 };
        let total = wr + wp;
        amount *= 1.0 - 0.08 * (1.0 - load);
        if total <= 1e-6 { return None; }
        let r = res.unwrap_or([0.0; 4]);
        let p = pickup.unwrap_or([0.0; 4]);
        let paint: [f32; 3] = std::array::from_fn(|c| (r[c] * wr + p[c] * wp) / total);
        Some((paint.map(to_u8), total.min(1.0) * flow))
    };
    let mut v: Vec<Span<'_, Rgba8>> = Vec::new();
    if parallel(rect.area() as usize) && s.spans_mut(rect, &mut v) {
        use rayon::prelude::*;
        let got = if wet > 0.0 { v.par_iter().map(&sample).reduce(Sampled::default, Sampled::add) } else { Sampled::default() };
        if let Some((paint8, dep)) = mixed(got) { v.par_iter_mut().with_min_len(8).for_each(|sp| paint_run(sp, paint8, dep)); }
    } else {
        drop(v);
        s.prepare(rect);
        let mut got = Sampled::default();
        if wet > 0.0 { for_runs(s, rect, |sp| got = got.add(sample(sp))); }
        if let Some((paint8, dep)) = mixed(got) { for_runs(s, rect, |sp| paint_run(sp, paint8, dep)); }
    }
    state.amount = amount;
    state.pickup = pickup;
}

/// `top` over `bottom`, straight RGBA.
fn over_straight(top: Rgba8, bottom: Rgba8) -> Rgba8 {
    let (ta, ba) = (top[3] as f32 / 255.0, bottom[3] as f32 / 255.0);
    if ta >= 1.0 { return top; }
    if ta <= 0.0 { return bottom; }
    let a = ta + ba * (1.0 - ta);
    if a <= 0.0 { return [0; 4]; }
    let c = |i: usize| (top[i] as f32 * ta + bottom[i] as f32 * ba * (1.0 - ta)) / a;
    [c(0).round() as u8, c(1).round() as u8, c(2).round() as u8, (a * 255.0).round() as u8]
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_core::FlatSurface;

    fn brush(size: f32) -> BrushSettings { BrushSettings { size, pressure_size: false, color: [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 1.0], ..Default::default() } }

    fn line(f: &mut Forge, s: &mut FlatSurface, from: (f32, f32), to: (f32, f32)) {
        f.stroke_to(s, PenSample::new(from.0, from.1, 1.0, 0.0));
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            f.stroke_to(s, PenSample::new(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t, 1.0, t as f64 * 0.2));
        }
        f.end_stroke(s);
    }

    /// The samples of a stroke one by one or all at once paint the same
    /// pixels: with a 2nd tip (whose dabs change tiles painted before)
    /// and with a Mixer brush.
    #[test]
    fn samples_one_by_one_or_together_paint_the_same() {
        let dual = crate::brush::DualBrush { enabled: true, size: 70.0, spacing: 1.0, scatter: 0.0, count: 1, ..Default::default() };
        let brushes = [
            BrushSettings { dual_brush: dual, spacing: 0.1, ..brush(8.0) },
            BrushSettings { mixer: crate::mixer::MixerSettings { enabled: true, ..Default::default() }, ..brush(12.0) },
        ];
        for b in brushes {
            let pts: Vec<PenSample> = (0..=90).map(|i| PenSample::new(10.0 + i as f32 * 4.0, 30.0 + (i as f32 * 0.3).sin() * 4.0, 0.8, i as f64 * 0.004)).collect();
            let mut one = FlatSurface::new(400, 60, [200, 100, 50, 255]);
            let mut f = Forge::new(b.clone());
            for p in &pts { f.stroke_to(&mut one, *p); }
            f.end_stroke(&mut one);
            let mut all = FlatSurface::new(400, 60, [200, 100, 50, 255]);
            let mut f = Forge::new(b.clone());
            f.stroke_through(&mut all, &pts);
            f.end_stroke(&mut all);
            let diff = one.pixels.iter().zip(&all.pixels).map(|(a, b)| a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0)).max().unwrap_or(0);
            assert!(diff <= 1, "mixer {}: differs by {diff}", b.mixer.enabled);
        }
    }

    #[test]
    fn a_stroke_paints_without_building_up() {
        let mut s = FlatSurface::new(100, 40, [255, 255, 255, 0]);
        let mut f = Forge::new(BrushSettings { opacity: 0.5, ..brush(10.0) });
        line(&mut f, &mut s, (10.0, 20.0), (90.0, 20.0));
        let p = s.pixel(50, 20);
        assert_eq!(&p[..3], &[10, 20, 30]);
        assert!((p[3] as i32 - 128).abs() <= 1, "{p:?}");
        assert_eq!(s.pixel(50, 35)[3], 0);
    }

    #[test]
    fn the_eraser_takes_alpha_away() {
        let mut s = FlatSurface::new(30, 30, [9, 9, 9, 255]);
        let mut f = Forge::new(BrushSettings { erase: true, ..brush(8.0) });
        line(&mut f, &mut s, (15.0, 15.0), (15.5, 15.0));
        assert_eq!(s.pixel(15, 15)[3], 0);
        assert_eq!(s.pixel(1, 1)[3], 255);
    }

    #[test]
    fn a_selection_limits_the_paint() {
        let mut s = FlatSurface::new(60, 20, [255, 255, 255, 0]);
        let mut lim = vec![0u8; 60 * 20];
        for y in 0..20 { for x in 0..30 { lim[y * 60 + x] = 255; } }
        s.limit = Some(lim);
        let mut f = Forge::new(brush(8.0));
        line(&mut f, &mut s, (5.0, 10.0), (55.0, 10.0));
        assert_eq!(s.pixel(20, 10)[3], 255);
        assert_eq!(s.pixel(45, 10)[3], 0);
    }

    #[test]
    fn short_runs_paint_the_same_as_whole_rows() {
        let mut a = FlatSurface::new(80, 40, [255, 255, 255, 255]);
        let mut b = FlatSurface::new(80, 40, [255, 255, 255, 255]);
        b.run = 7;
        let soft = BrushSettings { hardness: 0.0, flow: 0.3, seed: 7, ..brush(20.0) };
        line(&mut Forge::new(soft.clone()), &mut a, (10.0, 20.0), (70.0, 20.0));
        line(&mut Forge::new(soft), &mut b, (10.0, 20.0), (70.0, 20.0));
        assert_eq!(a.pixels, b.pixels);
    }

    #[test]
    fn a_soft_low_flow_stroke_builds_up_gradually() {
        let mut s = FlatSurface::new(100, 40, [255, 255, 255, 0]);
        let mut f = Forge::new(BrushSettings { hardness: 0.0, flow: 0.1, ..brush(24.0) });
        line(&mut f, &mut s, (10.0, 20.0), (90.0, 20.0));
        let (mid, edge) = (s.pixel(50, 20)[3], s.pixel(50, 29)[3]);
        assert!(mid > 100 && edge < mid, "{mid} {edge}");
    }

    #[test]
    fn a_mixer_brush_smears_what_is_there() {
        let mut s = FlatSurface::new(80, 30, [255, 255, 255, 255]);
        for y in 0..30 { for x in 0..20 { s.pixels[y * 80 + x] = [200, 0, 0, 255]; } }
        s.next_stroke();
        let mut b = brush(12.0);
        b.mixer.enabled = true;
        b.mixer.wet = 1.0;
        b.mixer.mix = 1.0;
        b.mixer.load = 0.0;
        let mut f = Forge::new(b);
        let c = f.brush().color;
        f.mixer_state().clean(c);
        line(&mut f, &mut s, (10.0, 15.0), (60.0, 15.0));
        let p = s.pixel(30, 15);
        assert!(p[0] > p[1] + 40, "red dragged along: {p:?}");
    }

    /// "Sample all layers": an empty layer over red picks the red up; on
    /// its own it has nothing to pick.
    #[test]
    fn sampling_all_layers_picks_up_the_layers_under() {
        let paint = |all: bool| {
            let mut s = FlatSurface::new(80, 30, [255, 255, 255, 0]);
            let mut b = brush(12.0);
            b.mixer = crate::MixerSettings { enabled: true, wet: 1.0, mix: 1.0, load: 0.0, sample_all_layers: all, ..Default::default() };
            let mut f = Forge::new(b);
            let c = f.brush().color;
            f.mixer_state().clean(c);
            if all {
                // Red under the layer at the left, nothing over it.
                let red: Sampler = std::sync::Arc::new(|r: Rect| {
                    let under = (r.y0..r.y1).flat_map(|_| (r.x0..r.x1).map(|x| if x < 20 { [200, 0, 0, 255] } else { [255; 4] })).collect();
                    (under, vec![[0; 4]; r.area() as usize])
                });
                f.set_sampler(Some(red));
            }
            line(&mut f, &mut s, (10.0, 15.0), (60.0, 15.0));
            f.end_stroke(&mut s);
            s.pixel(30, 15)
        };
        let p = paint(true);
        assert!(p[3] > 0 && p[0] > p[1] + 40, "red picked up from under: {p:?}");
        assert_eq!(paint(false)[3], 0, "nothing on the layer to pick");
    }

    #[test]
    fn imported_style_sampled_tips_paint() {
        let tip = crate::GrayTile::from_fn(16, 16, |x, y| if (x as i32 - 8).abs() < 4 && (y as i32 - 8).abs() < 4 { 1.0 } else { 0.0 });
        let mut s = FlatSurface::new(80, 40, [255, 255, 255, 0]);
        let mut f = Forge::new(BrushSettings { tip: crate::TipShape::Sampled(tip), spacing: 0.5, ..brush(16.0) });
        line(&mut f, &mut s, (10.0, 20.0), (70.0, 20.0));
        assert!(s.pixel(40, 20)[3] > 200);
        assert_eq!(s.pixel(40, 35)[3], 0);
    }
}

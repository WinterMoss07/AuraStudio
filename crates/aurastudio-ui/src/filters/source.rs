//! What a filter dialog works on: the active layer's pixels where the
//! filter applies (as the paint thread's session takes them), with its
//! histogram and a reduced copy for the dialog's preview at fit; and the
//! thread that filters the preview's crops.

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};

use aurastudio_filters::{Filter, Histogram};
use aurastudio_layers::{LayerStack};
use aurastudio_selection::{Selection};

/// The smallest reduced copy's longest side, about.
const SMALLEST_SIDE: usize = 128;

/// A level of the source: the pixels, straight RGBA, and the selected
/// ones (None: all).
pub struct Level {
    pub w:    usize,
    pub h:    usize,
    pub px:   Vec<u8>,
    pub mask: Option<Vec<bool>>,
}

pub struct Source {
    /// The painted area or the selection inside it (local pixels), what
    /// the preview frames at first.
    pub area:      (u32, u32, u32, u32),
    pub full:      Level,
    /// `full` reduced 2, 4, 8… times, down to about 128 px: the preview
    /// filters the smallest one that still has a pixel per screen pixel
    /// (a thumbnail when zoomed out: fast).
    pub reduced:   Vec<Level>,
    pub hist:      Histogram,
    pub selection: bool,
    /// The layer's empty parts are transparent (line art) rather than a
    /// color (the paper).
    pub transparent: bool,
}

impl Source {
    /// The active layer's source, as the paint thread will see it. None:
    /// the layer can't be painted, or nothing is painted on it.
    pub fn capture(layers: &LayerStack, sel: Option<&Selection>) -> Option<Source> {
        let (id, rect) = layers.filter_area(sel, aurastudio_filters::MARGIN)?;
        let (_, inner) = layers.filter_area(sel, 0)?;
        if rect.2 <= rect.0 || rect.3 <= rect.1 { return None; }
        let px = layers.layer_rect(id, rect)?;
        let (w, h) = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
        let mask = sel.map(|s| {
            let mut m = Vec::with_capacity(w * h);
            for y in rect.1..rect.3 { for x in rect.0..rect.2 { m.push(s.contains(x, y)); } }
            m
        });
        let hist = match &mask { Some(m) => Histogram::of(&px, |i| m[i]), None => Histogram::of(&px, |_| true) };
        let transparent = matches!(layers.node(id), Some(aurastudio_layers::Node::Layer(l)) if l.tiles.fill()[3] == 0);
        let full = Level { w, h, px, mask };
        let reduced = pyramid(&full);
        let area = (inner.0 - rect.0, inner.1 - rect.1, inner.2 - rect.0, inner.3 - rect.1);
        Some(Source { area, full, reduced, hist, selection: sel.is_some(), transparent })
    }

    /// What filter layer `id` filters: the merge of everything under it
    /// (its dialog shows it before and after). None if the canvas is empty.
    pub fn below(layers: &LayerStack, id: u64) -> Option<Source> {
        let (w, h) = (layers.width, layers.height);
        if w == 0 || h == 0 { return None; }
        let pick = aurastudio_layers::Pick { until: id, ..aurastudio_layers::Pick::ALL };
        let px = layers.composite_pick((0, 0, w, h), pick, false);
        let hist = Histogram::of(&px, |_| true);
        let full = Level { w: w as usize, h: h as usize, px, mask: None };
        let reduced = pyramid(&full);
        Some(Source { area: (0, 0, w, h), full, reduced, hist, selection: false, transparent: false })
    }

    /// The level reduced `k` times (a power of two; 1: full size).
    pub fn level(&self, k: usize) -> &Level {
        match k.trailing_zeros() as usize { 0 => &self.full, n => &self.reduced[(n - 1).min(self.reduced.len() - 1)] }
    }

    /// The largest level no longer than `side` on a side (or the
    /// smallest).
    pub fn level_under(&self, side: usize) -> &Level {
        std::iter::once(&self.full).chain(&self.reduced).find(|l| l.w.max(l.h) <= side).unwrap_or(self.reduced.last().unwrap_or(&self.full))
    }

    /// The most a level is reduced.
    pub fn max_k(&self) -> usize { 1 << self.reduced.len() }

    /// The smallest level with at least one pixel per screen pixel at
    /// `s` points per layer pixel (`ppp` pixels per point): its factor.
    pub fn k_for(&self, s: f32, ppp: f32) -> usize {
        let mut k = 1;
        while k < self.max_k() && s * (k * 2) as f32 * ppp <= 1.0 { k *= 2; }
        k
    }

    /// How many pixels the filter changes (painted, selected).
    pub fn painted(&self) -> u64 { self.hist.count }
}

/// `full` reduced 2, 4, 8… times, down to about SMALLEST_SIDE.
fn pyramid(full: &Level) -> Vec<Level> {
    let mut reduced: Vec<Level> = Vec::new();
    loop {
        let last = reduced.last().unwrap_or(full);
        if last.w.max(last.h) <= SMALLEST_SIDE * 2 { return reduced; }
        let next = reduce(last, 2);
        reduced.push(next);
    }
}

/// `l` reduced `k` times: each pixel the average of k × k, colors weighted
/// by their alpha.
fn reduce(l: &Level, k: usize) -> Level {
    let (w, h) = (l.w.div_ceil(k), l.h.div_ceil(k));
    let mut px = vec![0u8; w * h * 4];
    let mut mask = l.mask.as_ref().map(|_| vec![false; w * h]);
    for oy in 0..h {
        for ox in 0..w {
            let (mut s, mut n, mut sel) = ([0u64; 4], 0u64, 0u64);
            for y in oy * k..((oy + 1) * k).min(l.h) {
                for x in ox * k..((ox + 1) * k).min(l.w) {
                    let i = y * l.w + x;
                    let p = &l.px[i * 4..i * 4 + 4];
                    let a = p[3] as u64;
                    for c in 0..3 { s[c] += p[c] as u64 * a; }
                    s[3] += a;
                    n += 1;
                    if l.mask.as_ref().is_some_and(|m| m[i]) { sel += 1; }
                }
            }
            let o = &mut px[(oy * w + ox) * 4..][..4];
            if let Some(a) = std::num::NonZeroU64::new(s[3]) {
                for c in 0..3 { o[c] = ((s[c] + a.get() / 2) / a) as u8; }
            } else {
                // Fully transparent: the first pixel's (empty) color.
                let i = (oy * k * l.w + ox * k) * 4;
                o[..3].copy_from_slice(&l.px[i..i + 3]);
            }
            o[3] = ((s[3] + n / 2) / n.max(1)) as u8;
            if let Some(m) = &mut mask { m[oy * w + ox] = sel * 2 >= n; }
        }
    }
    Level { w, h, px, mask }
}

/// A part of a level to filter: (x0, y0, x1, y1) with the margin the
/// filter reads around it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Crop {
    pub k:     usize,
    /// What is shown.
    pub shown: (usize, usize, usize, usize),
    /// What is read (shown + the filter's reach, inside the level).
    pub read:  (usize, usize, usize, usize),
}

impl Crop {
    pub fn new(level: &Level, k: usize, shown: (usize, usize, usize, usize), reach: usize) -> Crop {
        let read = (shown.0.saturating_sub(reach), shown.1.saturating_sub(reach), (shown.2 + reach).min(level.w), (shown.3 + reach).min(level.h));
        Crop { k, shown, read }
    }

    pub fn shown_size(&self) -> (usize, usize) { (self.shown.2 - self.shown.0, self.shown.3 - self.shown.1) }
}

/// Pixels of `r` in `l` (and its mask).
pub fn cut(l: &Level, r: (usize, usize, usize, usize)) -> (Vec<u8>, Option<Vec<bool>>) {
    let (w, h) = (r.2 - r.0, r.3 - r.1);
    let mut px = Vec::with_capacity(w * h * 4);
    for y in r.1..r.3 { px.extend_from_slice(&l.px[(y * l.w + r.0) * 4..(y * l.w + r.2) * 4]); }
    let mask = l.mask.as_ref().map(|m| {
        let mut o = Vec::with_capacity(w * h);
        for y in r.1..r.3 { o.extend_from_slice(&m[y * l.w + r.0..y * l.w + r.2]); }
        o
    });
    (px, mask)
}

/// The filter's result as the canvas will show it: outside the selection
/// and where the layer stays transparent, the pixels from before.
pub fn keep_outside(before: &[u8], after: &mut [u8], mask: Option<&[bool]>) {
    for (i, (b, a)) in before.chunks_exact(4).zip(after.chunks_exact_mut(4)).enumerate() {
        if mask.is_some_and(|m| !m[i]) || (a[3] == 0 && b[3] == 0) { a.copy_from_slice(b); }
    }
}

/// Filter `crop` of `level`, return the shown part.
pub fn filter_crop(f: &Filter, level: &Level, crop: &Crop) -> Vec<u8> {
    let (px, mask) = cut(level, crop.read);
    let (rw, rh) = (crop.read.2 - crop.read.0, crop.read.3 - crop.read.1);
    let mut out = f.apply(&px, rw, rh);
    keep_outside(&px, &mut out, mask.as_deref());
    let (sx, sy) = (crop.shown.0 - crop.read.0, crop.shown.1 - crop.read.1);
    let (sw, sh) = crop.shown_size();
    let mut shown = Vec::with_capacity(sw * sh * 4);
    for y in sy..sy + sh { shown.extend_from_slice(&out[(y * rw + sx) * 4..(y * rw + sx + sw) * 4]); }
    shown
}

struct Job { key: u64, filter: Filter, crop: Crop, source: Arc<Source> }

/// The next job, and "the dialog closed".
type Slot = Arc<(Mutex<(Option<Job>, bool)>, Condvar)>;

/// A filtered crop: its key, the crop, and its pixels.
pub type Done = (u64, Crop, Vec<u8>);

/// The thread filtering the preview's crops: only the latest request
/// counts (dragging a slider asks every frame); it ends with the dialog.
pub struct Worker {
    slot: Slot,
    done: Receiver<Done>,
}

impl Worker {
    pub fn new(ctx: &egui::Context) -> Worker {
        let slot: Slot = Arc::new((Mutex::new((None, false)), Condvar::new()));
        let (send, done) = channel();
        let (s, ctx) = (Arc::clone(&slot), ctx.clone());
        let spawned = std::thread::Builder::new().name("filter preview".into()).spawn(move || loop {
            let job = {
                let (lock, wake) = &*s;
                let mut g = lock.lock().unwrap();
                loop {
                    if g.1 { return; }
                    if let Some(j) = g.0.take() { break j; }
                    g = wake.wait(g).unwrap();
                }
            };
            let level = job.source.level(job.crop.k);
            let px = filter_crop(&job.filter, level, &job.crop);
            if send.send((job.key, job.crop, px)).is_err() { return; }
            ctx.request_repaint();
        });
        if let Err(e) = spawned { log::error!("filter preview thread: {e}"); }
        Worker { slot, done }
    }

    pub fn ask(&self, key: u64, filter: Filter, crop: Crop, source: Arc<Source>) {
        let (lock, wake) = &*self.slot;
        lock.lock().unwrap().0 = Some(Job { key, filter, crop, source });
        wake.notify_one();
    }

    /// The results that came since the last call, oldest first.
    pub fn take(&self) -> Vec<Done> { self.done.try_iter().collect() }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.slot;
        lock.lock().unwrap().1 = true;
        wake.notify_one();
    }
}

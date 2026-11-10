//! The screen's merge, on a thread of its own: the paint thread hands it a
//! snapshot of the layer stack (its tile tables shared, taken under the
//! document's lock in a moment) and the areas that changed, then goes on
//! painting while this one merges them, cuts them along the GPU pages,
//! sorts the pages sent whole and puts them on the projection. The
//! document is not locked while merging; painting and merging run side by
//! side (a tile painted while a snapshot holds it is copied once).
//!
//! One job at a time: while one is merged, the paint thread gathers the
//! next areas, so a merge slower than the pen merges more at once instead
//! of falling behind.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use aurastudio_compose::{Opts, StrokeCache};
use aurastudio_core::Rect;
use aurastudio_gpu::{Piece, PAGE};
use aurastudio_layers::LayerStack;
use rayon::prelude::*;

use crate::paint_thread::{Cmd, Event};
use crate::projection::{add_rect, SharedProjection};
use crate::Locked;

/// What to merge.
pub(crate) enum Area {
    Rects(Vec<Rect>),
    /// The whole frame, band by band (a document just shown, a resize):
    /// each band goes to the screen as it comes.
    Whole,
}

pub(crate) struct Job {
    /// The stack as it was when the job was made (`None`: nothing to
    /// merge, only `after` to tell in turn).
    pub stack:  Option<LayerStack>,
    pub area:   Area,
    /// The layer a stroke paints now (its merge through the stroke cache).
    pub layer:  Option<u64>,
    /// Told once the job is on the projection (a transform preview shown).
    pub after:  Vec<Event>,
    /// The newest pen sample in it (input clock seconds): the latency
    /// report follows it.
    pub painted_upto: Option<f64>,
}

/// The merge thread, as the paint thread holds it.
pub(crate) struct Merger {
    tx:     Option<Sender<Job>>,
    handle: Option<JoinHandle<()>>,
    busy:   Arc<AtomicBool>,
    /// The slowest merge since asked last, microseconds (a stroke's pace).
    slowest: Arc<AtomicU64>,
}

impl Merger {
    /// `wake`: the paint thread's queue (told when a job is done, so the
    /// next one goes at once).
    pub fn spawn(projection: SharedProjection, cache: StrokeCache, notify: Arc<dyn Fn(Event) + Send + Sync>, wake: Sender<Cmd>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let busy = Arc::new(AtomicBool::new(false));
        let slowest = Arc::new(AtomicU64::new(0));
        let (b, s) = (Arc::clone(&busy), Arc::clone(&slowest));
        let handle = std::thread::Builder::new().name("merge".into()).spawn(move || {
            profiling::register_thread!("merge");
            for job in rx {
                let t = Instant::now();
                run(job, &projection, &cache, &*notify);
                s.fetch_max(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                b.store(false, Ordering::Release);
                let _ = wake.send(Cmd::Merged);
            }
        });
        if let Err(e) = &handle { log::error!("the merge thread could not start: {e}"); }
        Self { tx: Some(tx), handle: handle.ok(), busy, slowest }
    }

    /// A job is being merged.
    pub fn busy(&self) -> bool { self.busy.load(Ordering::Acquire) }

    pub fn send(&self, job: Job) {
        self.busy.store(true, Ordering::Release);
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(job).is_ok());
        if !sent { self.busy.store(false, Ordering::Release); }
    }

    /// The slowest merge since the last call.
    pub fn take_slowest(&self) -> std::time::Duration { std::time::Duration::from_micros(self.slowest.swap(0, Ordering::Relaxed)) }
}

impl Drop for Merger {
    /// The jobs sent are all merged first.
    fn drop(&mut self) {
        self.tx = None;
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

/// The rects cut along the GPU pages, joined per page where that costs no
/// more area: each piece is one page's (merged and written without a copy).
fn by_page(rects: &[Rect], frame: Rect) -> Vec<Rect> {
    let p = PAGE as i32;
    let mut pages: std::collections::BTreeMap<(i32, i32), Vec<Rect>> = std::collections::BTreeMap::new();
    for r in rects {
        let r = r.intersect(&frame);
        if r.is_empty() { continue; }
        for py in r.y0.div_euclid(p)..=(r.y1 - 1).div_euclid(p) {
            for px in r.x0.div_euclid(p)..=(r.x1 - 1).div_euclid(p) {
                let part = r.intersect(&Rect::new(px * p, py * p, px * p + p, py * p + p));
                if !part.is_empty() { add_rect(pages.entry((py, px)).or_default(), part); }
            }
        }
    }
    pages.into_values().flatten().collect()
}

/// Merge `rects` of `stack` and make them ready for the GPU.
fn pieces(stack: &LayerStack, layer: Option<u64>, cache: &StrokeCache, rects: &[Rect]) -> Vec<Piece> {
    let canvas = (stack.width, stack.height);
    let opts = Opts { ghosts: true, ..Default::default() };
    let px = match layer {
        // Mid-stroke: the layers under and over it from the cache.
        Some(l) => cache.merge_rects(stack, l, rects),
        None => aurastudio_compose::merge_rects(stack, rects, opts),
    };
    rects.par_iter().zip(px).flat_map_iter(|(r, px)| aurastudio_gpu::prepare(canvas, *r, px)).collect()
}

fn run(job: Job, projection: &SharedProjection, cache: &StrokeCache, notify: &(dyn Fn(Event) + Send + Sync)) {
    let Job { stack, area, layer, after, painted_upto } = job;
    if let Some(stack) = stack { merge(&stack, area, layer, projection, cache, notify); }
    // (After the push: a frame that takes these samples has them.)
    if let Some(t) = painted_upto { crate::latency::merged(t); }
    for e in after { notify(e); }
}

fn merge(stack: &LayerStack, area: Area, layer: Option<u64>, projection: &SharedProjection, cache: &StrokeCache, notify: &(dyn Fn(Event) + Send + Sync)) {
    let frame = stack.frame();
    let size = (stack.width, stack.height);
    // (Pushed only while the screen is this size: a resize sent after.)
    let push = |pieces: Vec<Piece>| -> bool {
        let mut p = projection.locked();
        if (p.width, p.height) != size { return false; }
        p.push(pieces);
        true
    };
    match area {
        Area::Whole => {
            profiling::scope!("merge whole");
            cache.clear();
            for y0 in (0..stack.height).step_by(PAGE as usize) {
                let band = Rect::from_xywh(0, y0 as i32, stack.width, PAGE.min(stack.height - y0));
                if !push(pieces(stack, None, cache, &by_page(&[band], frame))) { break; }
                notify(Event::Painted);
            }
        }
        Area::Rects(rects) => {
            profiling::scope!("merge for the screen");
            // A filter layer reading far around (a big blur): one piece
            // over them all reads its margin once.
            let far = rects.len() > 1 && aurastudio_compose::margin(&stack.root, aurastudio_layers::Pick::ALL) > 16;
            let rects = if far { vec![rects.iter().fold(rects[0], |a, b| a.union(b)).intersect(&frame)] } else { by_page(&rects, frame) };
            let rects: Vec<Rect> = rects.into_iter().filter(|r| !r.is_empty()).collect();
            if !rects.is_empty() && push(pieces(stack, layer, cache, &rects)) { notify(Event::Painted); }
        }
    }
}

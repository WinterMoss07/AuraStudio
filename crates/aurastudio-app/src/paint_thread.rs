//! The paint thread: pen samples queue up here and are painted in batches,
//! on this one thread. Every other edit of the document's pixels or
//! history (undo, clear, fill, filters, transform...) goes through the same
//! queue, after what was sent before it. It also merges the changed areas
//! for the screen (Krita): every sample is painted as it comes, but the
//! screen's merge is made once per canvas frame (Preferences, "Canvas
//! frame rate"), what changed since the last one together.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use crate::Locked;

use aurastudio_compose::{Opts, StrokeCache};
use aurastudio_core::{Rect, Rgba8};
use aurastudio_document::{DocId, Done, Edit, Region, SharedDoc};
use aurastudio_file::SaveCaches;
use aurastudio_forge::{Brush, Forge, MixerState, PenSample};
use aurastudio_layers::FillParams;

use crate::projection::{add_rect, SharedProjection};

/// Set by the interface while it waits for the document: between two
/// samples the paint thread lets it in (a big brush otherwise kept the
/// document locked nearly all the time and the interface froze).
pub static UI_WANTS_DOC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the paint thread holds back for the interface, at most.
const MAX_YIELD: Duration = Duration::from_millis(4);

/// Most pen samples painted together (the screen still gets a frame
/// between two batches of a stroke far behind).
const MAX_BATCH: usize = 64;

/// What the paint thread is doing (the interface says what it waited for).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Work { Idle, Stroke, StrokeEnd, Edit, Merge, Save }

static WORK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn work(w: Work) { WORK.store(w as u8, Ordering::Relaxed); }

/// What the paint thread is doing now.
pub fn current_work() -> Work {
    match WORK.load(Ordering::Relaxed) {
        1 => Work::Stroke,
        2 => Work::StrokeEnd,
        3 => Work::Edit,
        4 => Work::Merge,
        5 => Work::Save,
        _ => Work::Idle,
    }
}

/// Wait (briefly) while the interface asks for the document.
fn let_ui_in() {
    if !UI_WANTS_DOC.load(Ordering::Acquire) { return; }
    let t = Instant::now();
    while UI_WANTS_DOC.load(Ordering::Acquire) && t.elapsed() < MAX_YIELD { std::thread::yield_now(); }
}

/// Time between two merges for the screen, in microseconds: one canvas
/// frame (the app sets it from the preferences; 60 fps by default).
static CANVAS_FRAME_US: AtomicU64 = AtomicU64::new(16_667);

/// Merge for the screen at most this often (the end of a stroke, an undo...
/// are shown at once all the same).
pub fn set_canvas_frame(d: Duration) { CANVAS_FRAME_US.store((d.as_micros() as u64).clamp(1_000, 100_000), Ordering::Relaxed); }

pub fn canvas_frame() -> Duration { Duration::from_micros(CANVAS_FRAME_US.load(Ordering::Relaxed)) }

fn is_filter(c: &Cmd) -> bool { matches!(c, Cmd::Edit { edit: Edit::FilterPreview(_) | Edit::FilterApply(_) | Edit::FilterCancel, .. }) }

/// Rows merged at a time when the whole frame is merged again (the
/// interface gets the document between two): a row of GPU pages, each
/// sent whole (sorted into its smallest kind).
const BAND: u32 = aurastudio_gpu::PAGE;

pub enum Cmd {
    /// A stroke on the layer strokes go to now; `limit`: stay inside the
    /// lines, the area these fill settings find from the first sample.
    /// `zoom`: the view's (smoothing measures its string on screen).
    Begin { doc: SharedDoc, brush: Box<Brush>, limit: Option<FillParams>, zoom: f32 },
    /// One pen sample.
    Sample(PenSample),
    End,
    /// Any other edit of the document, after the strokes sent before.
    /// Only the latest of the filter / transform previews waiting is done;
    /// `seq` comes back in `TransformShown` once a preview is shown.
    Edit { doc: SharedDoc, edit: Edit, seq: Option<u64> },
    /// The interface changed the tree or the time: merge this again for
    /// the screen.
    Show { doc: SharedDoc, region: Region },
    /// Save the document to `path` (after the strokes sent before). The
    /// file is written on its own thread; painting goes on meanwhile.
    /// `origin`: autosaves only, the file the work belongs to.
    Save { doc: SharedDoc, path: PathBuf, origin: Option<PathBuf>, auto: bool },
    /// The document is closed: what was kept for it goes.
    Forget(DocId),
    /// The transform's live preview over `area` (it moved past the one it
    /// had).
    Lift { doc: SharedDoc, area: Rect },
    /// The Mixer Brush's paint: loaded with `color`, or cleaned (`color`:
    /// the one in use).
    Mixer { load: bool, color: [f32; 4] },
}

/// What the worker reports to the main thread (through `notify`).
pub enum Event {
    /// The selection changed (its outline to draw).
    Selection,
    /// A transform began: what moves (canvas pixels), or why it can't.
    TransformStarted(Result<Rect, &'static str>),
    /// The pieces of its live preview (the GPU draws it while dragging).
    TransformLift(Box<aurastudio_document::Lift>),
    /// Preview number n is on the projection.
    TransformShown(u64),
    /// The projection has new changed areas to upload.
    Painted,
    /// A layer's pixels changed (stroke done, undo...): its thumbnail.
    /// `rect`: where (None: anywhere, or unknown).
    LayerChanged { layer_id: u64, rect: Option<Rect> },
    /// The tree changed (an undo of a layer step): the list and every
    /// thumbnail to refresh.
    TreeChanged,
    /// The canvas has a new size (the screen follows).
    Resized(u32, u32),
    /// Short message for the options bar ("Nothing to undo"...).
    Notice(&'static str),
    /// A save finished: the document revision now on disk, or the error.
    Saved { doc: DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String> },
}

pub struct PaintThread {
    tx:     Option<Sender<Cmd>>,
    handle: Option<JoinHandle<()>>,
    /// Filter commands sent so far: a preview under way stops when more
    /// were sent than the worker took (it would be thrown away).
    filters_sent: Arc<AtomicU64>,
}

impl PaintThread {
    /// `projection` receives the merged result of everything painted.
    pub fn spawn(projection: SharedProjection, notify: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(projection, Arc::new(notify));
        let filters_sent = Arc::clone(&worker.filters_sent);
        let handle = std::thread::Builder::new()
            .name("paint".into())
            .spawn(move || {
                profiling::register_thread!("paint");
                // Its panic unwound (the document is free again): keep the
                // work before closing (rescue.rs).
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run(rx))).is_err() {
                    #[cfg(not(test))]
                    crate::rescue::after_unwind();
                }
            })
            .ok();
        if handle.is_none() { log::error!("the paint thread could not start"); }
        Self { tx: Some(tx), handle, filters_sent }
    }

    pub fn send(&self, cmd: Cmd) {
        if is_filter(&cmd) { self.filters_sent.fetch_add(1, Ordering::AcqRel); }
        if let Some(tx) = &self.tx { let _ = tx.send(cmd); }
    }
}

impl Drop for PaintThread {
    fn drop(&mut self) {
        // Closing the channel ends the worker loop.
        self.tx = None;
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

/// A stroke being painted.
struct Painting {
    engine:  Forge,
    /// The stroke has begun in the document (at the first sample, once the
    /// "stay inside" area is known).
    begun:   bool,
    limit:   Option<FillParams>,
    layer:   u64,
    pace:    StrokePace,
}

/// What the paint thread holds, for the memory report (set as it works).
pub struct Held {
    pub stroke_cache: AtomicU64,
    pub stroke:       AtomicU64,
    pub saves:        AtomicU64,
}

pub static HELD: Held = Held { stroke_cache: AtomicU64::new(0), stroke: AtomicU64::new(0), saves: AtomicU64::new(0) };

struct Worker {
    /// Document being worked on.
    doc:        Option<SharedDoc>,
    projection: SharedProjection,
    /// Changed areas not merged into the projection yet.
    pending:    Vec<Rect>,
    last_update: Instant,
    notify:     Arc<dyn Fn(Event) + Send + Sync>,
    stroke:     Option<Painting>,
    /// Merges of the layers under / over the stroke's layer (compose).
    stroke_cache: StrokeCache,
    /// The last save's encodings per document (incremental saves).
    saves:      HashMap<DocId, Arc<Mutex<SaveCaches>>>,
    /// Held by the autosave writing (two never write the recovery file at
    /// once): the document and revision last written.
    autosaving: Arc<Mutex<Option<(DocId, u64)>>>,
    /// Filter commands taken from the queue, and sent (`PaintThread`).
    filters_taken: u64,
    filters_sent:  Arc<AtomicU64>,
    /// The paint a Mixer Brush carries from stroke to stroke (the engine
    /// holds it during a stroke).
    mixer:         MixerState,
}

/// Painting time of a stroke against the pen's: a stroke that fell behind
/// is logged with what took the time.
#[derive(Default)]
struct StrokePace {
    start:    Option<(Instant, f64)>,
    samples:  u32,
    painting: Duration,
    slowest:  (Duration, u32),
    slowest_merge: Duration,
    behind:   (Duration, u32),
}

impl StrokePace {
    /// `n` samples painted together in `took`, the last at `pen_time`.
    fn sample(&mut self, pen_time: f64, took: Duration, n: u32) {
        let now = Instant::now();
        let (t0, p0) = *self.start.get_or_insert((now - took, pen_time));
        self.samples += n;
        self.painting += took;
        if took > self.slowest.0 { self.slowest = (took, self.samples); }
        let behind = (now - t0).saturating_sub(Duration::from_secs_f64((pen_time - p0).max(0.0)));
        if behind > self.behind.0 { self.behind = (behind, self.samples); }
    }

    /// The stroke is over: what to say about it (nothing if it kept up).
    fn report(&self) -> Option<String> {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        (self.behind.0 > Duration::from_millis(100)).then(|| format!(
            "stroke behind the pen by up to {:.0} ms (sample {} of {}): {:.0} ms painting in all, slowest batch {:.0} ms (to #{}), slowest merge {:.0} ms",
            ms(self.behind.0), self.behind.1, self.samples, ms(self.painting), ms(self.slowest.0), self.slowest.1, ms(self.slowest_merge)))
    }
}

/// The layers around `layer` as a "Sample all layers" Mixer Brush sees
/// them, for one stroke: a copy of the stack as the stroke begins (its
/// tiles shared, the painted layer's own left out so painting it copies
/// none), merged by cells as the dabs ask (each cell once).
fn all_layers(stack: &aurastudio_layers::LayerStack, layer: u64) -> aurastudio_forge::Sampler {
    const CELL: i32 = 128;
    let mut snap = stack.clone();
    if let Some(aurastudio_layers::Node::Layer(l)) = snap.node_mut(layer) { l.surface.clear(); }
    type Cell = Arc<(Vec<Rgba8>, Vec<Rgba8>)>;
    let cells: Mutex<HashMap<(i32, i32), Cell>> = Mutex::default();
    let frame = snap.frame();
    Arc::new(move |r: Rect| {
        let w = r.w() as usize;
        let mut under = vec![[0u8; 4]; r.area() as usize];
        let mut over = vec![[0u8; 4]; r.area() as usize];
        let inside = r.intersect(&frame);
        if inside.is_empty() { return (under, over); }
        for cy in inside.y0.div_euclid(CELL)..=(inside.y1 - 1).div_euclid(CELL) {
            for cx in inside.x0.div_euclid(CELL)..=(inside.x1 - 1).div_euclid(CELL) {
                let cr = Rect::new(cx * CELL, cy * CELL, cx * CELL + CELL, cy * CELL + CELL).intersect(&frame);
                let got = cells.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(cx, cy)).cloned();
                let cell = match got {
                    Some(c) => c,
                    None => {
                        let Some(m) = aurastudio_compose::merge_around(&snap, layer, cr) else { continue };
                        let c = Arc::new(m);
                        cells.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert((cx, cy), Arc::clone(&c));
                        c
                    }
                };
                let part = cr.intersect(&r);
                let cw = cr.w() as usize;
                for y in part.y0..part.y1 {
                    let (from, to) = ((y - cr.y0) as usize * cw + (part.x0 - cr.x0) as usize, (y - r.y0) as usize * w + (part.x0 - r.x0) as usize);
                    let n = part.w() as usize;
                    under[to..to + n].copy_from_slice(&cell.0[from..from + n]);
                    over[to..to + n].copy_from_slice(&cell.1[from..from + n]);
                }
            }
        }
        (under, over)
    })
}

/// A preview another one waiting makes useless.
fn superseded(cmd: &Cmd, queue: &VecDeque<Cmd>) -> bool {
    let is_xform = |c: &Cmd| matches!(c, Cmd::Edit { edit: Edit::TransformPreview { .. } | Edit::TransformApply { .. } | Edit::TransformCancel, .. });
    match cmd {
        Cmd::Edit { edit: Edit::FilterPreview(_), .. } => queue.iter().any(is_filter),
        Cmd::Edit { edit: Edit::TransformPreview { .. }, seq: None, .. } => queue.iter().any(is_xform),
        Cmd::Edit { edit: Edit::TransformPreview { .. }, .. } => queue.iter().any(|c| matches!(c, Cmd::Edit { edit: Edit::TransformApply { .. } | Edit::TransformCancel, .. }) || matches!(c, Cmd::Edit { edit: Edit::TransformPreview { .. }, .. })),
        Cmd::Show { region: Region::Whole, .. } => queue.iter().any(|c| matches!(c, Cmd::Show { region: Region::Whole, .. })),
        _ => false,
    }
}

impl Worker {
    /// Tell the memory report what the caches hold now.
    fn publish_held(&self) {
        let o = Ordering::Relaxed;
        HELD.stroke_cache.store(self.stroke_cache.bytes() as u64, o);
        HELD.stroke.store(self.stroke.as_ref().map_or(0, |s| s.engine.stroke_bytes()) as u64, o);
        let saves: usize = self.saves.values().map(|c| c.try_lock().map_or(0, |c| c.bytes())).sum();
        if saves > 0 || self.saves.is_empty() { HELD.saves.store(saves as u64, o); }
    }

    fn new(projection: SharedProjection, notify: Arc<dyn Fn(Event) + Send + Sync>) -> Self {
        Self {
            doc: None, projection, pending: Vec::new(), last_update: Instant::now(), notify, stroke: None,
            stroke_cache: StrokeCache::default(), saves: HashMap::new(), autosaving: Arc::default(), filters_taken: 0, filters_sent: Arc::default(),
            mixer: MixerState::default(),
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let mut queue = VecDeque::new();
        loop {
            // Sleep until there is work, then take everything queued.
            if queue.is_empty() {
                work(Work::Idle);
                // Painted, not merged yet: wake for the next canvas frame.
                let merge_at = (!self.pending.is_empty()).then(|| self.last_update + canvas_frame());
                let cmd = match merge_at {
                    Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(cmd) => Some(cmd),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    },
                    None => match rx.recv() { Ok(cmd) => Some(cmd), Err(_) => return },
                };
                match cmd {
                    Some(cmd) => queue.push_back(cmd),
                    // Its frame: show what was painted.
                    None => {
                        self.update_projection();
                        (self.notify)(Event::Painted);
                        continue;
                    }
                }
            }
            queue.extend(rx.try_iter());
            profiling::scope!("paint batch");
            let mut events = Vec::new();
            let mut flush = false;
            let t0 = Instant::now();
            while let Some(cmd) = queue.pop_front() {
                if is_filter(&cmd) { self.filters_taken += 1; }
                if superseded(&cmd, &queue) { continue; }
                match cmd {
                    Cmd::Begin { doc, brush, limit, zoom } => {
                        // A stroke still open (its End never came, e.g. the
                        // window lost focus): finished first, recorded for
                        // undo like any other.
                        if self.stroke.is_some() { self.finish_stroke(&mut events); }
                        self.doc = Some(doc);
                        let mut engine = Forge::new(*brush).with_mixer_state(std::mem::take(&mut self.mixer));
                        engine.set_zoom(zoom);
                        self.stroke = Some(Painting { engine, begun: false, limit, layer: 0, pace: StrokePace::default() });
                    }
                    Cmd::Sample(p) => {
                        // Every sample waiting is painted at once: the dabs
                        // are the same, the layer is painted once, a stroke
                        // that fell behind catches up.
                        let mut ps = vec![p];
                        while ps.len() < MAX_BATCH {
                            let Some(Cmd::Sample(q)) = queue.front() else { break };
                            ps.push(*q);
                            queue.pop_front();
                        }
                        let t = Instant::now();
                        work(Work::Stroke);
                        self.sample(&ps, &mut events);
                        if let (Some(s), Some(last)) = (&mut self.stroke, ps.last()) { s.pace.sample(last.time, t.elapsed(), ps.len() as u32); }
                    }
                    Cmd::End => {
                        work(Work::StrokeEnd);
                        self.finish_stroke(&mut events);
                        flush = true;
                    }
                    Cmd::Edit { doc, edit, seq } => {
                        if self.stroke.is_some() { self.finish_stroke(&mut events); }
                        work(Work::Edit);
                        self.edit(doc, edit, seq, &mut events);
                        flush = true;
                    }
                    Cmd::Show { doc, region } => {
                        match region {
                            Region::Whole => self.merge_whole(doc),
                            r => { self.add_region(&doc, &r); self.doc = Some(doc); }
                        }
                    }
                    Cmd::Save { doc, path, origin, auto } => self.save(doc, path, origin, auto),
                    Cmd::Lift { doc, area } => {
                        if self.stroke.is_some() { self.finish_stroke(&mut events); }
                        let lift = doc.locked().transform_lift(Some(area));
                        if let Some(l) = lift { events.push(Event::TransformLift(Box::new(l))); }
                        self.doc = Some(doc);
                    }
                    Cmd::Mixer { load, color } => {
                        if self.stroke.is_some() { self.finish_stroke(&mut events); }
                        if load { self.mixer.load(color); } else { self.mixer.clean(color); }
                    }
                    Cmd::Forget(id) => {
                        self.saves.remove(&id);
                        if self.doc.as_ref().is_some_and(|d| d.lock().map_or(true, |d| d.id() == id)) {
                            self.doc = None;
                            self.stroke_cache.clear();
                        }
                        self.publish_held();
                    }
                }
                if flush || t0.elapsed() >= canvas_frame() { break; }
            }
            // Update the projection once per canvas frame (when behind the
            // pen too), or at once when asked to (the end of a stroke...).
            queue.extend(rx.try_iter());
            let due = flush || self.last_update.elapsed() >= canvas_frame();
            if due && !self.pending.is_empty() {
                self.update_projection();
                (self.notify)(Event::Painted);
            }
            for e in events { (self.notify)(e); }
        }
    }

    /// `region` of `doc`'s frame to merge again.
    fn add_region(&mut self, doc: &SharedDoc, region: &Region) {
        let frame = {
            let d = doc.locked();
            d.stack().frame()
        };
        self.add_region_in(frame, region);
    }

    fn add_region_in(&mut self, frame: Rect, region: &Region) {
        match region {
            Region::Nothing => {}
            Region::Whole => add_rect(&mut self.pending, frame),
            Region::Rects(v) => for r in v { add_rect(&mut self.pending, r.intersect(&frame)); },
        }
    }

    /// What an edit did, shown and told.
    fn report(&mut self, frame: Rect, done: &Done, events: &mut Vec<Event>) { self.report_merging(frame, done, &done.region, events); }

    /// `report`, merging `merge` (what the screen doesn't show yet) of
    /// what changed.
    fn report_merging(&mut self, frame: Rect, done: &Done, merge: &Region, events: &mut Vec<Event>) {
        self.add_region_in(frame, merge);
        if done.selection { events.push(Event::Selection); }
        if done.tree { events.push(Event::TreeChanged); }
        let rect = done.region.bounds().flatten();
        for &l in &done.layers { events.push(Event::LayerChanged { layer_id: l, rect }); }
        if let Some(why) = done.refused { events.push(Event::Notice(why)); }
    }

    fn edit(&mut self, doc: SharedDoc, edit: Edit, seq: Option<u64>, events: &mut Vec<Event>) {
        let _name = edit.label();
        profiling::scope!("edit", _name);
        let (undo, redo, begin) = (matches!(edit, Edit::Undo), matches!(edit, Edit::Redo), matches!(edit, Edit::TransformBegin(_)));
        let (taken, sent) = (self.filters_taken, Arc::clone(&self.filters_sent));
        let stop = move || sent.load(Ordering::Acquire) > taken;
        let (done, frame, started, lift, size) = {
            let mut d = doc.locked();
            let before = (d.stack().width, d.stack().height);
            let done = d.apply_until(edit, &stop);
            let size = (d.stack().width, d.stack().height);
            if size != before { log::info!("canvas resize: {}x{} -> {}x{}", before.0, before.1, size.0, size.1); }
            let size = (size != before).then_some(size);
            let started = begin.then(|| match done.refused {
                Some(why) => Err(why),
                None => d.transform_source().ok_or("Nothing to transform"),
            });
            // The box first: the app takes the lift only for a box it has.
            let lift = started.as_ref().is_some_and(Result::is_ok).then(|| d.transform_lift(None)).flatten();
            (done, d.stack().frame(), started, lift, size)
        };
        if let Some((w, h)) = size { events.push(Event::Resized(w, h)); }
        if let Some(s) = started { events.push(Event::TransformStarted(s)); }
        if let Some(l) = lift { events.push(Event::TransformLift(Box::new(l))); }
        if (undo || redo) && !done.changed() {
            match done.refused {
                // (Told by `report`.)
                Some(why) => log::warn!("{}: {why}; the step was dropped", if undo { "undo" } else { "redo" }),
                None => events.push(Event::Notice(if undo { "Nothing to undo" } else { "Nothing to redo" })),
            }
        }
        // (A whole merge after a tree step: the caches no longer match.)
        if done.tree || done.region == Region::Whole { self.stroke_cache.clear(); }
        self.report(frame, &done, events);
        if let Some(n) = seq { events.push(Event::TransformShown(n)); }
        self.doc = Some(doc);
        self.publish_held();
    }

    fn sample(&mut self, ps: &[PenSample], events: &mut Vec<Event>) {
        let Some(&p) = ps.first() else { return };
        let Some(doc) = self.doc.clone() else { return };
        let Some(mut st) = self.stroke.take() else { return };
        let_ui_in();
        let mut d = doc.locked();
        let frame = d.stack().frame();
        if !st.begun {
            st.begun = true;
            let limit = st.limit.take().and_then(|lp| d.stay_inside(p.x, p.y, lp));
            let done = d.begin_stroke("Brush stroke", limit);
            if let Some(why) = done.refused {
                events.push(Event::Notice(why));
                self.mixer = st.engine.take_mixer_state();
                return;
            }
            st.layer = d.stroke_layer().unwrap_or(0);
            let m = &st.engine.brush().mixer;
            if m.enabled && m.sample_all_layers && !st.engine.brush().erase {
                st.engine.set_sampler(Some(all_layers(d.stack(), st.layer)));
            }
            // (A sheet made for it: the time shows another drawing.)
            self.stroke_cache.clear();
            self.add_region_in(frame, &done.region);
            if done.tree { events.push(Event::TreeChanged); }
        }
        // The layer gone: nothing left to paint on (the stroke stops).
        let Some(r) = d.paint_area(|s| st.engine.stroke_through(s, ps)) else {
            self.mixer = st.engine.take_mixer_state();
            return;
        };
        drop(d);
        if let Some(r) = r { add_rect(&mut self.pending, r.intersect(&frame)); }
        self.stroke = Some(st);
    }

    /// End the open stroke (recorded for undo) and report its layer.
    fn finish_stroke(&mut self, events: &mut Vec<Event>) {
        let Some(mut st) = self.stroke.take() else { return };
        if let Some(r) = st.pace.report() { log::warn!("{r}"); }
        let Some(doc) = self.doc.clone() else { self.mixer = st.engine.take_mixer_state(); return };
        let (done, frame, last) = {
            let mut d = doc.locked();
            let last = d.paint_area(|s| st.engine.end_stroke(s)).flatten();
            (d.end_stroke(), d.stack().frame(), last)
        };
        self.mixer = st.engine.take_mixer_state();
        // Each stroke builds its own (a stroke begins with a new one): the
        // memory goes back now, not at the next stroke.
        self.stroke_cache.clear();
        if done.tree {
            // (A sheet made for it went: the time shows another drawing.)
            self.report(frame, &done, events);
        } else {
            // What the stroke painted is on the screen already: only its
            // last dabs are merged, not the whole stroke again.
            if let Some(r) = last { add_rect(&mut self.pending, r.intersect(&frame)); }
            self.report_merging(frame, &done, &Region::Nothing, events);
        }
        self.publish_held();
    }

    /// Merge the whole frame again by bands, the document locked for one at
    /// a time (the interface runs between two), each band shown as it comes.
    fn merge_whole(&mut self, doc: SharedDoc) {
        profiling::scope!("merge whole");
        self.stroke_cache.clear();
        let (w, h) = {
            let d = doc.locked();
            (d.stack().width, d.stack().height)
        };
        // (What was waiting is part of it.)
        self.pending.clear();
        // Only asked for a canvas just made (a document shown, a resize):
        // each band goes to the screen at once, nothing is held.
        for y0 in (0..h).step_by(BAND as usize) {
            let r = Rect::from_xywh(0, y0 as i32, w, BAND.min(h - y0));
            let px = {
                let d = doc.locked();
                // (Resized meanwhile: a new whole merge comes.)
                if (d.stack().width, d.stack().height) != (w, h) { break; }
                aurastudio_compose::merge(d.stack(), r, Opts { ghosts: true, ..Default::default() })
            };
            {
                let mut p = self.projection.locked();
                if (p.width, p.height) != (w, h) { break; }
                p.push(r, px);
            }
            (self.notify)(Event::Painted);
            let_ui_in();
        }
        self.doc = Some(doc);
    }

    /// Merge the layers of the changed areas into the projection.
    fn update_projection(&mut self) {
        profiling::scope!("merge for the screen");
        work(Work::Merge);
        self.last_update = Instant::now();
        let rects = std::mem::take(&mut self.pending);
        let Some(doc) = self.doc.clone() else { return };
        let d = doc.locked();
        let s = d.stack();
        let frame = s.frame();
        // A filter layer reading far around (a big blur): one piece over
        // them all reads its margin once.
        let rects = if rects.len() > 1 && aurastudio_compose::margin(&s.root, aurastudio_layers::Pick::ALL) > 16 { vec![rects.iter().fold(rects[0], |a, b| a.union(b))] } else { rects };
        let t = Instant::now();
        let layer = self.stroke.as_ref().filter(|p| p.begun).map(|p| p.layer);
        let patches: Vec<_> = rects.into_iter().map(|r| r.intersect(&frame)).filter(|r| !r.is_empty()).map(|r| {
            let px = match layer {
                // Mid-stroke: the layers under and over it from the cache.
                Some(l) => self.stroke_cache.merge(s, l, r),
                None => aurastudio_compose::merge(s, r, Opts { ghosts: true, ..Default::default() }),
            };
            (r, px)
        }).collect();
        if let Some(st) = &mut self.stroke { st.pace.slowest_merge = st.pace.slowest_merge.max(t.elapsed()); }
        self.publish_held();
        let (w, h) = (s.width, s.height);
        drop(d);
        let mut p = self.projection.locked();
        if (p.width, p.height) != (w, h) { return; }
        for (r, px) in patches { p.push(r, px); }
    }

    /// Locked only to take the parts (shared tiles and patches); the
    /// preview merge, packing and writing are done on a thread of its own.
    fn save(&mut self, doc: SharedDoc, path: PathBuf, origin: Option<PathBuf>, auto: bool) {
        work(Work::Save);
        let (parts, id) = {
            let d = doc.locked();
            (d.snapshot_parts(origin.as_deref()), d.id())
        };
        let caches = Arc::clone(self.saves.entry(id).or_default());
        let notify = Arc::clone(&self.notify);
        let autosaving = Arc::clone(&self.autosaving);
        let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
            profiling::register_thread!("save");
            let snap = parts.finish();
            // (An autosave has its own file: the caches are the document's.)
            let result = if auto {
                let mut last = autosaving.locked();
                // (A newer one of the same document written first: this
                // one is stale.)
                if last.is_some_and(|(d, r)| d == id && snap.revision < r) { Ok(()) } else {
                    *last = Some((id, snap.revision));
                    aurastudio_file::save(&snap, &path, &mut SaveCaches::default())
                }
            } else {
                aurastudio_file::save(&snap, &path, &mut caches.locked())
            }.map_err(|e| e.to_string());
            log::info!("{} {}: {:?}", if auto { "autosave" } else { "save" }, path.display(), result);
            notify(Event::Saved { doc: id, path, revision: snap.revision, auto, result });
        });
        if let Err(e) = spawned { log::error!("save thread: {e}"); }
        self.doc = Some(doc);
    }
}

/// Upload helper for tests: paste merged patches into a frame-sized image.
#[cfg(test)]
fn blit(screen: &mut [aurastudio_core::Rgba8], w: u32, r: Rect, px: &[aurastudio_core::Rgba8]) {
    for y in r.y0..r.y1 {
        let row = ((y - r.y0) as u32 * r.w()) as usize;
        let o = (y as u32 * w + r.x0 as u32) as usize;
        screen[o..o + r.w() as usize].copy_from_slice(&px[row..row + r.w() as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_core::DocType;
    use aurastudio_document::Document;
    use std::sync::mpsc::channel;
    use crate::projection::Projection;

    fn proj(w: u32, h: u32) -> SharedProjection { Arc::new(Mutex::new(Projection::new(w, h))) }

    fn doc_with_layer() -> (SharedDoc, u64) {
        let mut d = Document::new(DocId(1), "t", 64, 64, DocType::Illustration);
        let id = d.apply(Edit::AddLayer("ink".into())).made.unwrap();
        (Arc::new(Mutex::new(d)), id)
    }

    fn pixel(doc: &SharedDoc, id: u64, x: i32, y: i32) -> [u8; 4] { doc.locked().stack().image(id).unwrap().pixel(x, y) }

    fn brush(r: f32) -> Brush { Brush { size: r * 2.0, hardness: 1.0, pressure_size: false, color: [0.0, 0.0, 0.0, 1.0], ..Brush::default() } }

    fn stroke(pt: &PaintThread, doc: &SharedDoc, y: f32) {
        pt.send(Cmd::Begin { doc: Arc::clone(doc), brush: Box::new(brush(4.0)), limit: None, zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(10.0, y, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(50.0, y, 1.0, 0.01)));
        pt.send(Cmd::End);
    }

    fn edit(pt: &PaintThread, doc: &SharedDoc, edit: Edit) { pt.send(Cmd::Edit { doc: Arc::clone(doc), edit, seq: None }); }

    /// A stroke sent to the thread ends up on the layer, the thread reports
    /// the changed layer, and the projection gets the merged result.
    #[test]
    fn paints_on_its_own_thread() {
        let (doc, id) = doc_with_layer();
        let (ev_tx, ev_rx) = channel();
        let projection = proj(64, 64);
        let pt = PaintThread::spawn(Arc::clone(&projection), move |e| {
            if let Event::LayerChanged { layer_id, .. } = e { let _ = ev_tx.send(layer_id); }
        });
        stroke(&pt, &doc, 32.0);
        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(5)), Ok(id));
        assert_eq!(pixel(&doc, id, 30, 32), [0, 0, 0, 255]);
        drop(pt);
        let patches = projection.locked().take_patches();
        let mut screen = vec![[0u8; 4]; 64 * 64];
        for (r, px) in &patches { blit(&mut screen, 64, *r, px); }
        assert_eq!(screen[32 * 64 + 30], [0, 0, 0, 255]);
    }

    /// Samples sent faster than painted are painted together; the screen
    /// ends exactly as the layers merge though the stroke is not merged
    /// again at its end.
    #[test]
    fn the_screen_matches_the_stroke_without_a_second_merge() {
        let (doc, id) = doc_with_layer();
        let projection = proj(64, 64);
        let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});
        pt.send(Cmd::Show { doc: Arc::clone(&doc), region: Region::Whole });
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(3.0)), limit: None, zoom: 1.0 });
        for i in 0..300 {
            let t = i as f32 / 300.0;
            pt.send(Cmd::Sample(PenSample::new(6.0 + 52.0 * t, 32.0 + 20.0 * (t * 6.0).sin(), 0.5 + t / 2.0, i as f64 * 0.001)));
        }
        pt.send(Cmd::End);
        drop(pt);
        let patches = projection.locked().take_patches();
        let mut screen = vec![[0u8; 4]; 64 * 64];
        for (r, px) in &patches { blit(&mut screen, 64, *r, px); }
        let d = doc.locked();
        let merged = d.merged(Rect::new(0, 0, 64, 64), aurastudio_layers::Pick::ALL);
        assert_eq!(screen, merged);
        assert_eq!(d.stack().image(id).map(|i| i.pixel(6, 32)[3]), Some(255));
    }

    fn line_with(pt: &PaintThread, doc: &SharedDoc, b: &Brush, y: f32) {
        pt.send(Cmd::Begin { doc: Arc::clone(doc), brush: Box::new(b.clone()), limit: None, zoom: 1.0 });
        for i in 0..=20 { pt.send(Cmd::Sample(PenSample::new(8.0 + i as f32 * 2.4, y, 1.0, i as f64 * 0.005))); }
        pt.send(Cmd::End);
    }

    /// A Mixer Brush keeps its paint from stroke to stroke; Clean and Load
    /// change it.
    #[test]
    fn the_mixer_paint_is_kept_cleaned_and_loaded() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut b = brush(3.0);
        // Dry (paints only what it holds), nothing automatic.
        b.mixer = aurastudio_forge::MixerSettings { enabled: true, wet: 0.0, load: 1.0, clean_after_stroke: false, load_after_stroke: false, ..Default::default() };
        line_with(&pt, &doc, &b, 10.0);
        pt.send(Cmd::Mixer { load: false, color: b.color });
        line_with(&pt, &doc, &b, 30.0);
        pt.send(Cmd::Mixer { load: true, color: b.color });
        line_with(&pt, &doc, &b, 50.0);
        drop(pt);
        assert!(pixel(&doc, id, 30, 10)[3] > 200, "loaded at first");
        assert_eq!(pixel(&doc, id, 30, 30)[3], 0, "cleaned: nothing to paint");
        assert!(pixel(&doc, id, 30, 50)[3] > 200, "loaded again");
    }

    /// "Sample all layers": an empty layer over a red one picks the red up.
    #[test]
    fn a_mixer_brush_samples_the_layers_under() {
        for all in [true, false] {
            let (doc, ink) = doc_with_layer();
            let under = {
                let mut d = doc.locked();
                let under = d.apply(Edit::AddLayer("under".into())).made.unwrap();
                d.apply(Edit::Move { ids: vec![under], dragged: under, target: ink, place: aurastudio_layers::Place::Below });
                under
            };
            let pt = PaintThread::spawn(proj(64, 64), |_| {});
            let mut red = brush(12.0);
            red.color = [0.8, 0.0, 0.0, 1.0];
            line_with(&pt, &doc, &red, 32.0);
            edit(&pt, &doc, Edit::SetActive(ink));
            let mut b = brush(4.0);
            b.mixer = aurastudio_forge::MixerSettings { enabled: true, wet: 1.0, mix: 1.0, load: 0.0, sample_all_layers: all, ..Default::default() };
            pt.send(Cmd::Mixer { load: false, color: b.color });
            line_with(&pt, &doc, &b, 32.0);
            drop(pt);
            assert!(pixel(&doc, under, 30, 32)[0] > 150, "the red layer");
            let p = pixel(&doc, ink, 40, 32);
            if all { assert!(p[3] > 0 && p[0] > p[1] + 40, "red picked up: {p:?}"); } else { assert_eq!(p[3], 0, "its own layer only: {p:?}"); }
        }
    }

    /// A resize is done on the paint thread, which tells the new size.
    #[test]
    fn a_resize_is_reported() {
        let (doc, _) = doc_with_layer();
        let (ev_tx, ev_rx) = channel();
        let pt = PaintThread::spawn(proj(64, 64), move |e| {
            if let Event::Resized(w, h) = e { let _ = ev_tx.send((w, h)); }
        });
        edit(&pt, &doc, Edit::Resize { width: 80, height: 40 });
        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(5)), Ok((80, 40)));
        let d = doc.locked();
        assert_eq!((d.stack().width, d.stack().height), (80, 40));
    }

    /// The onion skin reaches the screen: the merge for it draws the
    /// ghosts (the sheet before, red).
    #[test]
    fn the_screen_shows_the_onion_skin() {
        let mut d = Document::new(DocId(1), "a", 64, 64, DocType::Animation);
        let a = d.stack().root.iter().find(|n| n.is_animation()).unwrap().id();
        d.apply(Edit::SetActive(a));
        d.apply(Edit::SetFrame(1));
        let ink = |d: &mut Document, r: Rect| { d.stroke("Brush stroke", |s| { use aurastudio_core::PaintSurface; s.prepare(r); let mut v = Vec::new(); s.spans_mut(r, &mut v); for sp in v { sp.pixels.fill([0, 0, 0, 255]); } }); };
        ink(&mut d, Rect::new(4, 4, 12, 12));
        d.apply(Edit::SetFrame(3));
        d.apply(Edit::SetActive(a));
        ink(&mut d, Rect::new(40, 40, 50, 50));
        let doc = Arc::new(Mutex::new(d));
        let projection = proj(64, 64);
        let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});
        let mut o = aurastudio_animation::Onion { enabled: true, ..Default::default() };
        o.mobile.insert(-1, Some(1.0));
        edit(&pt, &doc, Edit::SetOnion(o));
        drop(pt);
        let mut screen = vec![[0u8; 4]; 64 * 64];
        for (r, px) in &projection.locked().take_patches() { blit(&mut screen, 64, *r, px); }
        let g = screen[8 * 64 + 8];
        assert!(g[0] > 200 && g[1] < 60, "the sheet before, red: {g:?}");
    }

    /// The first stroke in an animation folder makes its first sheet:
    /// every piece merged for the screen meanwhile is the paper and the
    /// ink, never a hole (it showed as a black block).
    #[test]
    fn the_first_sheet_shows_no_hole() {
        let mut d = Document::new(DocId(1), "a", 300, 200, DocType::Animation);
        let a = d.stack().root.iter().find(|n| n.is_animation()).unwrap().id();
        d.apply(Edit::SetActive(a));
        let doc = Arc::new(Mutex::new(d));
        let projection = proj(300, 200);
        let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(20.0)), limit: None, zoom: 1.0 });
        for i in 0..30 {
            pt.send(Cmd::Sample(PenSample::new(20.0 + i as f32 * 8.0, 100.0, 1.0, i as f64 * 0.01)));
            std::thread::sleep(Duration::from_millis(3));
        }
        pt.send(Cmd::End);
        drop(pt);
        let patches = projection.locked().take_patches();
        assert!(!patches.is_empty());
        for (r, px) in &patches {
            let holes = px.iter().filter(|p| p[3] < 255).count();
            assert_eq!(holes, 0, "piece {r:?}: {holes} pixels not opaque");
        }
    }

    /// A big dab (many tiles, painted on all cores): whole, no seams.
    #[test]
    fn a_big_brush_paints_every_tile() {
        let mut d = Document::new(DocId(1), "t", 400, 400, DocType::Illustration);
        let id = d.apply(Edit::AddLayer("ink".into())).made.unwrap();
        let doc = Arc::new(Mutex::new(d));
        let pt = PaintThread::spawn(proj(400, 400), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(150.0)), limit: None, zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(200.0, 200.0, 1.0, 0.0)));
        pt.send(Cmd::End);
        drop(pt);
        for (x, y) in [(200, 200), (63, 200), (64, 200), (127, 128), (128, 127), (300, 300), (200, 60)] {
            assert_eq!(pixel(&doc, id, x, y), [0, 0, 0, 255], "({x}, {y})");
        }
        assert_eq!(pixel(&doc, id, 5, 5)[3], 0);
    }

    /// A pen faster than the canvas frame rate: every sample is painted,
    /// but the screen's merge comes about once per canvas frame.
    #[test]
    fn merges_once_per_canvas_frame() {
        let (doc, id) = doc_with_layer();
        let (ev_tx, ev_rx) = channel();
        let pt = PaintThread::spawn(proj(64, 64), move |e| { if matches!(e, Event::Painted) { let _ = ev_tx.send(Instant::now()); } });
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(2.0)), limit: None, zoom: 1.0 });
        let (n, t0) = (200, Instant::now());
        for i in 0..n {
            let x = 4.0 + 56.0 * i as f32 / n as f32;
            pt.send(Cmd::Sample(PenSample::new(x, 32.0, 1.0, i as f64 * 0.001)));
            std::thread::sleep(Duration::from_millis(1));
        }
        pt.send(Cmd::End);
        drop(pt);
        let spent = t0.elapsed();
        let merges: Vec<Instant> = ev_rx.try_iter().collect();
        let frames = (spent.as_secs_f64() / canvas_frame().as_secs_f64()).ceil() as usize;
        assert!(merges.len() <= frames + 2, "{} merges for {n} samples in {spent:?}", merges.len());
        assert!(merges.len() >= 3 && merges.len() < n / 4, "{} merges", merges.len());
        assert_eq!(pixel(&doc, id, 58, 32), [0, 0, 0, 255], "the whole stroke painted");
    }

    /// Undo sent right after End undoes that stroke (same queue), then redo
    /// brings it back; a stroke stays on its layer when another is chosen.
    #[test]
    fn undo_follows_the_stroke_it_undoes() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        stroke(&pt, &doc, 20.0);
        stroke(&pt, &doc, 40.0);
        edit(&pt, &doc, Edit::Undo);
        drop(pt);
        assert_eq!(pixel(&doc, id, 30, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 30, 40)[3], 0);
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        edit(&pt, &doc, Edit::Redo);
        drop(pt);
        assert_eq!(pixel(&doc, id, 30, 40), [0, 0, 0, 255]);

        let other = doc.locked().apply(Edit::AddLayer("other".into())).made.unwrap();
        doc.locked().apply(Edit::SetActive(id));
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(4.0)), limit: None, zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(10.0, 50.0, 1.0, 0.0)));
        std::thread::sleep(Duration::from_millis(50));
        doc.locked().apply(Edit::SetActive(other));
        pt.send(Cmd::Sample(PenSample::new(50.0, 50.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        assert_eq!(pixel(&doc, id, 40, 50), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, other, 40, 50)[3], 0);
    }

    /// A Begin without End (window lost focus...) still records the
    /// unfinished stroke before starting the next one; a layer deleted
    /// mid-stroke stops it cleanly.
    #[test]
    fn unfinished_strokes_are_recorded_and_deleted_layers_stop_them() {
        let (doc, id) = doc_with_layer();
        let n = doc.locked().history().undo_len();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(4.0)), limit: None, zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(10.0, 10.0, 1.0, 0.0)));
        stroke(&pt, &doc, 40.0);
        drop(pt);
        assert_eq!(doc.locked().history().undo_len(), n + 2);

        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(brush(4.0)), limit: None, zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(10.0, 10.0, 1.0, 0.0)));
        std::thread::sleep(Duration::from_millis(50));
        {
            let mut d = doc.locked();
            d.apply(Edit::SetActive(id));
            assert!(d.apply(Edit::RemoveActive).changed());
        }
        pt.send(Cmd::Sample(PenSample::new(50.0, 10.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        let d = doc.locked();
        let paper = d.stack().root[0].id();
        assert_eq!(d.stack().image(paper).unwrap().pixel(30, 10), [255; 4], "nothing landed on the Paper");
    }

    /// Save goes through the queue: the file holds the strokes sent before.
    #[test]
    fn save_after_a_stroke_holds_it() {
        let (doc, id) = doc_with_layer();
        let (tx, rx) = channel();
        let pt = PaintThread::spawn(proj(64, 64), move |e| { if let Event::Saved { revision, result, .. } = e { let _ = tx.send((revision, result)); } });
        let path = std::env::temp_dir().join(format!("pt-save-{}.aurs", std::process::id()));
        stroke(&pt, &doc, 32.0);
        pt.send(Cmd::Save { doc: Arc::clone(&doc), path: path.clone(), origin: None, auto: false });
        let (revision, result) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        result.unwrap();
        assert_eq!(revision, doc.locked().revision());
        let opened = aurastudio_file::open(&path, 4096).unwrap();
        assert_eq!(opened.stack.image(id).unwrap().pixel(30, 32), [0, 0, 0, 255]);
        std::fs::remove_file(path).unwrap();
    }

    /// With a selection, strokes, fill and clear only change selected
    /// pixels.
    #[test]
    fn selection_limits_strokes_fill_and_clear() {
        let (doc, id) = doc_with_layer();
        let sel = |r: Rect| Edit::Select(aurastudio_document::SelectOp::Shape { shape: aurastudio_document::Shape::Rect(r), mode: aurastudio_document::SelectMode::Replace, aa: false, feather: 0.0 });
        doc.locked().apply(sel(Rect::new(0, 0, 30, 64)));
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        stroke(&pt, &doc, 20.0);
        let params = FillParams { refer: aurastudio_layers::Refer::Layer, ..Default::default() };
        edit(&pt, &doc, Edit::Fill { x: 5, y: 50, color: [255, 0, 0, 255], params });
        drop(pt);
        assert_eq!(pixel(&doc, id, 20, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 40, 20)[3], 0);
        assert_eq!(pixel(&doc, id, 5, 60), [255, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 50, 50)[3], 0);
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        edit(&pt, &doc, sel(Rect::new(0, 0, 10, 64)));
        edit(&pt, &doc, Edit::ClearActive);
        edit(&pt, &doc, Edit::Undo);
        edit(&pt, &doc, Edit::Redo);
        drop(pt);
        assert_eq!(pixel(&doc, id, 5, 60)[3], 0);
        assert_eq!(pixel(&doc, id, 20, 60), [255, 0, 0, 255]);
    }

    /// "Stay inside the lines": a stroke crossing the reference layer's
    /// lines stops at them; started on a line, it is free.
    #[test]
    fn strokes_stay_inside_the_lines() {
        let (doc, color) = doc_with_layer();
        let ink = {
            let mut d = doc.locked();
            let ink = d.apply(Edit::AddLayer("lines".into())).made.unwrap();
            // A wall at x = 30..33.
            d.stroke("wall", |s| {
                use aurastudio_core::PaintSurface;
                let r = Rect::new(30, 0, 33, 64);
                s.prepare(r);
                for y in 0..64 { let sp = s.span_mut(30, y, 3); for i in 0..sp.len() { sp.pixels[i] = [0, 0, 0, 255]; } }
            });
            d.apply(Edit::SetReference(ink, true));
            d.apply(Edit::SetActive(color));
            ink
        };
        let p = FillParams { tolerance: 32, expand: 0, ..Default::default() };
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(Brush { color: [200.0 / 255.0, 0.0, 0.0, 1.0], ..brush(4.0) }), limit: Some(p), zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(10.0, 20.0, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(55.0, 20.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        // Started on the line: free.
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: Box::new(Brush { color: [200.0 / 255.0, 0.0, 0.0, 1.0], ..brush(4.0) }), limit: Some(p), zoom: 1.0 });
        pt.send(Cmd::Sample(PenSample::new(31.0, 45.0, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(55.0, 45.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        assert_eq!(pixel(&doc, color, 20, 20), [200, 0, 0, 255]);
        assert_eq!(pixel(&doc, color, 45, 20)[3], 0, "stopped at the wall");
        assert_eq!(pixel(&doc, color, 45, 45)[3], 255, "free from the line");
        assert_eq!(pixel(&doc, ink, 31, 20), [0, 0, 0, 255]);
    }
}

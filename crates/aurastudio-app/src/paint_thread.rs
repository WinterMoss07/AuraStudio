//! The paint thread: pen samples queue up here and are painted in batches,
//! on this one thread (splitting a stroke was slower). Undo, clear, fill...
//! go through the same queue, after what was sent before them. It also
//! merges the changed areas for the screen (Krita), at least every
//! UPDATE_PERIOD.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aurastudio_forge::{Brush, Forge, ForgeBrush, PenSample};
use aurastudio_core::Rect;
use aurastudio_document::{Document, SharedDoc};
use aurastudio_gpu::PlayKey;
use aurastudio_layers::play::{PlayPart, PlayPlan, PlayTiles};

use crate::projection::{add_rect, union, SharedProjection};

/// Set by the interface while it waits for the document: between two
/// samples the paint thread lets it in (a big brush otherwise kept the
/// document locked nearly all the time, and the interface froze for up to
/// 400 ms, "UI waited … for the document").
pub static UI_WANTS_DOC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the paint thread holds back for the interface, at most.
const MAX_YIELD: Duration = Duration::from_millis(4);

/// Wait (briefly) while the interface asks for the document.
fn let_ui_in() {
    use std::sync::atomic::Ordering;
    if !UI_WANTS_DOC.load(Ordering::Acquire) { return; }
    let t = Instant::now();
    while UI_WANTS_DOC.load(Ordering::Acquire) && t.elapsed() < MAX_YIELD { std::thread::yield_now(); }
}

/// Shortest time between two projection updates while painting is behind
/// the pen (~120 fps; Krita's default limit is 100).
const UPDATE_PERIOD: Duration = Duration::from_millis(8);

pub enum Cmd {
    /// A stroke on the active layer; `limit`: stay inside the lines, the
    /// area these fill settings find from the start.
    Begin { doc: SharedDoc, brush: Brush, forge: Arc<ForgeBrush>, limit: Option<aurastudio_layers::FillParams> },
    /// Merge the whole canvas again (layer visibility, opacity, order,
    /// a filter layer's values...). Only the latest of those waiting is
    /// done.
    Recomposite(SharedDoc),
    /// Merge again only where the time changed the picture (sheets shown
    /// or hidden, ghosts): the rectangle the document keeps.
    RecompositeTime(SharedDoc),
    /// One pen sample.
    Sample(PenSample),
    End,
    Undo(SharedDoc),
    Redo(SharedDoc),
    /// Clear the active layer, or its selected pixels (undoable).
    Clear(SharedDoc),
    /// Tests: answered once everything sent before is done (and shown).
    #[cfg(test)]
    Ping(Sender<()>),
    /// Paste pixels on a new layer above the active one (undoable).
    Paste { doc: SharedDoc, clip: aurastudio_document::Clip },
    /// Fill tool at canvas pixel (x, y) on the active layer (undoable).
    Fill { doc: SharedDoc, x: u32, y: u32, color: [u8; 4], params: aurastudio_layers::FillParams },
    /// The filter dialog: show / keep / drop a filter on the active layer.
    /// Only the latest of the previews waiting is done.
    Filter { doc: SharedDoc, action: aurastudio_ui::FilterAction },
    /// A selection command (tools, Select menu), undoable.
    Select { doc: SharedDoc, op: aurastudio_document::SelectOp },
    /// The transform tool. Only the latest of the previews waiting is done.
    Transform { doc: SharedDoc, action: TransformAction },
    /// Save the document to `path` (after the strokes sent before). The
    /// file is written on its own thread; painting goes on meanwhile.
    /// `origin`: autosaves only, the file the work belongs to.
    Save { doc: SharedDoc, path: PathBuf, origin: Option<PathBuf>, auto: bool },
    /// Prepare animation playback from the GPU: the plan, then the tiles of
    /// each part one by one while nothing else waits (statics first, then
    /// the drawings from the current frame on). `gen` comes back with them.
    PlayBuild { doc: SharedDoc, gen: u64 },
}

/// What the transform tool asks.
#[derive(Clone, Debug)]
pub enum TransformAction {
    /// Start on these layers (the selection says what moves).
    Begin(Vec<u64>),
    /// Show it (the latest waiting only); the number comes back in
    /// TransformShown once it is on the projection.
    Preview(aurastudio_transform::Warp, aurastudio_transform::Interp, u64),
    Apply(aurastudio_transform::Warp, aurastudio_transform::Interp),
    Cancel,
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
    /// A layer's pixels changed for good (stroke done, undo...): refresh
    /// its thumbnail. `rect`: where (None: anywhere, or unknown).
    LayerChanged { layer_id: u64, rect: Option<Rect> },
    /// Short message for the options bar ("Nothing to undo"...).
    Notice(&'static str),
    /// How the document plays from the GPU (None: it can't, the merge
    /// shows each frame).
    PlayPlan { gen: u64, plan: Option<Box<PlayPlan>> },
    /// The tiles of one part of it.
    PlayTiles { gen: u64, key: PlayKey, tiles: Box<PlayTiles> },
    /// A save finished: the document revision now on disk, or the error.
    Saved { doc: aurastudio_document::DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String> },
}

pub struct PaintThread {
    tx:     Option<Sender<Cmd>>,
    handle: Option<JoinHandle<()>>,
}

impl PaintThread {
    /// `projection` receives the merged result of everything painted.
    pub fn spawn(projection: SharedProjection, notify: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(projection, Arc::new(notify));
        let handle = std::thread::Builder::new()
            .name("paint".into())
            .spawn(move || {
                // Its panic unwound (the document is free again): keep the
                // work before closing (rescue.rs).
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.run(rx))).is_err() {
                    #[cfg(not(test))]
                    crate::rescue::after_unwind();
                }
            })
            .expect("spawn paint thread");
        Self { tx: Some(tx), handle: Some(handle) }
    }

    pub fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }
}

impl Drop for PaintThread {
    fn drop(&mut self) {
        // Closing the channel ends the worker loop.
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The area a stroke starting at (x, y) on `layer` must stay in: the
/// region of the reference layers around it (all the other layers when
/// none is marked), as the fill tool finds it. None (free): started on a
/// line, off the canvas, or nothing around.
fn stay_inside(doc: &Document, layer: u64, x: f32, y: f32, p: aurastudio_layers::FillParams) -> Option<aurastudio_selection::Selection> {
    use aurastudio_layers::Pick;
    let (w, h) = (doc.layers.width, doc.layers.height);
    if !(x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32) { return None; }
    let (x, y) = (x as u32, y as u32);
    let reference = doc.layers.reference_for(layer);
    let refs = reference.is_some();
    let pick = Pick { skip_drafts: p.skip_drafts, reference, skip: layer, ..Pick::ALL };
    // On a line itself: free (CSP: the brush's center decides).
    let under = doc.layers.composite_pick((x, y, x + 1, y + 1), pick, true);
    if under.get(3).is_some_and(|a| *a >= 128) { return None; }
    let t = Instant::now();
    let area = aurastudio_layers::flood_with(&doc.layers, layer, x, y, p, doc.selection.as_ref(), Some(pick))?;
    log::info!("stay inside the lines: area {:?} found in {:.0} ms ({})", area.bbox, t.elapsed().as_secs_f64() * 1000.0,
        if refs { "its reference layer" } else { "all the other layers" });
    aurastudio_selection::Selection::from_predicate(w, h, area.bbox, |x, y| area.contains(x, y))
}

struct Worker {
    /// Document being worked on (last stroke / undo / clear).
    doc:      Option<SharedDoc>,
    projection: SharedProjection,
    /// Changed areas not merged into the projection yet.
    pending:  Vec<Rect>,
    last_update: Instant,
    notify:   Arc<dyn Fn(Event) + Send + Sync>,

    brush:    Brush,
    /// The stroke's engine, with the preset's brush.
    engine:   Option<Forge>,
    /// The stroke started on a paintable layer.
    active:   bool,
    /// Layer the stroke paints on, fixed at Begin (another may get selected
    /// while a big brush's backlog is painted).
    layer:    u64,
    /// Canvas size at Begin: a resize meanwhile ends the stroke.
    size:     (u32, u32),
    /// The stroke got a sample.
    sampled:  bool,
    /// Everything the stroke touched: the area saved for undo.
    stroke_rect: Option<Rect>,
    /// "Stay inside the lines": the settings, until the first sample
    /// finds the area; then the area (None: free, the stroke started on a
    /// line).
    limit_params: Option<aurastudio_layers::FillParams>,
    limit:    Option<aurastudio_selection::Selection>,
    stats:    Stats,
    /// Playback tiles still to make.
    play:     Option<PlayJob>,
}

/// The parts of a playback plan left to make.
struct PlayJob {
    doc:      SharedDoc,
    gen:      u64,
    plan:     PlayPlan,
    revision: u64,
    keys:     VecDeque<PlayKey>,
}

/// A document edit refused while a filter's preview (or a transform) is
/// shown on the layer: nothing else touches the document until it is
/// applied or not. One lock of the document.
fn refused(cmd: &Cmd) -> Option<(&'static str, SharedDoc)> {
    let (doc, filter) = match cmd {
        Cmd::Begin { doc, .. } | Cmd::Undo(doc) | Cmd::Redo(doc) | Cmd::Clear(doc) | Cmd::Fill { doc, .. } | Cmd::Paste { doc, .. } | Cmd::Select { doc, .. } => (doc, false),
        Cmd::Filter { doc, .. } => (doc, true),
        _ => return None,
    };
    let d = doc.lock().unwrap();
    let msg = if !filter && d.filtering() {
        "Apply or cancel the filter first"
    } else if d.transforming() {
        "Apply or cancel the transform first"
    } else {
        return None;
    };
    Some((msg, Arc::clone(doc)))
}

impl Worker {
    /// Start making the playback plan and its tiles.
    fn play_build(&mut self, doc: SharedDoc, gen: u64) {
        let (plan, revision, time) = {
            let d = doc.lock().unwrap();
            (d.layers.play_plan(), d.revision(), d.layers.time())
        };
        (self.notify)(Event::PlayPlan { gen, plan: plan.clone().map(Box::new) });
        self.play = plan.map(|plan| {
            let mut keys: VecDeque<PlayKey> = (0..plan.statics()).map(PlayKey::Static).collect();
            // The sheets in the order they come from the current frame.
            let mut drawings: Vec<(u32, u64)> = plan.parts.iter().flat_map(|p| match p {
                PlayPart::Anim { sheets, .. } => sheets.clone(),
                PlayPart::Static(_) => Vec::new(),
            }).collect();
            // (A held frame counts from the sheet it shows.)
            let start = plan.parts.iter().filter_map(|p| match p {
                PlayPart::Anim { sheets, .. } => sheets.iter().rev().find(|f| f.0 <= time).map(|f| f.0),
                PlayPart::Static(_) => None,
            }).min().unwrap_or(1);
            drawings.sort_by_key(|&(n, _)| (n < start, n));
            keys.extend(drawings.into_iter().map(|d| PlayKey::Drawing(d.1)));
            PlayJob { doc, gen, plan, revision, keys }
        });
    }

    /// Make the tiles of the next part (given up if the document changed).
    fn play_step(&mut self) {
        let Some(job) = &mut self.play else { return };
        let Some(key) = job.keys.pop_front() else { self.play = None; return };
        let tiles = {
            let mut d = job.doc.lock().unwrap();
            if d.revision() != job.revision { None } else {
                Some(match key {
                    PlayKey::Static(i) => d.layers.play_static(&job.plan, i),
                    PlayKey::Drawing(id) => d.layers.play_drawing(&job.plan, id),
                })
            }
        };
        let gen = job.gen;
        if job.keys.is_empty() { self.play = None; }
        match tiles {
            Some(tiles) => (self.notify)(Event::PlayTiles { gen, key, tiles: Box::new(tiles) }),
            None => self.play = None,
        }
        let_ui_in();
    }

    fn new(projection: SharedProjection, notify: Arc<dyn Fn(Event) + Send + Sync>) -> Self {
        Self {
            doc: None, projection, pending: Vec::new(), last_update: Instant::now(), notify,
            brush: Brush::new(),
            engine: None,
            active: false,
            layer: 0,
            size: (0, 0),
            sampled: false,
            stroke_rect: None,
            limit_params: None,
            limit: None,
            stats: Stats::default(),
            play: None,
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let mut queue = VecDeque::new();
        loop {
            // Sleep until there is work, then take everything queued.
            if queue.is_empty() {
                // Playback tiles are made while nothing else waits.
                let cmd = if self.play.is_some() {
                    match rx.try_recv() {
                        Ok(cmd) => Ok(cmd),
                        Err(TryRecvError::Empty) => { self.play_step(); continue; }
                        Err(TryRecvError::Disconnected) => return,
                    }
                } else {
                    rx.recv().map_err(|_| ())
                };
                match cmd {
                    Ok(cmd) => queue.push_back(cmd),
                    Err(_) => return,
                }
            }
            queue.extend(rx.try_iter());
            self.stats.max_queue = self.stats.max_queue.max(queue.len());

            let mut events = Vec::new();
            let mut flush = false;
            let t0 = Instant::now();
            while let Some(cmd) = queue.pop_front() {
                if let Some((msg, doc)) = refused(&cmd) {
                    events.push(Event::Notice(msg));
                    self.doc = Some(doc);
                    continue;
                }
                match cmd {
                    Cmd::Filter { action: aurastudio_ui::FilterAction::Preview(_), .. }
                        if queue.iter().any(|c| matches!(c, Cmd::Filter { .. })) => {}
                    // Never under a stroke (its snapshot would no longer
                    // match the layer).
                    Cmd::Transform { action: TransformAction::Begin(_), .. } if self.active => {
                        events.push(Event::Notice("Finish the stroke first"));
                    }
                    Cmd::Transform { action: TransformAction::Preview(..), .. }
                        if queue.iter().any(|c| matches!(c, Cmd::Transform { .. })) => {}
                    Cmd::Transform { doc, action } => {
                        let t = Instant::now();
                        let mut d = doc.lock().unwrap();
                        let name = match &action { TransformAction::Begin(_) => "begin", TransformAction::Preview(..) => "preview", TransformAction::Apply(..) => "apply", TransformAction::Cancel => "cancel" };
                        match action {
                            TransformAction::Begin(ids) => {
                                let r = d.transform_begin(&ids);
                                let ok = r.is_ok();
                                events.push(Event::TransformStarted(r));
                                if ok {
                                    let t = Instant::now();
                                    if let Some(l) = d.transform_lift() { events.push(Event::TransformLift(Box::new(l))); }
                                    log::info!("transform lift: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
                                }
                            }
                            TransformAction::Preview(w, i, n) => {
                                if let Some(r) = d.transform_preview(&w, i) { add_rect(&mut self.pending, r); }
                                events.push(Event::TransformShown(n));
                            }
                            TransformAction::Apply(w, i) => {
                                let had = d.selection.clone();
                                if let Some((ids, r)) = d.transform_apply(&w, i) {
                                    if r.2 > r.0 { add_rect(&mut self.pending, r); }
                                    for id in ids { events.push(Event::LayerChanged { layer_id: id, rect: (r.2 > r.0).then_some(r) }); }
                                }
                                if d.selection != had { events.push(Event::Selection); }
                            }
                            TransformAction::Cancel => {
                                if let Some((ids, r)) = d.transform_cancel() {
                                    if r.2 > r.0 { add_rect(&mut self.pending, r); }
                                    for id in ids { events.push(Event::LayerChanged { layer_id: id, rect: (r.2 > r.0).then_some(r) }); }
                                }
                            }
                        }
                        drop(d);
                        log::info!("transform {name}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
                        self.doc = Some(doc);
                        flush = true;
                    }
                    Cmd::Select { doc, op } => {
                        let t = Instant::now();
                        let changed = doc.lock().unwrap().select(&op);
                        if t.elapsed() > Duration::from_millis(30) { log::info!("{}: {:.0} ms", op.label(), t.elapsed().as_secs_f64() * 1000.0); }
                        if changed { events.push(Event::Selection); }
                        self.doc = Some(doc);
                    }
                    Cmd::Recomposite(_) if queue.iter().any(|c| matches!(c, Cmd::Recomposite(_))) => {}
                    Cmd::Recomposite(doc) => self.recomposite(doc, false),
                    // (What a later one will merge is enough: the rectangle
                    // is kept in the document, and adds up meanwhile.)
                    Cmd::RecompositeTime(_) if queue.iter().any(|c| matches!(c, Cmd::Recomposite(_) | Cmd::RecompositeTime(_))) => {}
                    Cmd::RecompositeTime(doc) => self.recomposite(doc, true),
                    Cmd::PlayBuild { doc, gen } => self.play_build(doc, gen),
                    Cmd::Filter { doc, action } => {
                        use aurastudio_ui::FilterAction as F;
                        let t = Instant::now();
                        let (done, keep) = {
                            let mut d = doc.lock().unwrap();
                            match &action {
                                F::Preview(f) => (d.filter_preview(f.as_ref()), false),
                                F::Apply(f) => (d.filter_apply(f), true),
                                F::Cancel => (d.filter_cancel(), true),
                            }
                        };
                        log::info!("filter {:?}: {:.0} ms", match &action { F::Preview(f) => f.as_ref().map(|f| f.kind.name()), F::Apply(f) => Some(f.kind.name()), F::Cancel => None },
                            t.elapsed().as_secs_f64() * 1000.0);
                        match done {
                            Some((id, r)) => {
                                if r.2 > r.0 { add_rect(&mut self.pending, r); }
                                if keep { events.push(Event::LayerChanged { layer_id: id, rect: (r.2 > r.0).then_some(r) }); }
                            }
                            None if !matches!(action, F::Cancel) => events.push(Event::Notice("Layer is locked or is a folder")),
                            None => {}
                        }
                        self.doc = Some(doc);
                        flush = true;
                    }
                    Cmd::Begin { doc, brush, forge, limit } => {
                        // A stroke still open (its End never came, e.g. the
                        // window lost focus): finish it first, so it is
                        // recorded for undo like any other.
                        if self.active { self.finish_stroke(&mut events); }
                        self.doc = Some(doc);
                        self.limit_params = limit;
                        self.limit = None;
                        if self.with_doc(|w, d| w.begin(d, brush, &forge)) == Some(false) {
                            events.push(Event::Notice("Layer is locked or is a folder"));
                        }
                    }
                    Cmd::Sample(p) => {
                        let r = self.with_doc(|w, d| w.sample(d, p)).flatten();
                        if let Some(r) = r {
                            add_rect(&mut self.pending, r);
                            self.stroke_rect = Some(self.stroke_rect.map_or(r, |s| union(s, r)));
                        }
                    }
                    Cmd::End => {
                        self.finish_stroke(&mut events);
                        // Show the end of the stroke right away.
                        flush = true;
                    }
                    Cmd::Save { doc, path, origin, auto } => {
                        // Locked only to copy the layers; compressed and
                        // written on a thread of its own.
                        let (snap, doc_id) = {
                            let d = doc.lock().unwrap();
                            (aurastudio_file::snapshot(&d, origin.as_deref()), d.id())
                        };
                        let notify = Arc::clone(&self.notify);
                        let spawned = std::thread::Builder::new().name("save".into()).spawn(move || {
                            let t = Instant::now();
                            let result = aurastudio_file::save(&snap, &path).map_err(|e| e.to_string());
                            log::info!("{} {}: {:?} in {:.0} ms", if auto { "autosave" } else { "save" },
                                path.display(), result, t.elapsed().as_secs_f64() * 1000.0);
                            notify(Event::Saved { doc: doc_id, path, revision: snap.revision, auto, result });
                        });
                        if let Err(e) = spawned { log::error!("save thread: {e}"); }
                    }
                    Cmd::Undo(_) | Cmd::Redo(_) | Cmd::Clear(_) | Cmd::Fill { .. } | Cmd::Paste { .. } if self.active => {
                        // Never while a stroke is open (the main thread
                        // doesn't send them then; this is a safety net).
                        events.push(Event::Notice("Finish the stroke first"));
                    }
                    Cmd::Undo(doc) => {
                        let c = doc.lock().unwrap().undo();
                        self.history_step(doc, c, "Nothing to undo", &mut events);
                        flush = true;
                    }
                    Cmd::Redo(doc) => {
                        let c = doc.lock().unwrap().redo();
                        self.history_step(doc, c, "Nothing to redo", &mut events);
                        flush = true;
                    }
                    Cmd::Clear(doc) => {
                        let cleared = doc.lock().unwrap().clear_active();
                        self.report(cleared, "Layer is locked or is a folder", &mut events);
                        self.doc = Some(doc);
                        flush = true;
                    }
                    #[cfg(test)]
                    Cmd::Ping(done) => {
                        if !self.pending.is_empty() { self.update_projection(); }
                        let _ = done.send(());
                    }
                    Cmd::Paste { doc, clip } => {
                        let pasted = doc.lock().unwrap().paste(&clip);
                        self.report(pasted, "Nothing to paste", &mut events);
                        self.doc = Some(doc);
                        flush = true;
                    }
                    Cmd::Fill { doc, x, y, color, params } => {
                        let t = Instant::now();
                        let filled = {
                            let mut d = doc.lock().unwrap();
                            let f = d.layers.can_paint().then(|| d.fill(x, y, color, params));
                            if d.take_reshown() { add_rect(&mut self.pending, (0, 0, d.layers.width, d.layers.height)); }
                            f
                        };
                        log::info!("fill: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
                        match filled {
                            Some(f) => self.report(f, "Outside the selection", &mut events),
                            None => events.push(Event::Notice("Layer is locked or is a folder")),
                        }
                        self.doc = Some(doc);
                        flush = true;
                    }
                }
                if flush || t0.elapsed() >= UPDATE_PERIOD { break; }
            }

            // Update the projection when caught up with the pen (lowest
            // latency), when asked to, or at least every UPDATE_PERIOD.
            queue.extend(rx.try_iter());
            let due = flush || queue.is_empty() || self.last_update.elapsed() >= UPDATE_PERIOD;
            if due && !self.pending.is_empty() {
                self.update_projection();
                (self.notify)(Event::Painted);
            }
            for e in events {
                (self.notify)(e);
            }
        }
    }

    /// Merge the whole canvas again (filter layers make it slow: off the
    /// UI thread), by bands, the document locked for one at a time (the UI
    /// runs between two), each shown as it comes. `time_only`: just where
    /// the time changed the picture (nothing at all if nowhere).
    fn recomposite(&mut self, doc: SharedDoc, time_only: bool) {
        let t = Instant::now();
        let (w, h, bands) = {
            let mut d = doc.lock().unwrap();
            d.layers.drop_stroke_cache();
            let (w, h) = (d.layers.width, d.layers.height);
            let dirty = d.layers.take_time_dirty();
            let rect = if time_only {
                dirty.map(|r| (r.0.min(w), r.1.min(h), r.2.min(w), r.3.min(h))).filter(|r| r.2 > r.0 && r.3 > r.1)
            } else {
                Some((0, 0, w, h))
            };
            (w, h, rect.map(|r| d.layers.merge_bands(r, 128)).unwrap_or_default())
        };
        // (What was waiting to be shown is part of a whole merge.)
        if !time_only { self.pending.clear(); }
        for r in bands {
            let rgba = {
                let d = doc.lock().unwrap();
                if (d.layers.width, d.layers.height) != (w, h) { break; }
                d.layers.composite_rect(r)
            };
            let mut p = self.projection.lock().unwrap();
            if (p.width, p.height) != (w, h) { break; }
            p.push(r, rgba);
            drop(p);
            (self.notify)(Event::Painted);
            // Room for the UI to take the document (a mutex isn't fair:
            // locking again at once, this thread would get it first), only
            // when it is waiting: no fixed sleep per band.
            let_ui_in();
        }
        self.doc = Some(doc);
        log::debug!("recomposite{} {w}x{h}: {:.0} ms", if time_only { " (time)" } else { "" }, t.elapsed().as_secs_f64() * 1000.0);
    }

    /// An undo / redo done: show what changed (a layer step: everything).
    fn history_step(&mut self, doc: SharedDoc, c: Option<aurastudio_document::Change>, none_msg: &'static str, events: &mut Vec<Event>) {
        let Some(c) = c else {
            events.push(Event::Notice(none_msg));
            self.doc = Some(doc);
            return;
        };
        if c.selection { events.push(Event::Selection); }
        if c.tree {
            self.recomposite(doc, false);
            // (The tree changed: any layer may look different.)
            events.push(Event::LayerChanged { layer_id: c.layer_id, rect: None });
            for (id, _) in c.more { events.push(Event::LayerChanged { layer_id: id, rect: None }); }
            return;
        }
        for (id, r) in std::iter::once((c.layer_id, c.rect)).chain(c.more) {
            if r.2 > r.0 && r.3 > r.1 {
                add_rect(&mut self.pending, r);
                events.push(Event::LayerChanged { layer_id: id, rect: Some(r) });
            }
        }
        self.doc = Some(doc);
    }

    /// End the open stroke (recorded for undo) and report its layer.
    fn finish_stroke(&mut self, events: &mut Vec<Event>) {
        if let Some((rect, whole, id)) = self.with_doc(Worker::end).flatten() {
            if let Some(r) = rect { add_rect(&mut self.pending, r); }
            events.push(Event::LayerChanged { layer_id: id, rect: whole });
        }
    }

    /// Merge the layers of the changed areas into the projection.
    fn update_projection(&mut self) {
        self.last_update = Instant::now();
        let rects = std::mem::take(&mut self.pending);
        let Some(doc) = self.doc.clone() else { return };
        let mut d = doc.lock().unwrap();
        // Merged outside the projection lock (the UI only waits for the
        // handover); mid-stroke from the cached merges (stroke_cache.rs).
        let (w, h) = (d.layers.width, d.layers.height);
        // A filter layer reading far around (a big blur): each piece would
        // read and filter its whole margin again; one piece over them all
        // does it once.
        let rects = if rects.len() > 1 && d.layers.merge_margin() > 16 {
            vec![rects.into_iter().reduce(union).unwrap()]
        } else {
            rects
        };
        let patches: Vec<_> = rects.into_iter().map(|r| (r, d.layers.composite_rect_stroke(r))).collect();
        let mut p = self.projection.lock().unwrap();
        if (p.width, p.height) != (w, h) { return; }
        for (r, rgba) in patches {
            p.push((r.0, r.1, r.2.min(w), r.3.min(h)), rgba);
        }
    }

    /// Run `f` with the document locked (per command: the main thread gets
    /// it between two samples).
    fn with_doc<R>(&mut self, f: impl FnOnce(&mut Self, &mut Document) -> R) -> Option<R> {
        let doc = self.doc.clone()?;
        let_ui_in();
        let mut d = doc.lock().unwrap();
        Some(f(self, &mut d))
    }

    /// An edit of layer `id` in `rect` to show, or `none_msg`.
    fn report(&mut self, edit: Option<(u64, Rect)>, none_msg: &'static str, events: &mut Vec<Event>) {
        match edit {
            Some((id, r)) => {
                add_rect(&mut self.pending, r);
                events.push(Event::LayerChanged { layer_id: id, rect: Some(r) });
            }
            None => events.push(Event::Notice(none_msg)),
        }
    }

    /// Start a stroke on the active layer. False if it can't be painted
    /// (locked, or a folder is selected).
    fn begin(&mut self, doc: &mut Document, brush: Brush, forge: &ForgeBrush) -> bool {
        self.sampled = false;
        self.stroke_rect = None;
        self.stats = Stats::default();
        self.size = (doc.layers.width, doc.layers.height);
        // (In an animation folder: maybe a new sheet, made here.)
        let target = doc.begin_paint();
        self.layer = target.unwrap_or(0);
        self.active = target.is_some() && doc.layers.begin_stroke(self.layer);
        if !self.active { doc.drop_new_sheet(); }
        self.reshow(doc);
        if !self.active { return false; }

        let mut e = Forge::new(forge.clone());
        e.sync_from(&brush);
        e.begin_stroke();
        self.engine = Some(e);
        self.brush = brush;
        true
    }

    fn sample(&mut self, doc: &mut Document, now: PenSample) -> Option<Rect> {
        if !self.active { return None; }
        // The canvas was resized under the stroke: its snapshot no longer
        // matches the layer. Stop here (what was painted stays, undoable).
        if (doc.layers.width, doc.layers.height) != self.size {
            self.active = false;
            return None;
        }
        if let Some(p) = self.limit_params.take() {
            self.limit = stay_inside(doc, self.layer, now.x, now.y, p);
        }
        let t0 = Instant::now();
        let Some(mut surface) = doc.layers.layer_surface(self.layer) else {
            // The layer was deleted: nothing left to paint on.
            self.active = false;
            return None;
        };
        self.sampled = true;
        // Forge paints only the path over the canvas (zoomed out, the pen
        // can wander far off it).
        let rect = self.engine.as_mut()?.stroke_to(&mut surface, now);
        // Only selected pixels may change, and inside the lines.
        if let (Some(r), Some(sel)) = (rect, &doc.selection) { doc.layers.clip_stroke(r, sel); }
        if let (Some(r), Some(l)) = (rect, &self.limit) { doc.layers.clip_stroke(r, l); }
        self.stats.paint.add(t0.elapsed());
        rect
    }

    /// A sheet was made or taken back: the whole canvas merges again.
    fn reshow(&mut self, doc: &mut Document) {
        if doc.take_reshown() { add_rect(&mut self.pending, (0, 0, doc.layers.width, doc.layers.height)); }
    }

    /// Finish the stroke and record it for undo. Returns the last painted
    /// area, the area of the whole stroke and the layer id, or None if
    /// nothing was painted.
    fn end(&mut self, doc: &mut Document) -> Option<(Option<Rect>, Option<Rect>, u64)> {
        let was_active = std::mem::take(&mut self.active);
        // Also after an aborted stroke (layer deleted / canvas resized):
        // whatever was painted before is still recorded for undo.
        if !std::mem::take(&mut self.sampled) {
            // (A sheet made for nothing goes.)
            doc.drop_new_sheet();
            self.reshow(doc);
            return None;
        }
        let id = self.layer;
        let same_size = (doc.layers.width, doc.layers.height) == self.size;
        let mut rect = None;
        if was_active {
            if let (Some(mut surface), Some(e)) = (doc.layers.layer_surface(id), self.engine.as_mut()) {
                rect = e.end_stroke(&mut surface);
            }
        }
        if let (Some(r), Some(sel)) = (rect, &doc.selection) { doc.layers.clip_stroke(r, sel); }
        if let (Some(r), Some(l)) = (rect, &self.limit) { doc.layers.clip_stroke(r, l); }
        self.limit = None;
        self.limit_params = None;
        self.engine = None;
        if let Some(r) = rect { self.stroke_rect = Some(self.stroke_rect.map_or(r, |s| union(s, r))); }
        // After a resize the snapshot no longer matches: nothing to record.
        if !same_size { self.stroke_rect = None; }
        let whole = self.stroke_rect.take();
        if let Some(r) = whole {
            let t0 = Instant::now();
            doc.commit_stroke(id, r);
            log::info!("stroke: {} samples | paint {} | max queued {} | undo save {:.1} ms ({} KB history)",
                self.stats.paint.count, self.stats.paint, self.stats.max_queue,
                t0.elapsed().as_secs_f64() * 1000.0, doc.history().bytes() / 1024);
        } else {
            doc.drop_new_sheet();
        }
        self.reshow(doc);
        // The stroke buffers are freed until the next stroke.
        doc.layers.end_stroke();
        Some((rect, whole, id))
    }
}

#[derive(Default)]
struct Stats {
    paint:     Timing,
    max_queue: usize,
}

/// Count, average and worst duration, printed in the stroke log line.
#[derive(Default)]
pub struct Timing {
    pub count: u32,
    total: Duration,
    max:   Duration,
}

impl Timing {
    pub fn add(&mut self, d: Duration) {
        self.count += 1;
        self.total += d;
        self.max = self.max.max(d);
    }
}

impl std::fmt::Display for Timing {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.count == 0 { return write!(f, "-"); }
        let avg = self.total.as_secs_f64() * 1000.0 / self.count as f64;
        write!(f, "avg {avg:.2} ms, max {:.2} ms", self.max.as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_document::{DocId, Document};
    use aurastudio_layers::Node;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};
    use crate::projection::Projection;

    fn round() -> Arc<ForgeBrush> { Arc::new(ForgeBrush::round()) }

    fn proj(w: u32, h: u32) -> SharedProjection {
        Arc::new(Mutex::new(Projection::new(w, h)))
    }

    fn doc_with_layer() -> (SharedDoc, u64) {
        let mut d = Document::new(DocId(1), "t", 64, 64);
        let id = d.layers.add_layer("ink");
        (Arc::new(Mutex::new(d)), id)
    }

    fn pixel(doc: &SharedDoc, id: u64, x: u32, y: u32) -> [u8; 4] {
        let d = doc.lock().unwrap();
        let Some(Node::Layer(l)) = d.layers.node(id) else { panic!() };
        l.tiles.pixel(x, y)
    }

    fn stroke(pt: &PaintThread, doc: &SharedDoc, y: f32) {
        let mut brush = Brush::new();
        brush.radius = 4.0;
        pt.send(Cmd::Begin { doc: Arc::clone(doc), brush, forge: round(), limit: None });
        pt.send(Cmd::Sample(PenSample::new(10.0, y, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(50.0, y, 1.0, 0.01)));
        pt.send(Cmd::End);
    }

    /// A stroke sent to the thread ends up on the layer, and the thread
    /// reports both the painted area and the changed layer.
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
        // The projection got the merged result and its changed areas.
        drop(pt);
        let patches = projection.lock().unwrap().take_patches();
        let mut screen = vec![0u8; 64 * 64 * 4];
        for (r, rgba) in &patches { blit(&mut screen, 64, *r, rgba); }
        let i = ((32 * 64 + 30) * 4) as usize;
        assert_eq!(&screen[i..i + 4], &[0, 0, 0, 255]);
    }

    /// Undo sent right after End undoes that stroke (same queue), then redo
    /// brings it back.
    #[test]
    fn undo_follows_the_stroke_it_undoes() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        stroke(&pt, &doc, 20.0);
        stroke(&pt, &doc, 40.0);
        pt.send(Cmd::Undo(Arc::clone(&doc)));
        drop(pt); // joins: everything above is processed, in order
        assert_eq!(pixel(&doc, id, 30, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 30, 40)[3], 0);

        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        pt.send(Cmd::Redo(Arc::clone(&doc)));
        drop(pt);
        assert_eq!(pixel(&doc, id, 30, 40), [0, 0, 0, 255]);
    }

    /// Selecting another layer while the stroke is still queued must not
    /// move the rest of the stroke onto it (the stroke keeps its layer).
    #[test]
    fn stroke_stays_on_its_layer_when_selection_changes() {
        let (doc, id) = doc_with_layer();
        let other = doc.lock().unwrap().layers.add_layer("other");
        doc.lock().unwrap().layers.set_active(id);
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut brush = Brush::new();
        brush.radius = 4.0;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit: None });
        pt.send(Cmd::Sample(PenSample::new(10.0, 30.0, 1.0, 0.0)));
        std::thread::sleep(Duration::from_millis(50));
        // The user selects another layer while the stroke goes on.
        doc.lock().unwrap().layers.set_active(other);
        pt.send(Cmd::Sample(PenSample::new(50.0, 30.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        assert_eq!(pixel(&doc, id, 40, 30), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, other, 40, 30)[3], 0);
    }

    /// A Begin without End (window lost focus...) still records the
    /// unfinished stroke for undo before starting the next one.
    #[test]
    fn unfinished_stroke_is_recorded_for_undo() {
        let (doc, _) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut brush = Brush::new();
        brush.radius = 4.0;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: brush.clone(), forge: round(), limit: None });
        pt.send(Cmd::Sample(PenSample::new(10.0, 10.0, 1.0, 0.0)));
        stroke(&pt, &doc, 40.0);
        drop(pt);
        // (The new layer, then the two strokes.)
        assert_eq!(doc.lock().unwrap().history().undo_len(), 3);
    }

    /// Deleting the layer mid-stroke stops the stroke; what was painted
    /// before is still undoable and nothing lands on another layer.
    #[test]
    fn deleted_layer_ends_the_stroke_cleanly() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut brush = Brush::new();
        brush.radius = 4.0;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit: None });
        pt.send(Cmd::Sample(PenSample::new(10.0, 10.0, 1.0, 0.0)));
        std::thread::sleep(Duration::from_millis(50));
        {
            let mut d = doc.lock().unwrap();
            d.layers.set_active(id);
            assert!(d.layers.remove_active());
        }
        pt.send(Cmd::Sample(PenSample::new(50.0, 10.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        let d = doc.lock().unwrap();
        // Only the background is left, untouched.
        let Some(Node::Layer(bg)) = d.layers.node(1) else { panic!() };
        assert!(bg.tiles.to_rgba().chunks(4).all(|p| p == [255, 255, 255, 255]));
    }

    /// Zoomed out, a stroke can wander hundreds of thousands of pixels off
    /// the canvas: it pauses there and resumes where it comes back, with
    /// any engine, quickly (MyPaint brushes took seconds per sample).
    #[test]
    fn stroke_leaving_the_canvas_far_pauses_and_resumes() {
        let lib = aurastudio_forge_library::Library::load();
        let mypaint = lib.presets.iter().find(|p| p.name == "Marker").unwrap().brush.clone();
        for (n, forge) in [round(), mypaint].into_iter().enumerate() {
            let (doc, id) = doc_with_layer();
            let pt = PaintThread::spawn(proj(64, 64), |_| {});
            let mut brush = Brush::new();
            brush.radius = 3.0;
            let t = Instant::now();
            pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge, limit: None });
            // A few samples on the canvas (MyPaint brushes only paint once
            // the pen moves), far away, back step by step (they fade at
            // high speed), a few more.
            let on = |y: f32| (0..6).map(move |i| (10.0 + i as f32 * 4.0, y));
            let path: Vec<(f32, f32)> = on(10.0)
                .chain([(400_000.0, -300_000.0), (-250_000.0, 900_000.0), (-2000.0, 50.0), (-600.0, 50.0),
                        (-300.0, 50.0), (-150.0, 50.0), (-60.0, 50.0), (-20.0, 50.0)])
                .chain(on(50.0)).collect();
            for (i, (x, y)) in path.into_iter().enumerate() {
                pt.send(Cmd::Sample(PenSample::new(x, y, 1.0, i as f64 * 0.01)));
            }
            pt.send(Cmd::End);
            drop(pt);
            assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
            assert!(pixel(&doc, id, 20, 10)[3] > 0, "start not painted (engine {n})");
            assert!(pixel(&doc, id, 20, 50)[3] > 0, "end not painted (engine {n})");
            // (The new layer, one stroke.)
            assert_eq!(doc.lock().unwrap().history().undo_len(), 2);
        }
    }

    /// Save goes through the queue: the file holds the strokes sent before
    /// it, and the thread reports the revision it saved.
    #[test]
    fn save_after_a_stroke_holds_it() {
        let (doc, id) = doc_with_layer();
        let (tx, rx) = channel();
        let pt = PaintThread::spawn(proj(64, 64), move |e| {
            if let Event::Saved { revision, result, .. } = e { let _ = tx.send((revision, result)); }
        });
        let path = std::env::temp_dir().join(format!("pt-save-{}.aurs", std::process::id()));
        stroke(&pt, &doc, 32.0);
        pt.send(Cmd::Save { doc: Arc::clone(&doc), path: path.clone(), origin: None, auto: false });
        let (revision, result) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        result.unwrap();
        assert_eq!(revision, doc.lock().unwrap().revision());
        let opened = aurastudio_file::open(&path, DocId(9), 4096).unwrap().doc;
        let Some(Node::Layer(l)) = opened.layers.node(id) else { panic!() };
        assert_eq!(l.tiles.pixel(30, 32), [0, 0, 0, 255]);
        std::fs::remove_file(path).unwrap();
    }

    /// With a selection, strokes, fill and clear only change selected
    /// pixels.
    #[test]
    fn selection_limits_strokes_fill_and_clear() {
        let (doc, id) = doc_with_layer();
        doc.lock().unwrap().selection = aurastudio_selection::Selection::rect(64, 64, (0, 0, 30, 64));
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        stroke(&pt, &doc, 20.0);
        drop(pt);
        assert_eq!(pixel(&doc, id, 20, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 40, 20)[3], 0);
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let params = aurastudio_layers::FillParams { refer: aurastudio_layers::Refer::Layer, ..Default::default() };
        pt.send(Cmd::Fill { doc: Arc::clone(&doc), x: 5, y: 50, color: [255, 0, 0, 255], params });
        drop(pt);
        assert_eq!(pixel(&doc, id, 5, 60), [255, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 50, 50)[3], 0);
        assert_eq!(pixel(&doc, id, 20, 20), [0, 0, 0, 255], "the fill went over the line");
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        doc.lock().unwrap().selection = aurastudio_selection::Selection::rect(64, 64, (0, 0, 10, 64));
        pt.send(Cmd::Clear(Arc::clone(&doc)));
        pt.send(Cmd::Undo(Arc::clone(&doc)));
        pt.send(Cmd::Redo(Arc::clone(&doc)));
        drop(pt);
        assert_eq!(pixel(&doc, id, 5, 60)[3], 0);
        assert_eq!(pixel(&doc, id, 20, 60), [255, 0, 0, 255]);
    }

    /// "Stay inside the lines": a stroke crossing the reference layer's
    /// wall stops at it; started on the wall, it is free; the recomposite
    /// command shows the whole canvas.
    #[test]
    fn strokes_stay_inside_the_reference_lines() {
        let (doc, color) = doc_with_layer();
        let ink = {
            let mut d = doc.lock().unwrap();
            let ink = d.layers.add_layer("lines");
            d.layers.set_layer_rect(ink, (30, 0, 33, 64), &[0u8, 0, 0, 255].repeat(3 * 64));
            d.layers.node_mut(ink).unwrap().props_mut().reference = true;
            d.layers.set_active(color);
            ink
        };
        let projection = proj(64, 64);
        let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});
        let limit = Some(aurastudio_layers::FillParams { expand: 1, ..Default::default() });
        let mut brush = Brush::new();
        brush.radius = 4.0;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: brush.clone(), forge: round(), limit });
        pt.send(Cmd::Sample(PenSample::new(10.0, 20.0, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(55.0, 20.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        // Started on the line: free.
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit });
        pt.send(Cmd::Sample(PenSample::new(31.0, 45.0, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(55.0, 45.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        pt.send(Cmd::Recomposite(Arc::clone(&doc)));
        drop(pt);
        assert_eq!(pixel(&doc, color, 20, 20)[3], 255, "painted on its side");
        assert_eq!(pixel(&doc, color, 45, 20)[3], 0, "not past the line");
        assert_eq!(pixel(&doc, color, 45, 45)[3], 255, "free from the line");
        assert_eq!(pixel(&doc, ink, 31, 20), [0, 0, 0, 255]);
        let p = projection.lock().unwrap().take_patches();
        assert!(p.iter().any(|(r, _)| r.1 == 0 && r.2 == 64), "the whole canvas shown again");
    }

    /// The same over a painted picture and under a filter layer (the
    /// picture isn't a reference: it doesn't count).
    #[test]
    fn stay_inside_over_a_picture_and_a_filter_layer() {
        let doc: SharedDoc = Arc::new(Mutex::new(Document::new(DocId(1), "t", 64, 64)));
        let (ink, color) = {
            let mut d = doc.lock().unwrap();
            let pic = d.layers.add_layer("picture");
            let px: Vec<u8> = (0..64 * 64).flat_map(|i| [(i % 64 * 4) as u8, 120, 200, 255]).collect();
            d.layers.set_layer_rect(pic, (0, 0, 64, 64), &px);
            d.layers.add_filter_layer(aurastudio_filters::Filter::new(aurastudio_filters::Kind::Invert));
            let ink = d.layers.add_layer("lines");
            d.layers.set_layer_rect(ink, (30, 0, 33, 64), &[0u8, 0, 0, 255].repeat(3 * 64));
            d.layers.node_mut(ink).unwrap().props_mut().reference = true;
            let color = d.layers.add_layer("color");
            (ink, color)
        };
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut brush = Brush::new();
        brush.radius = 4.0;
        let limit = Some(aurastudio_layers::FillParams { expand: 1, ..Default::default() });
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit });
        pt.send(Cmd::Sample(PenSample::new(10.0, 20.0, 1.0, 0.0)));
        pt.send(Cmd::Sample(PenSample::new(55.0, 20.0, 1.0, 0.01)));
        pt.send(Cmd::End);
        drop(pt);
        assert_eq!(pixel(&doc, color, 20, 20)[3], 255);
        assert_eq!(pixel(&doc, color, 45, 20)[3], 0, "not past the line");
        let _ = ink;
    }

    /// How long a stroke takes to show under a blur filter layer (A4).
    ///     cargo test --release -p aurastudio blur_layer_stroke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn blur_layer_stroke_timing() {
        let doc: SharedDoc = Arc::new(Mutex::new(Document::new(DocId(1), "t", 2480, 3508)));
        let paint = {
            let mut d = doc.lock().unwrap();
            let paint = d.layers.active_id();
            let sigma: f32 = std::env::var("SIGMA").ok().and_then(|s| s.parse().ok()).unwrap_or(30.0);
            d.layers.add_filter_layer(aurastudio_filters::Filter { values: vec![sigma], ..aurastudio_filters::Filter::new(aurastudio_filters::Kind::GaussianBlur) });
            d.layers.set_active(paint);
            paint
        };
        let projection = proj(2480, 3508);
        let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});
        let t = Instant::now();
        let mut brush = Brush::new();
        brush.radius = 25.0;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit: None });
        for i in 0..400 {
            pt.send(Cmd::Sample(PenSample::new(300.0 + i as f32 * 4.0, 1000.0 + (i as f32 * 0.05).sin() * 300.0, 1.0, i as f64 * 0.005)));
        }
        pt.send(Cmd::End);
        drop(pt);
        println!("1600 px stroke under a blur layer: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(pixel(&doc, paint, 300, 1000)[3], 255);
    }

    /// A locked layer is never painted, even if a stroke is sent.
    #[test]
    fn locked_layer_stays_untouched() {
        let (doc, id) = doc_with_layer();
        *doc.lock().unwrap().layers.node_mut(id).unwrap().locked_mut() = true;
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        stroke(&pt, &doc, 16.0);
        drop(pt);
        let d = doc.lock().unwrap();
        let Some(Node::Layer(l)) = d.layers.node(id) else { panic!() };
        assert_eq!(l.tiles.stored(), 0);
        assert_eq!(d.history().undo_len(), 0);
    }

    /// Headless model of the app while painting: pen samples at 200 Hz,
    /// a "GUI" thread at 250 fps that shows the painted area and holds the
    /// document ~1.5 ms per frame to build the UI. Prints how far the
    /// painting lags behind the pen and how long GUI frames take.
    /// cargo test --release -p aurastudio -- --ignored pipeline --nocapture
    #[test]
    #[ignore]
    fn pipeline_benchmark() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for radius in [50u32, 250, 500] {
            let mut d = Document::new(DocId(1), "b", 2048, 2048);
            d.layers.add_layer("ink");
            let doc: SharedDoc = Arc::new(Mutex::new(d));
            let projection = proj(2048, 2048);
            let pt = Arc::new(PaintThread::spawn(Arc::clone(&projection), |_| {}));
            let stop = Arc::new(AtomicBool::new(false));

            let gui = {
                let (doc, projection, stop) = (Arc::clone(&doc), Arc::clone(&projection), Arc::clone(&stop));
                std::thread::spawn(move || {
                    let mut screen = vec![0u8; 2048 * 2048 * 4];
                    let mut frames = Timing::default();
                    while !stop.load(Ordering::Relaxed) {
                        let t = Instant::now();
                        gui_show_painted(&projection, &mut screen);
                        // UI build: only when the document is free (the app
                        // reuses the last UI otherwise).
                        if let Ok(_ui) = doc.try_lock() {
                            let u = Instant::now();
                            while u.elapsed() < Duration::from_micros(1500) {}
                        }
                        frames.add(t.elapsed());
                        std::thread::sleep(Duration::from_millis(4));
                    }
                    frames
                })
            };

            let mut brush = Brush::new();
            brush.radius = radius as f32;
            pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, forge: round(), limit: None });
            let t0 = Instant::now();
            let n = 200;
            for i in 0..n {
                let t = i as f32 / n as f32;
                let x = 300.0 + i as f32 * 7.0;
                let y = 1000.0 + (t * 12.0).sin() * 400.0;
                pt.send(Cmd::Sample(PenSample::new(x, y, 0.3 + 0.7 * (t * 9.0).sin().abs(), i as f64 * 0.005)));
                let due = t0 + Duration::from_millis(5 * (i as u64 + 1));
                while Instant::now() < due { std::thread::sleep(Duration::from_micros(200)); }
            }
            let pen_up = Instant::now();
            pt.send(Cmd::End);
            while doc.lock().unwrap().history().undo_len() == 0 {
                std::thread::sleep(Duration::from_micros(200));
            }
            let lag = pen_up.elapsed();
            stop.store(true, Ordering::Relaxed);
            let frames = gui.join().unwrap();
            println!("radius {radius:>3}: stroke done {:>6.1} ms after pen up | gui frames {}",
                lag.as_secs_f64() * 1000.0, frames);
        }
    }

    /// GUI side of the benchmark: copy the finished rectangles (stands for the
    /// GPU upload, which copies them into wgpu's staging memory).
    fn gui_show_painted(projection: &SharedProjection, screen: &mut [u8]) {
        let (w, patches) = {
            let mut p = projection.lock().unwrap();
            (p.width, p.take_patches())
        };
        for (r, rgba) in &patches { blit(screen, w, *r, rgba); }
    }

    /// Copy a merged piece into a canvas-sized RGBA buffer (what the GPU
    /// upload does with the texture).
    fn blit(screen: &mut [u8], width: u32, r: aurastudio_core::Rect, rgba: &[u8]) {
        let rw = (r.2 - r.0) as usize * 4;
        for (y, row) in (r.1..r.3).zip(rgba.chunks_exact(rw)) {
            let a = (y as usize * width as usize + r.0 as usize) * 4;
            screen[a..a + rw].copy_from_slice(row);
        }
    }
}

#[cfg(test)]
mod ui_turn_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// The paint thread holds back while the interface asks, for a few
    /// milliseconds at most, and not at all otherwise.
    #[test]
    fn the_paint_thread_lets_the_interface_in_briefly() {
        let t = Instant::now();
        let_ui_in();
        assert!(t.elapsed() < Duration::from_millis(1), "no wait when nobody asks");
        UI_WANTS_DOC.store(true, Ordering::Release);
        let t = Instant::now();
        let_ui_in();
        let waited = t.elapsed();
        UI_WANTS_DOC.store(false, Ordering::Release);
        assert!(waited >= MAX_YIELD && waited < MAX_YIELD * 10, "{waited:?}");
        // Released as soon as the interface has it.
        UI_WANTS_DOC.store(true, Ordering::Release);
        let release = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_micros(500));
            UI_WANTS_DOC.store(false, Ordering::Release);
        });
        let t = Instant::now();
        let_ui_in();
        release.join().unwrap();
        assert!(t.elapsed() < MAX_YIELD, "{:?}", t.elapsed());
    }
}

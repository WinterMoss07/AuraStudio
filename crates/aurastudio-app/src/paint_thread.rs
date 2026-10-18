//! Brush painting on its own thread.
//!
//! The main thread only reads the pen, forwards samples here and displays.
//! Before, a big brush (3-10 ms per sample at 1000 px) blocked the event
//! loop: pen packets and UI frames waited behind the painting. Now samples
//! queue up while one is being painted, and are painted in batches.
//!
//! Rule: a brush always paints on this ONE thread. Never split a stroke or
//! a dab across threads (rayon, tiles...): the pieces must be gathered and
//! reconciled afterwards, which costs more than it saves. Measured here:
//! a parallel round pen made every stroke slower, dab engines included. This
//! thread exists to keep painting off the input / UI thread, not to split it.
//!
//! Undo, redo and clear go through the same queue, so they always apply
//! after the samples sent before them: Ctrl+Z right after lifting the pen
//! undoes that stroke, even if it is still being painted.
//!
//! Display pipeline, modeled on Krita (FreehandStrokeStrategy +
//! KisCanvas2): after painting, this thread also merges the layers of the
//! changed areas into the projection (Krita does that merge on worker
//! threads too, never on the GUI thread). The UI thread only uploads the
//! finished rectangles to the GPU. Changed areas travel as a list of
//! rectangles, never one ever-growing bounding box.
//!
//! When painting keeps up with the pen, the projection is updated after
//! every batch (lowest latency). When it falls behind (huge brush), it is
//! updated at most every UPDATE_PERIOD, so the time goes to painting.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aurastudio_brush::{Brush, DabEngine, MyPaintEngine};
use aurastudio_brushes::Engine;
use aurastudio_canvas::Rect;
use aurastudio_document::{Document, SharedDoc};

use crate::projection::{add_rect, union, SharedProjection};

/// Shortest time between two projection updates while painting is behind
/// the pen (~120 fps; Krita's default limit is 100).
const UPDATE_PERIOD: Duration = Duration::from_millis(8);

pub enum Cmd {
    /// Start a stroke on the active layer of `doc` with these settings and
    /// this engine (from the selected brush preset).
    Begin { doc: SharedDoc, brush: Brush, engine: Engine },
    /// One pen sample: canvas coordinates, pressure, pen time in seconds.
    Sample { x: f32, y: f32, pressure: f32, time: f64 },
    End,
    Undo(SharedDoc),
    Redo(SharedDoc),
    /// Clear the active layer (undoable).
    Clear(SharedDoc),
}

/// What the worker reports to the main thread (through `notify`).
pub enum Event {
    /// The projection has new changed areas to upload.
    Painted,
    /// A layer's pixels changed for good (stroke done, undo...): refresh
    /// its thumbnail.
    LayerChanged { layer_id: u64 },
    /// Short message for the options bar ("Nothing to undo"...).
    Notice(&'static str),
}

pub struct PaintThread {
    tx:     Option<Sender<Cmd>>,
    handle: Option<JoinHandle<()>>,
}

impl PaintThread {
    /// `projection` receives the merged result of everything painted.
    pub fn spawn(projection: SharedProjection, notify: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(projection, Box::new(notify));
        let handle = std::thread::Builder::new()
            .name("paint".into())
            .spawn(move || worker.run(rx))
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

struct Worker {
    /// Document being worked on (last stroke / undo / clear).
    doc:      Option<SharedDoc>,
    projection: SharedProjection,
    /// Changed areas not merged into the projection yet.
    pending:  Vec<Rect>,
    last_update: Instant,
    notify:   Box<dyn Fn(Event) + Send>,

    brush:    Brush,
    /// Engine of the stroke: the native dab engine, or a MyPaint one
    /// built from the preset when it is a MyPaint brush.
    dabs:     DabEngine,
    mypaint:  Option<MyPaintEngine>,
    /// The stroke started on a paintable layer.
    active:   bool,
    /// Layer the stroke paints on, fixed at Begin (not "the active layer":
    /// the user may select or delete another one while the backlog of a
    /// big brush is still being painted).
    layer:    u64,
    /// Canvas size at Begin: a resize meanwhile ends the stroke.
    size:     (u32, u32),
    /// Last sample of the stroke: x, y, pressure and pen time.
    last:     Option<(f32, f32, f32, f64)>,
    /// The engine is following the pen: it is on (or near) the canvas.
    /// Off it, the stroke is paused like a pen lifted, and resumes where
    /// it comes back (see `sample`).
    on_canvas: bool,
    /// Everything the stroke touched: the area saved for undo.
    stroke_rect: Option<Rect>,
    stats:    Stats,
}

impl Worker {
    fn new(projection: SharedProjection, notify: Box<dyn Fn(Event) + Send>) -> Self {
        Self {
            doc: None, projection, pending: Vec::new(), last_update: Instant::now(), notify,
            brush: Brush::new(),
            dabs: DabEngine::new(),
            mypaint: None,
            active: false,
            layer: 0,
            size: (0, 0),
            last: None,
            on_canvas: false,
            stroke_rect: None,
            stats: Stats::default(),
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        let mut queue = VecDeque::new();
        loop {
            // Sleep until there is work, then take everything queued.
            if queue.is_empty() {
                match rx.recv() {
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
                match cmd {
                    Cmd::Begin { doc, brush, engine } => {
                        // A stroke still open (its End never came, e.g. the
                        // window lost focus): finish it first, so it is
                        // recorded for undo like any other.
                        if self.active { self.finish_stroke(&mut events); }
                        self.doc = Some(doc);
                        if self.with_doc(|w, d| w.begin(d, brush, engine)) == Some(false) {
                            events.push(Event::Notice("Layer is locked or is a folder"));
                        }
                    }
                    Cmd::Sample { x, y, pressure, time } => {
                        let r = self.with_doc(|w, d| w.sample(d, x, y, pressure, time)).flatten();
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
                    Cmd::Undo(_) | Cmd::Redo(_) | Cmd::Clear(_) if self.active => {
                        // Never while a stroke is open (the main thread
                        // doesn't send them then; this is a safety net).
                        events.push(Event::Notice("Finish the stroke first"));
                    }
                    Cmd::Undo(doc) => {
                        let c = doc.lock().unwrap().undo();
                        self.doc = Some(doc);
                        self.report_change(c, "Nothing to undo", &mut events);
                        flush = true;
                    }
                    Cmd::Redo(doc) => {
                        let c = doc.lock().unwrap().redo();
                        self.doc = Some(doc);
                        self.report_change(c, "Nothing to redo", &mut events);
                        flush = true;
                    }
                    Cmd::Clear(doc) => {
                        let cleared = {
                            let mut d = doc.lock().unwrap();
                            d.clear_active().map(|id| (id, (0, 0, d.layers.width, d.layers.height)))
                        };
                        match cleared {
                            Some((id, full)) => {
                                add_rect(&mut self.pending, full);
                                events.push(Event::LayerChanged { layer_id: id });
                            }
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

    /// End the open stroke (recorded for undo) and report its layer.
    fn finish_stroke(&mut self, events: &mut Vec<Event>) {
        if let Some((rect, id)) = self.with_doc(|w, d| w.end(d)).flatten() {
            if let Some(r) = rect { add_rect(&mut self.pending, r); }
            events.push(Event::LayerChanged { layer_id: id });
        }
    }

    /// Merge the layers of the changed areas into the projection.
    fn update_projection(&mut self) {
        self.last_update = Instant::now();
        let rects = std::mem::take(&mut self.pending);
        let Some(doc) = self.doc.clone() else { return };
        let d = doc.lock().unwrap();
        let mut p = self.projection.lock().unwrap();
        if (p.width, p.height) != (d.layers.width, d.layers.height) { return; }
        for r in rects {
            d.layers.composite_rect_into(&mut p.pixels, r);
            p.add_dirty(r);
        }
    }

    /// Run `f` with the stroke's document locked. The lock is taken per
    /// command (cheap, uncontended): the main thread can composite between
    /// two samples.
    fn with_doc<R>(&mut self, f: impl FnOnce(&mut Self, &mut Document) -> R) -> Option<R> {
        let doc = self.doc.clone()?;
        let mut d = doc.lock().unwrap();
        Some(f(self, &mut d))
    }

    fn report_change(&mut self, c: Option<aurastudio_document::Change>, none_msg: &'static str, events: &mut Vec<Event>) {
        match c {
            Some(c) => {
                add_rect(&mut self.pending, c.rect);
                events.push(Event::LayerChanged { layer_id: c.layer_id });
            }
            None => events.push(Event::Notice(none_msg)),
        }
    }

    /// Start a stroke on the active layer. False if it can't be painted
    /// (locked, or a folder is selected).
    fn begin(&mut self, doc: &mut Document, brush: Brush, engine: Engine) -> bool {
        self.last = None;
        self.on_canvas = false;
        self.stroke_rect = None;
        self.stats = Stats::default();
        self.layer = doc.layers.active_id();
        self.size = (doc.layers.width, doc.layers.height);
        self.active = doc.layers.can_paint() && doc.layers.begin_stroke(self.layer);
        if !self.active { return false; }

        self.brush = brush;
        // The engine itself starts with the first sample on the canvas.
        self.mypaint = match &engine {
            Engine::Native => {
                self.dabs.sync_from(&self.brush);
                None
            }
            Engine::MyPaint(preset) => {
                let mut e = MyPaintEngine::new((**preset).clone());
                e.sync_from(&self.brush);
                Some(e)
            }
        };
        true
    }

    fn sample(&mut self, doc: &mut Document, x: f32, y: f32, pressure: f32, time: f64) -> Option<Rect> {
        if !self.active { return None; }
        // The canvas was resized under the stroke: its snapshot no longer
        // matches the layer. Stop here (what was painted stays, undoable).
        if (doc.layers.width, doc.layers.height) != self.size {
            self.active = false;
            return None;
        }
        let t0 = Instant::now();
        let Some(mut surface) = doc.layers.layer_surface(self.layer) else {
            // The layer was deleted: nothing left to paint on.
            self.active = false;
            return None;
        };
        let now = (x, y, pressure, time);
        let prev = self.last.replace(now);
        // Only the part of the path over the canvas (plus a margin for the
        // brush) goes to the engine. Zoomed out, the pen can travel
        // hundreds of thousands of canvas pixels off the canvas: every dab
        // there is invisible, and a MyPaint brush spent seconds on them.
        let (w, h) = self.size;
        let m = (4.0 * self.brush.radius as f32).max(256.0);
        let area = (-m, -m, w as f32 + m, h as f32 + m);
        let mut rect = None;
        let seg = match prev {
            None => inside(area, x, y).then_some((0.0, 1.0)),
            Some(p) => clip_segment(area, (p.0, p.1), (x, y)),
        };
        let at = |t: f32| match prev {
            Some(p) => {
                let l = |a: f32, b: f32| a + (b - a) * t;
                (l(p.0, x), l(p.1, y), l(p.2, pressure), p.3 + (time - p.3) * t as f64)
            }
            None => now,
        };
        match seg {
            Some((t0, t1)) => {
                if !self.on_canvas {
                    // Coming (back) onto the canvas: a fresh start there.
                    self.restart_engine();
                    self.on_canvas = true;
                    rect = union_opt(rect, self.engine_to(&mut surface, at(t0)));
                }
                rect = union_opt(rect, self.engine_to(&mut surface, at(t1)));
                if t1 < 1.0 {
                    // Left the canvas: lift the pen there.
                    rect = union_opt(rect, self.engine_end(&mut surface));
                    self.on_canvas = false;
                }
            }
            None if self.on_canvas => {
                rect = union_opt(rect, self.engine_end(&mut surface));
                self.on_canvas = false;
            }
            None => {}
        }
        self.stats.paint.add(t0.elapsed());
        rect
    }

    /// Follow the pen afresh, keeping what the stroke painted so far.
    fn restart_engine(&mut self) {
        match &mut self.mypaint {
            Some(e) => e.resume(),
            None => self.dabs.begin_stroke(),
        }
    }

    fn engine_to(&mut self, s: &mut aurastudio_layers::LayerSurface, (x, y, p, t): (f32, f32, f32, f64)) -> Option<Rect> {
        match &mut self.mypaint {
            Some(e) => e.stroke_to(s, x, y, p, t),
            None => self.dabs.stroke_to(s, x, y, p),
        }
    }

    /// Pen lifted (end of the stroke, or off the canvas).
    fn engine_end(&mut self, s: &mut aurastudio_layers::LayerSurface) -> Option<Rect> {
        match &mut self.mypaint {
            Some(e) => e.lift(s),
            None => self.dabs.end_stroke(s),
        }
    }

    /// Finish the stroke and record it for undo. Returns the last painted
    /// area and the layer id, or None if nothing was painted.
    fn end(&mut self, doc: &mut Document) -> Option<(Option<Rect>, u64)> {
        let was_active = std::mem::take(&mut self.active);
        // Also after an aborted stroke (layer deleted / canvas resized):
        // whatever was painted before is still recorded for undo.
        self.last.take()?;
        let id = self.layer;
        let same_size = (doc.layers.width, doc.layers.height) == self.size;
        let mut rect = None;
        if was_active && std::mem::take(&mut self.on_canvas) {
            if let Some(mut surface) = doc.layers.layer_surface(id) {
                rect = self.engine_end(&mut surface);
            }
        }
        self.mypaint = None;
        if let Some(r) = rect { self.stroke_rect = Some(self.stroke_rect.map_or(r, |s| union(s, r))); }
        // After a resize the snapshot no longer matches: nothing to record.
        if !same_size { self.stroke_rect = None; }
        if let Some(r) = self.stroke_rect.take() {
            let t0 = Instant::now();
            doc.commit_stroke(id, r);
            log::info!("stroke: {} samples | paint {} | max queued {} | undo save {:.1} ms ({} KB history)",
                self.stats.paint.count, self.stats.paint, self.stats.max_queue,
                t0.elapsed().as_secs_f64() * 1000.0, doc.history().bytes() / 1024);
        }
        // The stroke buffers are freed until the next stroke.
        doc.layers.end_stroke();
        Some((rect, id))
    }
}

fn union_opt(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(union(a, b)),
        (a, b) => a.or(b),
    }
}

/// (x0, y0, x1, y1), in canvas pixels, may be negative.
type Area = (f32, f32, f32, f32);

fn inside(a: Area, x: f32, y: f32) -> bool {
    x >= a.0 && x <= a.2 && y >= a.1 && y <= a.3
}

/// Part of the segment p -> q inside `a`, as (t_enter, t_exit) along it
/// (Liang-Barsky); None when it misses the area.
fn clip_segment(a: Area, p: (f32, f32), q: (f32, f32)) -> Option<(f32, f32)> {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (den, num) in [(-dx, p.0 - a.0), (dx, a.2 - p.0), (-dy, p.1 - a.1), (dy, a.3 - p.1)] {
        if den == 0.0 {
            if num < 0.0 { return None; }
        } else {
            let t = num / den;
            if den < 0.0 { t0 = t0.max(t) } else { t1 = t1.min(t) }
        }
    }
    (t0 <= t1).then_some((t0, t1))
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
        let i = ((y * d.layers.width + x) * 4) as usize;
        [l.pixels[i], l.pixels[i + 1], l.pixels[i + 2], l.pixels[i + 3]]
    }

    fn stroke(pt: &PaintThread, doc: &SharedDoc, y: f32) {
        let mut brush = Brush::new();
        brush.radius = 4;
        pt.send(Cmd::Begin { doc: Arc::clone(doc), brush, engine: Engine::Native });
        pt.send(Cmd::Sample { x: 10.0, y, pressure: 1.0, time: 0.0 });
        pt.send(Cmd::Sample { x: 50.0, y, pressure: 1.0, time: 0.01 });
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
            if let Event::LayerChanged { layer_id } = e { let _ = ev_tx.send(layer_id); }
        });
        stroke(&pt, &doc, 32.0);
        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(5)), Ok(id));
        assert_eq!(pixel(&doc, id, 30, 32), [0, 0, 0, 255]);
        // The projection got the merged result and its changed areas.
        drop(pt);
        let mut p = projection.lock().unwrap();
        assert!(!p.take_dirty().is_empty());
        let i = ((32 * 64 + 30) * 4) as usize;
        assert_eq!(&p.pixels[i..i + 4], &[0, 0, 0, 255]);
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
        brush.radius = 4;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, engine: Engine::Native });
        pt.send(Cmd::Sample { x: 10.0, y: 30.0, pressure: 1.0, time: 0.0 });
        std::thread::sleep(Duration::from_millis(50));
        // The user selects another layer while the stroke goes on.
        doc.lock().unwrap().layers.set_active(other);
        pt.send(Cmd::Sample { x: 50.0, y: 30.0, pressure: 1.0, time: 0.01 });
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
        brush.radius = 4;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush: brush.clone(), engine: Engine::Native });
        pt.send(Cmd::Sample { x: 10.0, y: 10.0, pressure: 1.0, time: 0.0 });
        stroke(&pt, &doc, 40.0);
        drop(pt);
        assert_eq!(doc.lock().unwrap().history().undo_len(), 2);
    }

    /// Deleting the layer mid-stroke stops the stroke; what was painted
    /// before is still undoable and nothing lands on another layer.
    #[test]
    fn deleted_layer_ends_the_stroke_cleanly() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(proj(64, 64), |_| {});
        let mut brush = Brush::new();
        brush.radius = 4;
        pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, engine: Engine::Native });
        pt.send(Cmd::Sample { x: 10.0, y: 10.0, pressure: 1.0, time: 0.0 });
        std::thread::sleep(Duration::from_millis(50));
        {
            let mut d = doc.lock().unwrap();
            d.layers.set_active(id);
            assert!(d.layers.remove_active());
        }
        pt.send(Cmd::Sample { x: 50.0, y: 10.0, pressure: 1.0, time: 0.01 });
        pt.send(Cmd::End);
        drop(pt);
        let d = doc.lock().unwrap();
        // Only the background is left, untouched.
        let Some(Node::Layer(bg)) = d.layers.node(1) else { panic!() };
        assert!(bg.pixels.chunks(4).all(|p| p == [255, 255, 255, 255]));
    }

    #[test]
    fn segments_are_clipped_to_the_area() {
        let a = (0.0, 0.0, 100.0, 100.0);
        assert_eq!(clip_segment(a, (10.0, 10.0), (20.0, 20.0)), Some((0.0, 1.0)));
        assert_eq!(clip_segment(a, (-100.0, 50.0), (100.0, 50.0)), Some((0.5, 1.0)));
        assert_eq!(clip_segment(a, (50.0, 50.0), (50.0, 250.0)), Some((0.0, 0.25)));
        // Crossing the whole area from outside to outside.
        assert_eq!(clip_segment(a, (-100.0, 50.0), (200.0, 50.0)), Some((1.0 / 3.0, 2.0 / 3.0)));
        assert_eq!(clip_segment(a, (-10.0, -10.0), (-5.0, 500.0)), None);
    }

    /// Zoomed out, a stroke can wander hundreds of thousands of pixels off
    /// the canvas: it pauses there and resumes where it comes back, with
    /// any engine, quickly (MyPaint brushes took seconds per sample).
    #[test]
    fn stroke_leaving_the_canvas_far_pauses_and_resumes() {
        let lib = aurastudio_brushes::Library::load();
        let mypaint = lib.presets.iter().find(|p| p.engine.is_mypaint() && p.name == "marker fat").unwrap().engine.clone();
        for (n, engine) in [Engine::Native, mypaint].into_iter().enumerate() {
            let (doc, id) = doc_with_layer();
            let pt = PaintThread::spawn(proj(64, 64), |_| {});
            let mut brush = Brush::new();
            brush.radius = 3;
            let t = Instant::now();
            pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, engine });
            // A few samples on the canvas (MyPaint brushes only paint once
            // the pen moves), far away, back step by step (they fade at
            // high speed), a few more.
            let on = |y: f32| (0..6).map(move |i| (10.0 + i as f32 * 4.0, y));
            let path: Vec<(f32, f32)> = on(10.0)
                .chain([(400_000.0, -300_000.0), (-250_000.0, 900_000.0), (-2000.0, 50.0), (-600.0, 50.0),
                        (-300.0, 50.0), (-150.0, 50.0), (-60.0, 50.0), (-20.0, 50.0)])
                .chain(on(50.0)).collect();
            for (i, (x, y)) in path.into_iter().enumerate() {
                pt.send(Cmd::Sample { x, y, pressure: 1.0, time: i as f64 * 0.01 });
            }
            pt.send(Cmd::End);
            drop(pt);
            assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
            assert!(pixel(&doc, id, 20, 10)[3] > 0, "start not painted (engine {n})");
            assert!(pixel(&doc, id, 20, 50)[3] > 0, "end not painted (engine {n})");
            assert_eq!(doc.lock().unwrap().history().undo_len(), 1);
        }
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
        assert!(l.pixels.chunks(4).all(|p| p[3] == 0));
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
            brush.radius = radius;
            pt.send(Cmd::Begin { doc: Arc::clone(&doc), brush, engine: Engine::Native });
            let t0 = Instant::now();
            let n = 200;
            for i in 0..n {
                let t = i as f32 / n as f32;
                let x = 300.0 + i as f32 * 7.0;
                let y = 1000.0 + (t * 12.0).sin() * 400.0;
                pt.send(Cmd::Sample { x, y, pressure: 0.3 + 0.7 * (t * 9.0).sin().abs(), time: i as f64 * 0.005 });
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
        let mut p = projection.lock().unwrap();
        let w = p.width as usize;
        for (x0, y0, x1, y1) in p.take_dirty() {
            for y in y0 as usize..y1 as usize {
                let a = (y * w + x0 as usize) * 4;
                let b = (y * w + x1 as usize) * 4;
                screen[a..b].copy_from_slice(&p.pixels[a..b]);
            }
        }
    }
}

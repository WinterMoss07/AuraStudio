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
//! a parallel round pen made every stroke slower, hokusai included. This
//! thread exists to keep painting off the input / UI thread, not to split it.
//!
//! Undo, redo and clear go through the same queue, so they always apply
//! after the samples sent before them: Ctrl+Z right after lifting the pen
//! undoes that stroke, even if it is still being painted.
//!
//! The document is shared under a mutex. The worker holds it for at most
//! BATCH_TIME at once, so the main thread can always grab it to composite
//! the changed area and show progress, even mid-stroke.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aurastudio_brush::{stroke_line, Brush, HokusaiEngine, PressureSmoother};
use aurastudio_canvas::Rect;
use aurastudio_document::{Document, SharedDoc};

/// Longest time the worker keeps the layers locked in one go.
const BATCH_TIME: Duration = Duration::from_millis(6);

pub enum Cmd {
    /// Start a stroke on the active layer of `doc` with these settings.
    Begin { doc: SharedDoc, brush: Brush, hokusai: bool, smoothing: f32 },
    /// One pen sample, canvas coordinates.
    Sample { x: f32, y: f32, pressure: f32 },
    End,
    Undo(SharedDoc),
    Redo(SharedDoc),
    /// Clear the active layer (undoable).
    Clear(SharedDoc),
}

/// What the worker reports to the main thread (through `notify`).
pub enum Event {
    /// Some canvas area changed: take it with `take_dirty`.
    Painted,
    /// A layer's pixels changed for good (stroke done, undo...): refresh
    /// its thumbnail.
    LayerChanged { layer_id: u64 },
    /// Short message for the options bar ("Nothing to undo"...).
    Notice(&'static str),
}

pub struct PaintThread {
    tx:     Option<Sender<Cmd>>,
    dirty:  Arc<Mutex<Option<Rect>>>,
    handle: Option<JoinHandle<()>>,
}

impl PaintThread {
    pub fn spawn(notify: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let dirty = Arc::new(Mutex::new(None));
        let worker = Worker::new(Arc::clone(&dirty), Box::new(notify));
        let handle = std::thread::Builder::new()
            .name("paint".into())
            .spawn(move || worker.run(rx))
            .expect("spawn paint thread");
        Self { tx: Some(tx), dirty, handle: Some(handle) }
    }

    pub fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Area painted since the last call.
    pub fn take_dirty(&self) -> Option<Rect> {
        self.dirty.lock().unwrap().take()
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
    /// Document of the current stroke.
    doc:      Option<SharedDoc>,
    dirty:    Arc<Mutex<Option<Rect>>>,
    notify:   Box<dyn Fn(Event) + Send>,

    brush:    Brush,
    hokusai:  HokusaiEngine,
    use_hokusai: bool,
    smoother: PressureSmoother,
    /// The stroke started on a paintable layer.
    active:   bool,
    /// Last sample of the stroke: x, y and smoothed pressure.
    last:     Option<(f32, f32, f32)>,
    /// Everything the stroke touched: the area saved for undo.
    stroke_rect: Option<Rect>,
    stats:    Stats,
}

impl Worker {
    fn new(dirty: Arc<Mutex<Option<Rect>>>, notify: Box<dyn Fn(Event) + Send>) -> Self {
        Self {
            doc: None, dirty, notify,
            brush: Brush::new(),
            hokusai: HokusaiEngine::new(),
            use_hokusai: false,
            smoother: PressureSmoother::new(0.5),
            active: false,
            last: None,
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

            let mut changed: Option<Rect> = None;
            let mut events = Vec::new();
            let t0 = Instant::now();
            while let Some(cmd) = queue.pop_front() {
                match cmd {
                    Cmd::Begin { doc, brush, hokusai, smoothing } => {
                        self.doc = Some(doc);
                        self.with_doc(|w, d| w.begin(d, brush, hokusai, smoothing));
                    }
                    Cmd::Sample { x, y, pressure } => {
                        let r = self.with_doc(|w, d| w.sample(d, x, y, pressure)).flatten();
                        merge(&mut changed, r);
                        merge(&mut self.stroke_rect, r);
                    }
                    Cmd::End => {
                        if let Some((rect, id)) = self.with_doc(|w, d| w.end(d)).flatten() {
                            merge(&mut changed, rect);
                            events.push(Event::LayerChanged { layer_id: id });
                        }
                        // Report the end right away, with its pixels.
                        break;
                    }
                    Cmd::Undo(doc) | Cmd::Redo(doc) | Cmd::Clear(doc) if self.active => {
                        // Never while a stroke is open (the main thread
                        // doesn't send them then; this is a safety net).
                        let _ = doc;
                        events.push(Event::Notice("Finish the stroke first"));
                    }
                    Cmd::Undo(doc) => {
                        let c = doc.lock().unwrap().undo();
                        Self::report_change(c, "Nothing to undo", &mut changed, &mut events);
                    }
                    Cmd::Redo(doc) => {
                        let c = doc.lock().unwrap().redo();
                        Self::report_change(c, "Nothing to redo", &mut changed, &mut events);
                    }
                    Cmd::Clear(doc) => {
                        let mut d = doc.lock().unwrap();
                        match d.clear_active() {
                            Some(id) => {
                                merge(&mut changed, Some((0, 0, d.layers.width, d.layers.height)));
                                events.push(Event::LayerChanged { layer_id: id });
                            }
                            None => events.push(Event::Notice("Layer is locked or is a folder")),
                        }
                    }
                }
                if t0.elapsed() >= BATCH_TIME { break; }
            }

            if let Some(r) = changed {
                merge(&mut self.dirty.lock().unwrap(), Some(r));
                (self.notify)(Event::Painted);
            }
            for e in events {
                (self.notify)(e);
            }
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

    fn report_change(
        c: Option<aurastudio_document::Change>,
        none_msg: &'static str,
        changed: &mut Option<Rect>,
        events: &mut Vec<Event>,
    ) {
        match c {
            Some(c) => {
                merge(changed, Some(c.rect));
                events.push(Event::LayerChanged { layer_id: c.layer_id });
            }
            None => events.push(Event::Notice(none_msg)),
        }
    }

    fn begin(&mut self, doc: &mut Document, brush: Brush, hokusai: bool, smoothing: f32) {
        // The main thread already refused locked layers / folders; checked
        // again here in case the layer changed in between.
        self.active = doc.layers.can_paint();
        self.last = None;
        self.stroke_rect = None;
        self.stats = Stats::default();
        if !self.active { return; }

        doc.layers.begin_stroke();
        self.brush = brush;
        self.use_hokusai = hokusai;
        self.smoother.span = smoothing;
        self.smoother.reset();
        if hokusai {
            self.hokusai.smoother.span = smoothing;
            self.hokusai.sync_from(&self.brush);
            self.hokusai.begin_stroke();
        }
    }

    fn sample(&mut self, doc: &mut Document, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        if !self.active { return None; }
        let t0 = Instant::now();
        let mut surface = doc.layers.active_surface()?;
        let rect = if self.use_hokusai {
            self.last = Some((x, y, pressure));
            self.hokusai.stroke_to(&mut surface, x, y, pressure)
        } else {
            let p = self.smoother.apply(x, y, pressure, self.brush.radius as f32);
            let (fx, fy, fp) = self.last.unwrap_or((x, y, p));
            self.last = Some((x, y, p));
            stroke_line((fx, fy), (x, y), &self.brush, &mut surface, fp, p)
        };
        self.stats.paint.add(t0.elapsed());
        rect
    }

    /// Finish the stroke and record it for undo. Returns the last painted
    /// area and the layer id, or None if nothing was painted.
    fn end(&mut self, doc: &mut Document) -> Option<(Option<Rect>, u64)> {
        let was_active = std::mem::take(&mut self.active);
        if !was_active || self.last.take().is_none() { return None; }
        let mut rect = None;
        if self.use_hokusai {
            if let Some(mut surface) = doc.layers.active_surface() {
                rect = self.hokusai.end_stroke(&mut surface);
            }
        }
        merge(&mut self.stroke_rect, rect);
        let id = doc.layers.active_id();
        if let Some(r) = self.stroke_rect.take() {
            let t0 = Instant::now();
            doc.commit_stroke(id, r);
            log::info!("stroke: {} samples | paint {} | max queued {} | undo save {:.1} ms ({} KB history)",
                self.stats.paint.count, self.stats.paint, self.stats.max_queue,
                t0.elapsed().as_secs_f64() * 1000.0, doc.history().bytes() / 1024);
        }
        Some((rect, id))
    }
}

fn merge(acc: &mut Option<Rect>, r: Option<Rect>) {
    let Some(r) = r else { return };
    *acc = Some(match *acc {
        None    => r,
        Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
    });
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
        pt.send(Cmd::Begin { doc: Arc::clone(doc), brush, hokusai: false, smoothing: 0.0 });
        pt.send(Cmd::Sample { x: 10.0, y, pressure: 1.0 });
        pt.send(Cmd::Sample { x: 50.0, y, pressure: 1.0 });
        pt.send(Cmd::End);
    }

    /// A stroke sent to the thread ends up on the layer, and the thread
    /// reports both the painted area and the changed layer.
    #[test]
    fn paints_on_its_own_thread() {
        let (doc, id) = doc_with_layer();
        let (ev_tx, ev_rx) = channel();
        let pt = PaintThread::spawn(move |e| {
            if let Event::LayerChanged { layer_id } = e { let _ = ev_tx.send(layer_id); }
        });
        stroke(&pt, &doc, 32.0);
        assert_eq!(ev_rx.recv_timeout(Duration::from_secs(5)), Ok(id));
        assert!(pt.take_dirty().is_some());
        assert_eq!(pixel(&doc, id, 30, 32), [0, 0, 0, 255]);
    }

    /// Undo sent right after End undoes that stroke (same queue), then redo
    /// brings it back.
    #[test]
    fn undo_follows_the_stroke_it_undoes() {
        let (doc, id) = doc_with_layer();
        let pt = PaintThread::spawn(|_| {});
        stroke(&pt, &doc, 20.0);
        stroke(&pt, &doc, 40.0);
        pt.send(Cmd::Undo(Arc::clone(&doc)));
        drop(pt); // joins: everything above is processed, in order
        assert_eq!(pixel(&doc, id, 30, 20), [0, 0, 0, 255]);
        assert_eq!(pixel(&doc, id, 30, 40)[3], 0);

        let pt = PaintThread::spawn(|_| {});
        pt.send(Cmd::Redo(Arc::clone(&doc)));
        drop(pt);
        assert_eq!(pixel(&doc, id, 30, 40), [0, 0, 0, 255]);
    }

    /// A locked layer is never painted, even if a stroke is sent.
    #[test]
    fn locked_layer_stays_untouched() {
        let (doc, id) = doc_with_layer();
        *doc.lock().unwrap().layers.node_mut(id).unwrap().locked_mut() = true;
        let pt = PaintThread::spawn(|_| {});
        stroke(&pt, &doc, 16.0);
        drop(pt);
        let d = doc.lock().unwrap();
        let Some(Node::Layer(l)) = d.layers.node(id) else { panic!() };
        assert!(l.pixels.chunks(4).all(|p| p[3] == 0));
        assert_eq!(d.history().undo_len(), 0);
    }
}

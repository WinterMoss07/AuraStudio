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
//! The layer stack is shared under a mutex. The worker holds it for at
//! most BATCH_TIME at once, so the main thread can always grab it to
//! composite the changed area and show progress, even mid-stroke.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aurastudio_brush::{stroke_line, Brush, HokusaiEngine, PressureSmoother};
use aurastudio_canvas::Rect;
use aurastudio_layers::LayerStack;

/// Longest time the worker keeps the layers locked in one go.
const BATCH_TIME: Duration = Duration::from_millis(6);

pub enum Cmd {
    /// Start a stroke on the active layer with these settings.
    Begin { brush: Brush, hokusai: bool, smoothing: f32 },
    /// One pen sample, canvas coordinates.
    Sample { x: f32, y: f32, pressure: f32 },
    End,
}

/// What the worker reports to the main thread (through `notify`).
pub enum Event {
    /// Some canvas area changed: take it with `take_dirty`.
    Painted,
    /// The stroke is fully painted on this layer.
    StrokeDone { layer_id: u64 },
}

pub struct PaintThread {
    tx:     Option<Sender<Cmd>>,
    dirty:  Arc<Mutex<Option<Rect>>>,
    handle: Option<JoinHandle<()>>,
}

impl PaintThread {
    pub fn spawn(layers: Arc<Mutex<LayerStack>>, notify: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let dirty = Arc::new(Mutex::new(None));
        let worker = Worker::new(layers, Arc::clone(&dirty), Box::new(notify));
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
    layers:   Arc<Mutex<LayerStack>>,
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
    stats:    Stats,
}

impl Worker {
    fn new(layers: Arc<Mutex<LayerStack>>, dirty: Arc<Mutex<Option<Rect>>>, notify: Box<dyn Fn(Event) + Send>) -> Self {
        Self {
            layers, dirty, notify,
            brush: Brush::new(),
            hokusai: HokusaiEngine::new(),
            use_hokusai: false,
            smoother: PressureSmoother::new(0.5),
            active: false,
            last: None,
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
            let mut done = None;
            {
                // Own handle: the guard must not borrow `self`.
                let shared = Arc::clone(&self.layers);
                let mut layers = shared.lock().unwrap();
                let t0 = Instant::now();
                while let Some(cmd) = queue.pop_front() {
                    match cmd {
                        Cmd::Begin { brush, hokusai, smoothing } => self.begin(&mut layers, brush, hokusai, smoothing),
                        Cmd::Sample { x, y, pressure } => merge(&mut changed, self.sample(&mut layers, x, y, pressure)),
                        Cmd::End => {
                            let (rect, id) = self.end(&mut layers);
                            merge(&mut changed, rect);
                            done = id;
                            // Report the end right away, with its pixels.
                            break;
                        }
                    }
                    if t0.elapsed() >= BATCH_TIME { break; }
                }
            }

            if let Some(r) = changed {
                merge(&mut self.dirty.lock().unwrap(), Some(r));
                (self.notify)(Event::Painted);
            }
            if let Some(layer_id) = done {
                (self.notify)(Event::StrokeDone { layer_id });
            }
        }
    }

    fn begin(&mut self, layers: &mut LayerStack, brush: Brush, hokusai: bool, smoothing: f32) {
        // The main thread already refused locked layers / folders; checked
        // again here in case the layer changed in between.
        self.active = layers.can_paint();
        self.last = None;
        self.stats = Stats::default();
        if !self.active { return; }

        layers.begin_stroke();
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

    fn sample(&mut self, layers: &mut LayerStack, x: f32, y: f32, pressure: f32) -> Option<Rect> {
        if !self.active { return None; }
        let t0 = Instant::now();
        let mut surface = layers.active_surface()?;
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

    /// Returns the last painted area and the layer id, if anything was painted.
    fn end(&mut self, layers: &mut LayerStack) -> (Option<Rect>, Option<u64>) {
        if !self.active || self.last.is_none() {
            self.active = false;
            return (None, None);
        }
        let mut rect = None;
        if self.use_hokusai {
            if let Some(mut surface) = layers.active_surface() {
                rect = self.hokusai.end_stroke(&mut surface);
            }
        }
        log::info!("stroke: {} samples | paint {} | max queued {}",
            self.stats.paint.count, self.stats.paint, self.stats.max_queue);
        self.active = false;
        self.last = None;
        (rect, Some(layers.active_id()))
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
    use std::sync::mpsc::channel;

    /// A stroke sent to the thread ends up on the layer, and the thread
    /// reports both the painted area and the end of the stroke.
    #[test]
    fn paints_on_its_own_thread() {
        let mut stack = LayerStack::new(64, 64);
        let id = stack.add_layer("ink");
        let layers = Arc::new(Mutex::new(stack));
        let (ev_tx, ev_rx) = channel();
        let pt = PaintThread::spawn(Arc::clone(&layers), move |e| {
            let _ = ev_tx.send(match e { Event::Painted => 0, Event::StrokeDone { layer_id } => layer_id });
        });

        let mut brush = Brush::new();
        brush.radius = 4;
        pt.send(Cmd::Begin { brush, hokusai: false, smoothing: 0.0 });
        pt.send(Cmd::Sample { x: 10.0, y: 32.0, pressure: 1.0 });
        pt.send(Cmd::Sample { x: 50.0, y: 32.0, pressure: 1.0 });
        pt.send(Cmd::End);

        let mut got_done = false;
        while let Ok(v) = ev_rx.recv_timeout(Duration::from_secs(5)) {
            if v == id { got_done = true; break; }
        }
        assert!(got_done, "no StrokeDone");
        assert!(pt.take_dirty().is_some());

        let stack = layers.lock().unwrap();
        let Some(aurastudio_layers::Node::Layer(l)) = stack.node(id) else { panic!() };
        let i = ((32 * 64 + 30) * 4) as usize;
        assert_eq!(&l.pixels[i..i + 4], &[0, 0, 0, 255]);
    }

    /// A locked layer is never painted, even if a stroke is sent.
    #[test]
    fn locked_layer_stays_untouched() {
        let mut stack = LayerStack::new(32, 32);
        let id = stack.add_layer("locked");
        *stack.node_mut(id).unwrap().locked_mut() = true;
        let layers = Arc::new(Mutex::new(stack));
        let pt = PaintThread::spawn(Arc::clone(&layers), |_| {});
        pt.send(Cmd::Begin { brush: Brush::new(), hokusai: false, smoothing: 0.0 });
        pt.send(Cmd::Sample { x: 16.0, y: 16.0, pressure: 1.0 });
        pt.send(Cmd::End);
        drop(pt); // joins: everything above is processed
        let stack = layers.lock().unwrap();
        let Some(aurastudio_layers::Node::Layer(l)) = stack.node(id) else { panic!() };
        assert!(l.pixels.chunks(4).all(|p| p[3] == 0));
    }
}

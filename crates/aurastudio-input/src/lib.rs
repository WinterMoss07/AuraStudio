//! The only door for everything that points: pen, eraser, tablet touch,
//! screen touch, mouse, wheel. It captures, cleans and publishes one
//! stream of events; it knows nothing of the document, the brush or the
//! interface. An input bug is looked for here only.
//!
//! The app hands it the window (once), the canvas rect and whether the
//! interface wants the pointer (each frame), its raw mouse / touch events
//! (no logic) and window changes; it polls events. Keyboard stays with the
//! app.
//!
//! A stroke under way can be routed (`Input::route`): its moves and its
//! end go to the app's sink on the thread that captured them (the pen's
//! own thread with WinTab) as they are made, in order, without waiting
//! for the interface's loop; the polled events say they were routed.
//!
//! Portions of the Windows Ink and WinTab hooks were adapted from
//! efude-input (https://github.com/852wa/Efude), © 2026 Hakoniwa, MIT OR
//! Apache-2.0 (see /NOTICE.md).

pub mod clock;
pub mod convert;
pub mod debounce;
pub mod event;
pub mod latency;
pub mod state;
pub mod strokelog;
pub mod synthetic;
#[cfg(windows)]
pub mod win;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use aurastudio_core::{Micros, TransferCurve};

pub use event::{Device, Event, Kind, Source, Target};
pub use state::{FrameInfo, RawSample};

/// Which API captures the pen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend { WinTab, WindowsInk, None }

impl Backend {
    pub fn name(self) -> &'static str { match self { Backend::WinTab => "WinTab", Backend::WindowsInk => "Windows Ink", Backend::None => "none (mouse and touch only)" } }
}

/// A mouse button as the app forwards it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button { Left, Right, Middle, Other(u8) }

/// What `input` has measured and found (for the log and the report).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub backend:        Option<&'static str>,
    pub packets:        u64,
    /// Packets the tablet driver dropped (gaps in serial numbers).
    pub lost:           u64,
    pub reopened:       u64,
    pub bounces:        u64,
    /// Settings problems, in plain words.
    pub warnings:       Vec<String>,
}

/// Wakes the app's loop when a capture thread has samples: without it they
/// waited for some other event (a frame, a mouse message) to be read, and
/// a busy loop held the pen back. One call until the app has read them.
#[derive(Default)]
pub struct Waker {
    pending: AtomicBool,
    wake:    OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl Waker {
    pub fn wake(&self) {
        if self.pending.swap(true, Ordering::AcqRel) { return; }
        if let Some(f) = self.wake.get() { f(); }
    }

    /// The app reads: the next sample wakes it again.
    fn read(&self) { self.pending.store(false, Ordering::Release); }
}

/// Where a routed stroke's events go (called on the capturing thread).
pub type Sink = Arc<dyn Fn(&Event) + Send + Sync>;

struct Route { device: Device, sink: Sink }

/// A routed device's events: its moves and its end.
fn routes(device: Device, e: &Event) -> bool { e.device == device && matches!(e.kind, Kind::Move | Kind::Up { .. }) }

/// Hand `events` of `route`'s device to its sink, marked; true once its
/// end went (the route is over).
fn route_events(route: &Route, events: &mut [Event]) -> bool {
    for e in events {
        if !routes(route.device, e) { continue; }
        (route.sink)(e);
        e.routed = true;
        if matches!(e.kind, Kind::Up { .. }) { return true; }
    }
    false
}

/// What the capture threads and the app share: the device state machines,
/// the events made and not polled yet, the stroke route. Every sample goes
/// through it in one order, whichever thread captured it.
#[derive(Default)]
pub struct Feed { core: Mutex<Core> }

#[derive(Default)]
struct Core {
    machine: state::Machine,
    out:     Vec<Event>,
    route:   Option<Route>,
}

impl Core {
    /// The events from `n` on, made just now: routed if their stroke is.
    fn route_from(&mut self, n: usize) {
        let Some(r) = &self.route else { return };
        if route_events(r, &mut self.out[n..]) { self.route = None; }
    }
}

impl Feed {
    fn lock(&self) -> MutexGuard<'_, Core> { self.core.lock().unwrap_or_else(PoisonError::into_inner) }

    /// One raw sample, from any thread.
    pub(crate) fn push(&self, s: RawSample) {
        let mut c = self.lock();
        let n = c.out.len();
        let Core { machine, out, .. } = &mut *c;
        machine.feed(s, out);
        c.route_from(n);
    }
}

pub struct Input {
    feed:     Arc<Feed>,
    waker:    Arc<Waker>,
    backend:  Backend,
    /// The mouse as the app forwards it.
    mouse:    (f64, f64, bool, u8),
    debounce: debounce::Debounce<RawSample>,
    /// Strokes being recorded, per device.
    recording: Vec<Event>,
    log:      Option<strokelog::Writer>,
    app_version: String,
    #[cfg(windows)]
    win:      Option<win::Capture>,
    #[cfg(windows)]
    hwnd:     isize,
}

impl Input {
    pub fn new(app_version: &str) -> Self {
        Self {
            feed: Arc::default(), waker: Arc::default(), backend: Backend::None,
            mouse: (0.0, 0.0, false, 0), debounce: debounce::Debounce::default(), recording: Vec::new(),
            log: None, app_version: app_version.to_owned(),
            #[cfg(windows)] win: None,
            #[cfg(windows)] hwnd: 0,
        }
    }

    /// Start capturing the pen for the window `hwnd` (Windows: WinTab on
    /// its own thread, else Windows Ink; elsewhere: mouse and touch only).
    pub fn install(&mut self, hwnd: isize) -> Backend {
        #[cfg(windows)]
        {
            self.hwnd = hwnd;
            let origin = Arc::new(win::Origin::default());
            if let Some((x, y)) = win::client_origin(hwnd) { origin.set(x, y); }
            let counters = Arc::new(win::Counters::default());
            let wintab = win::wintab::Thread::start(Arc::clone(&self.feed), Arc::clone(&origin), Arc::clone(&counters), Arc::clone(&self.waker));
            let ink = win::ink::Hook::install(hwnd, Arc::clone(&self.feed), Arc::clone(&origin), Arc::clone(&counters), wintab.is_err()).ok();
            self.backend = match (&wintab, &ink) { (Ok(_), _) => Backend::WinTab, (Err(_), Some(_)) => Backend::WindowsInk, _ => Backend::None };
            self.win = Some(win::Capture { wintab: wintab.ok(), ink, origin, counters });
        }
        #[cfg(not(windows))]
        let _ = hwnd;
        self.backend
    }

    pub fn backend(&self) -> Backend { self.backend }

    /// What wakes the app's loop when the pen has samples (once).
    pub fn set_waker(&self, f: impl Fn() + Send + Sync + 'static) { let _ = self.waker.wake.set(Box::new(f)); }

    /// The app's global pressure curve (Preferences > Tablet).
    pub fn set_curve(&mut self, c: TransferCurve) { self.feed.lock().machine.curve = c; }

    /// The mouse bounce filter's window (0: off).
    pub fn set_debounce(&mut self, window: Duration) { self.debounce.window = window; }

    /// When a mouse release held back by the bounce filter comes due (the
    /// app's loop wakes then and polls).
    pub fn deadline(&self) -> Option<Instant> { self.debounce.deadline() }

    /// Each frame: where the canvas is and whether the interface wants
    /// the pointer.
    pub fn set_frame(&mut self, f: FrameInfo) { self.feed.lock().machine.frame = f; }

    /// The window moved or its DPI changed.
    pub fn window_moved(&mut self) {
        #[cfg(windows)]
        if let Some(w) = &self.win {
            if let Some((x, y)) = win::client_origin(self.hwnd) { w.origin.set(x, y); }
            if let Some(i) = &w.ink { i.refresh(); }
        }
    }

    /// The app came to the front or went to the back. Going to the back
    /// ends any stroke cleanly.
    pub fn activated(&mut self, front: bool) {
        #[cfg(windows)]
        if let Some(t) = self.win.as_ref().and_then(|w| w.wintab.as_ref()) { t.overlap(front); }
        if !front { self.cancel(); }
    }

    /// The display changed or the computer woke up: strokes end, the
    /// tablet context is opened again.
    pub fn display_changed(&mut self) {
        self.cancel();
        self.window_moved();
        #[cfg(windows)]
        if let Some(t) = self.win.as_ref().and_then(|w| w.wintab.as_ref()) { t.reopen(); }
    }

    /// End every stroke in progress (focus lost, Alt+Tab).
    pub fn cancel(&mut self) {
        let mut c = self.feed.lock();
        let n = c.out.len();
        let Core { machine, out, .. } = &mut *c;
        machine.cancel_all(clock::now(), out);
        c.route_from(n);
    }

    /// Route the stroke `device` has down: its moves and its end go to
    /// `sink` from now on, on the capturing thread, as they are made. Those
    /// in `rest` (the rest of the events just polled) and those made since
    /// the poll go first, in order. False if the stroke already ended
    /// there (nothing left to route).
    pub fn route(&mut self, device: Device, sink: Sink, rest: &mut [Event]) -> bool {
        let mut c = self.feed.lock();
        let route = Route { device, sink };
        if route_events(&route, rest) || route_events(&route, &mut c.out) { return false; }
        c.route = Some(route);
        true
    }

    /// Stop routing (the app ended the stroke itself).
    pub fn unroute(&mut self) { self.feed.lock().route = None; }

    /// Whether the mouse event being handled was made by Windows from pen
    /// or touch input (dropped).
    fn synthetic(&self) -> bool {
        #[cfg(windows)]
        { if self.backend != Backend::None && win::is_synthetic_mouse() { return true; } }
        false
    }

    fn mouse_sample(&self, contact: bool) -> RawSample {
        let (x, y, _, buttons) = self.mouse;
        RawSample { device: Device::Mouse, x, y, pressure: 1.0, tilt: (0.0, 0.0), rotation: 0.0, contact, in_range: true, buttons, time: clock::now() }
    }

    /// The mouse moved (window physical pixels), as winit says.
    pub fn mouse_moved(&mut self, x: f64, y: f64) {
        if self.synthetic() { return; }
        self.mouse.0 = x;
        self.mouse.1 = y;
        let s = self.mouse_sample(self.mouse.2);
        for s in self.debounce.event(s, debounce::Kind::Move, Instant::now()) { self.feed.push(s); }
    }

    /// A mouse button, as winit says.
    pub fn mouse_button(&mut self, b: Button, pressed: bool) {
        if self.synthetic() { return; }
        let bit = match b { Button::Left => None, Button::Right => Some(0), Button::Middle => Some(1), Button::Other(n) => Some(2 + n.min(5)) };
        let kind = match (bit, pressed) {
            (None, true) => { self.mouse.2 = true; debounce::Kind::LeftPress }
            (None, false) => { self.mouse.2 = false; debounce::Kind::LeftRelease }
            (Some(i), true) => { self.mouse.3 |= 1 << i; debounce::Kind::Other }
            (Some(i), false) => { self.mouse.3 &= !(1 << i); debounce::Kind::Other }
        };
        let s = self.mouse_sample(self.mouse.2);
        for s in self.debounce.event(s, kind, Instant::now()) { self.feed.push(s); }
    }

    /// The mouse left the window.
    pub fn mouse_left(&mut self) {
        let mut s = self.mouse_sample(false);
        s.in_range = false;
        if !self.mouse.2 { self.feed.push(s); }
    }

    /// Wheel or touchpad scroll, in lines.
    pub fn wheel(&mut self, dx: f32, dy: f32) {
        let s = self.mouse_sample(false);
        let mut c = self.feed.lock();
        let target = c.machine.frame.target(s.x, s.y);
        c.out.push(Event { kind: Kind::Wheel { dx, dy }, device: Device::Mouse, source: Source::Live, target, x: s.x, y: s.y, pressure: 1.0, pressure_raw: 1.0, tilt: (0.0, 0.0), rotation: 0.0, time: s.time, routed: false });
    }

    /// A finger (phase 1: Windows touch as winit gives it).
    pub fn touch(&mut self, id: u64, x: f64, y: f64, down: bool, force: Option<f32>) {
        self.feed.push(RawSample { device: Device::Touch(id as u32), x, y, pressure: force.unwrap_or(1.0), tilt: (0.0, 0.0), rotation: 0.0, contact: down, in_range: down, buttons: 0, time: clock::now() });
    }

    /// Record strokes in `dir` (oldest sessions removed past the budget).
    pub fn open_strokelog(&mut self, dir: &std::path::Path) -> bool {
        let curve = self.feed.lock().machine.curve.clone();
        self.log = strokelog::Writer::open(dir, &self.app_version, &curve, strokelog::BUDGET);
        self.log.is_some()
    }

    /// Everything that happened since the last call, in order.
    pub fn poll(&mut self) -> Vec<Event> {
        self.waker.read();
        for s in self.debounce.poll(Instant::now()) { self.feed.push(s); }
        let (mut out, down) = {
            let mut c = self.feed.lock();
            (std::mem::take(&mut c.out), c.machine.any_down())
        };
        // During a stroke nothing is dropped; outside, hover is coalesced.
        if !down { state::coalesce_hover(&mut out); }
        self.record(&out);
        out
    }

    /// Feed a recorded stroke as if it came now (tests, benchmarks, the
    /// diagnostic): its events, retimed from `start`.
    pub fn replay(stroke: &strokelog::Stroke, start: Micros, speed: f64) -> Vec<Event> { strokelog::replay(stroke, start, speed) }

    /// Keep the events of strokes on the canvas; a finished one goes to
    /// the strokelog. No strokelog open: nothing is kept.
    fn record(&mut self, events: &[Event]) {
        if self.log.is_none() { return; }
        for e in events {
            if !matches!(e.device, Device::Pen | Device::Eraser | Device::Mouse) || e.target != Target::Canvas { continue; }
            match e.kind {
                Kind::Down => { self.recording.clear(); self.recording.push(*e); }
                Kind::Move | Kind::ButtonDown(_) | Kind::ButtonUp(_) if !self.recording.is_empty() => self.recording.push(*e),
                Kind::Up { .. } if !self.recording.is_empty() => {
                    self.recording.push(*e);
                    let events = std::mem::take(&mut self.recording);
                    if let Some(l) = &self.log { l.push(strokelog::Stroke { context: Vec::new(), events }); }
                }
                _ => {}
            }
        }
    }

    /// The last stroke, if one is being drawn or was just finished, as a
    /// file of its own ("send this stroke").
    pub fn export(&self, stroke: &strokelog::Stroke) -> Vec<u8> { strokelog::export(&self.app_version, &self.feed.lock().machine.curve, stroke) }

    /// Counters and warnings.
    #[cfg(test)]
    pub fn health(&self) -> Health {
        let mut h = Health { backend: Some(self.backend.name()), bounces: self.debounce.bounces, ..Default::default() };
        #[cfg(windows)]
        if let Some(w) = &self.win {
            use std::sync::atomic::Ordering::Relaxed;
            let c = &w.counters;
            (h.packets, h.lost, h.reopened) = (c.wintab_packets.load(Relaxed), c.wintab_lost.load(Relaxed), c.reopened.load(Relaxed));
            let ink = c.ink_pen.load(Relaxed);
            if self.backend == Backend::WinTab && h.packets == 0 && ink > 200 {
                h.warnings.push("The pen reaches the app through Windows Ink, not WinTab: in the Wacom settings (Wacom Center > Pen > Mapping), turn \"Use Windows Ink\" off for AuraStudio, or choose Windows Ink in Preferences > Tablet.".into());
            }
        }
        if h.lost > 0 { h.warnings.push(format!("The tablet driver dropped {} pen samples (fast strokes may miss a bit).", h.lost)); }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_strokes_on_the_canvas_are_recorded() {
        let mut i = Input::new("t");
        i.set_debounce(Duration::ZERO);
        i.set_frame(FrameInfo { canvas: (0.0, 0.0, 500.0, 500.0), ui_wants_pointer: false });
        let dir = std::env::temp_dir().join(format!("input-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(i.open_strokelog(&dir));
        i.mouse_moved(10.0, 10.0);
        i.mouse_button(Button::Left, true);
        i.mouse_moved(20.0, 15.0);
        i.mouse_button(Button::Left, false);
        let ev = i.poll();
        let kinds: Vec<Kind> = ev.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&Kind::Down) && kinds.contains(&Kind::Move) && kinds.contains(&Kind::Up { cancelled: false }), "{kinds:?}");
        assert!(ev.iter().all(|e| e.device == Device::Mouse && e.pressure == 1.0));
        i.wheel(0.0, -1.0);
        assert!(matches!(i.poll().first().map(|e| e.kind), Some(Kind::Wheel { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn going_to_the_back_ends_the_stroke() {
        let mut i = Input::new("t");
        i.set_debounce(Duration::ZERO);
        i.mouse_moved(10.0, 10.0);
        i.mouse_button(Button::Left, true);
        i.poll();
        i.activated(false);
        assert!(i.poll().iter().any(|e| e.kind == Kind::Up { cancelled: true }));
        assert_eq!(i.health().backend, Some("none (mouse and touch only)"));
    }

    fn pen(x: f64, contact: bool, t: u64) -> RawSample {
        RawSample { device: Device::Pen, x, y: 5.0, pressure: 0.5, tilt: (0.0, 0.0), rotation: 0.0, contact, in_range: true, buttons: 0, time: Micros(t) }
    }

    /// A routed stroke: the moves left in the poll, then those made since,
    /// then those a capture thread makes go to the sink once each, in
    /// order, up to its end; the polled events say so; the next stroke is
    /// not routed.
    #[test]
    fn a_routed_stroke_reaches_the_sink_in_order() {
        let mut i = Input::new("t");
        i.set_frame(FrameInfo { canvas: (0.0, 0.0, 1000.0, 100.0), ui_wants_pointer: false });
        for (k, c) in [(0, false), (1, true), (2, true), (3, true)] { i.feed.push(pen(k as f64, c, k)); }
        let mut ev = i.poll();
        // (Made after the poll, before the app routes.)
        i.feed.push(pen(4.0, true, 4));
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let down = ev.iter().position(|e| e.kind == Kind::Down).unwrap();
        assert!(i.route(Device::Pen, Arc::new(move |e: &Event| g.lock().unwrap().push((e.kind, e.x))), &mut ev[down + 1..]));
        assert!(ev[down + 1..].iter().filter(|e| e.kind == Kind::Move).all(|e| e.routed));
        let feed = Arc::clone(&i.feed);
        std::thread::spawn(move || { for k in 5..50 { feed.push(pen(k as f64, k < 49, k)); } }).join().unwrap();
        let later = i.poll();
        assert!(later.iter().filter(|e| matches!(e.kind, Kind::Move | Kind::Up { .. })).all(|e| e.routed));
        let xs: Vec<f64> = got.lock().unwrap().iter().map(|(_, x)| *x).collect();
        assert_eq!(xs, (2..50).map(|k| k as f64).collect::<Vec<_>>());
        assert!(matches!(got.lock().unwrap().last(), Some((Kind::Up { cancelled: false }, _))));
        // Over: the next stroke is the app's.
        i.feed.push(pen(60.0, true, 60));
        i.feed.push(pen(61.0, true, 61));
        assert!(i.poll().iter().all(|e| !e.routed));
        assert_eq!(got.lock().unwrap().len(), 48);
    }

    /// Going to the back ends a routed stroke through its sink.
    #[test]
    fn a_cancel_ends_a_routed_stroke() {
        let mut i = Input::new("t");
        i.feed.push(pen(1.0, true, 1));
        let _ = i.poll();
        let ends = Arc::new(Mutex::new(0));
        let n = Arc::clone(&ends);
        assert!(i.route(Device::Pen, Arc::new(move |e: &Event| if matches!(e.kind, Kind::Up { .. }) { *n.lock().unwrap() += 1; }), &mut []));
        i.activated(false);
        assert!(i.poll().iter().any(|e| e.kind == Kind::Up { cancelled: true } && e.routed));
        assert_eq!(*ends.lock().unwrap(), 1);
    }

    /// One wake per batch of samples, until the app reads them.
    #[test]
    fn the_waker_wakes_once_until_read() {
        use std::sync::atomic::AtomicUsize;
        let mut i = Input::new("t");
        let n = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&n);
        i.set_waker(move || { m.fetch_add(1, Ordering::Relaxed); });
        i.waker.wake();
        i.waker.wake();
        assert_eq!(n.load(Ordering::Relaxed), 1);
        i.poll();
        i.waker.wake();
        assert_eq!(n.load(Ordering::Relaxed), 2);
    }
}

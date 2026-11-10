//! A freehand stroke's samples sent to the paint thread from the thread
//! that captured them (aurastudio-input's route): the pen's own thread
//! with WinTab, as each packet comes, instead of waiting for the
//! interface's loop (an egui frame, a present). The interface opens the
//! route at the press (its Begin and first sample sent first) and only
//! follows the cursor after; the route sends the stroke's end too, so
//! everything reaches the paint thread in the pen's order.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use aurastudio_forge::PenSample;
use aurastudio_gpu::ViewSnapshot;
use aurastudio_input::{Event, Kind, Sink};

use crate::paint_thread::Cmd;

/// What the route needs between two samples.
struct State {
    /// Window -> canvas, as the view was when the stroke began (kept up
    /// to date by the interface).
    view:     ViewSnapshot,
    /// Where the last sample was sent (window pixels), and its pressure
    /// (the end is drawn there at it, as the interface did).
    last:     (f32, f32),
    pressure: f32,
}

/// The route of one stroke: the sink handed to aurastudio-input, and the
/// view it maps through.
pub struct StrokeRoute {
    state: Arc<Mutex<State>>,
}

impl StrokeRoute {
    /// A route sending to `tx`; `last` / `pressure`: the sample the
    /// interface sent at the press.
    pub fn new(tx: Sender<Cmd>, view: ViewSnapshot, last: (f32, f32), pressure: f32) -> (StrokeRoute, Sink) {
        let state = Arc::new(Mutex::new(State { view, last, pressure }));
        let s = Arc::clone(&state);
        let sink: Sink = Arc::new(move |e: &Event| {
            let mut st = s.lock().unwrap_or_else(PoisonError::into_inner);
            let pos = (e.x as f32, e.y as f32);
            let time = e.time.0 as f64 / 1e6;
            let sample = |st: &State, pressure: f32| {
                let (x, y) = st.view.screen_to_canvas(pos);
                crate::latency::sent(time);
                Cmd::Sample(PenSample::new(x, y, pressure, time).with_pen(e.tilt, e.rotation))
            };
            match e.kind {
                Kind::Move => {
                    st.pressure = e.pressure;
                    st.last = pos;
                    let _ = tx.send(sample(&st, e.pressure));
                }
                Kind::Up { .. } => {
                    // The tip lifted where no sample went yet: the stroke
                    // ends there, at the pressure last given.
                    if st.last != pos { let _ = tx.send(sample(&st, st.pressure)); }
                    let _ = tx.send(Cmd::End);
                    crate::latency::end_stroke();
                }
                _ => {}
            }
        });
        (StrokeRoute { state }, sink)
    }

    /// The view changed during the stroke (the window resized).
    pub fn set_view(&self, view: ViewSnapshot) { self.state.lock().unwrap_or_else(PoisonError::into_inner).view = view; }
}

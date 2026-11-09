//! Pen → screen latency of each stroke, measured on its way: when the
//! device sampled it, when the app took it from the input queue, when the
//! paint thread had it painted and merged for the screen, when the frame
//! showing it was started and when it was handed to the display. At the
//! end of a stroke one line goes to the log; the last stroke's figures
//! show in the Memory report. The device side (tablet, USB, the screen's
//! own delay) is out of reach and the same in every app.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, PoisonError};

use aurastudio_core::Micros;
use aurastudio_input::clock;
use aurastudio_input::latency::{Latency, Stages, Trace};

/// Samples followed at most at once (a stroke far behind keeps the newest).
const MAX_WAITING: usize = 4096;

#[derive(Default)]
struct Probe {
    /// Sent to the paint thread, not merged for the screen yet.
    waiting: VecDeque<Trace>,
    /// Merged, not on a frame yet.
    painted: Vec<Trace>,
    latency: Latency,
    /// A stroke is followed.
    open:    bool,
    /// Its last sample is sent: done once all are shown.
    ending:  bool,
    /// The last stroke's figures.
    last:    Option<Stages>,
}

static PROBE: LazyLock<Mutex<Probe>> = LazyLock::new(Mutex::default);

fn probe() -> std::sync::MutexGuard<'static, Probe> { PROBE.lock().unwrap_or_else(PoisonError::into_inner) }

fn micros(secs: f64) -> Micros { Micros((secs.max(0.0) * 1e6) as u64) }

impl Probe {
    /// The stroke followed is over: told, kept for the report.
    fn finish(&mut self) {
        if !self.open { return; }
        self.open = false;
        self.ending = false;
        self.waiting.clear();
        self.painted.clear();
        let s = self.latency.stages();
        if s.samples == 0 { return; }
        log::info!("stroke latency: {}", self.latency.line());
        self.last = Some(s);
    }
}

impl Probe {
    fn begin_stroke(&mut self) {
        self.finish();
        self.latency.begin_stroke();
        self.open = true;
    }

    fn end_stroke(&mut self) {
        self.ending = true;
        if self.waiting.is_empty() && self.painted.is_empty() { self.finish(); }
    }

    fn sent(&mut self, sampled: f64, now: Micros) {
        if !self.open { return; }
        if self.waiting.len() >= MAX_WAITING { self.waiting.pop_front(); }
        self.waiting.push_back(Trace { sampled: micros(sampled), received: now, ..Trace::default() });
    }

    fn merged(&mut self, upto: f64, now: Micros) {
        let upto = micros(upto);
        while self.waiting.front().is_some_and(|t| t.sampled <= upto) {
            let Some(mut t) = self.waiting.pop_front() else { break };
            t.painted = now;
            self.painted.push(t);
        }
    }

    fn frame_begins(&mut self, now: Micros) -> Vec<Trace> {
        let mut v = std::mem::take(&mut self.painted);
        for t in &mut v { t.submitted = now; }
        v
    }

    fn frame_shown(&mut self, traces: Vec<Trace>, now: Micros) {
        if traces.is_empty() { return; }
        for mut t in traces {
            t.presented = now;
            self.latency.shown(t);
        }
        if self.ending && self.waiting.is_empty() && self.painted.is_empty() { self.finish(); }
    }

    fn report(&self) -> Vec<String> {
        let Some(s) = self.last else { return Vec::new() };
        let ms = |v: (u64, u64)| format!("{:>6.1} / {:>6.1} ms", v.0 as f64 / 1000.0, v.1 as f64 / 1000.0);
        vec![
            format!("  {}  pen → screen (median / worst, {} samples)", ms(s.total), s.samples),
            format!("  {}  input: the device to the app (driver, queue, event loop)", ms(s.input)),
            format!("  {}  paint: painted and merged on the paint thread", ms(s.paint)),
            format!("  {}  wait: merged, waiting for the next frame", ms(s.wait)),
            format!("  {}  frame: drawn, uploaded and handed to the display", ms(s.present)),
        ]
    }
}

/// A stroke begins (the one before, if not all shown, is told as it is).
pub fn begin_stroke() { probe().begin_stroke(); }

/// Its last sample was sent.
pub fn end_stroke() { probe().end_stroke(); }

/// A sample (seconds on the input clock) was sent to the paint thread.
pub fn sent(sampled: f64) { probe().sent(sampled, clock::now()); }

/// The paint thread merged for the screen every sample up to `upto`
/// (seconds on the input clock): its patches are on the projection.
pub fn merged(upto: f64) { probe().merged(upto, clock::now()); }

/// A frame is about to take the merged patches: the samples it shows
/// (taken before the patches, so none of them is missing from it).
pub fn frame_begins() -> Vec<Trace> { probe().frame_begins(clock::now()) }

/// That frame was handed to the display.
pub fn frame_shown(traces: Vec<Trace>) { if !traces.is_empty() { probe().frame_shown(traces, clock::now()); } }

/// The last stroke's figures, as lines for the report.
pub fn report() -> Vec<String> { probe().report() }

#[cfg(test)]
mod tests {
    use super::*;

    /// A stroke's samples followed to the screen give its report, split by
    /// stage.
    #[test]
    fn a_stroke_is_followed_to_the_screen() {
        let mut p = Probe::default();
        let ms = |v: u64| Micros(v * 1000);
        p.begin_stroke();
        // Sampled at 100, 101, 102 ms, each taken 1 ms later.
        for i in 0..3u64 { p.sent((100 + i) as f64 / 1000.0, ms(101 + i)); }
        p.end_stroke();
        assert!(p.open, "not over before it is shown");
        p.merged(0.1015, ms(105));
        let first = p.frame_begins(ms(110));
        assert_eq!(first.len(), 2);
        p.frame_shown(first, ms(120));
        p.merged(1.0, ms(125));
        let next = p.frame_begins(ms(126));
        p.frame_shown(next, ms(140));
        assert!(!p.open);
        let s = p.last.unwrap();
        assert_eq!((s.samples, s.input), (3, (1000, 1000)));
        assert_eq!(s.total.1, 38_000);
        let r = p.report();
        assert!(r.len() == 5 && r[0].contains("3 samples"), "{r:?}");
    }
}

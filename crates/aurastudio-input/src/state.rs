//! Raw device samples to stream events: one state machine per device
//! (away → near → down → up), the rule that every down gets its up, the
//! canvas / interface target locked at down, hover coalesced.

use std::collections::HashMap;

use aurastudio_core::{Micros, TransferCurve};

use crate::event::{Device, Event, Kind, Source, Target};

/// What a capture hook hands over, before any rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawSample {
    pub device:   Device,
    /// Window client pixels.
    pub x:        f64,
    pub y:        f64,
    /// 0..1 as the device gives it.
    pub pressure: f32,
    pub tilt:     (f32, f32),
    pub rotation: f32,
    /// The tip (or the left button) is down.
    pub contact:  bool,
    /// The device is near (a pen in range; a mouse always).
    pub in_range: bool,
    /// Other buttons held (bit 0: first barrel button / right, bit 1:
    /// second / middle…).
    pub buttons:  u8,
    pub time:     Micros,
}

/// What the app tells about the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameInfo {
    /// The canvas rect in window pixels (x0, y0, x1, y1).
    pub canvas: (f64, f64, f64, f64),
    /// The interface wants the pointer (a docker, a menu open over the
    /// canvas).
    pub ui_wants_pointer: bool,
}

impl FrameInfo {
    pub fn target(&self, x: f64, y: f64) -> Target {
        let (x0, y0, x1, y1) = self.canvas;
        if !self.ui_wants_pointer && x >= x0 && x < x1 && y >= y0 && y < y1 { Target::Canvas } else { Target::Ui }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State { Away, Near, Down(Target) }

#[derive(Clone, Copy, Debug)]
struct DeviceState { state: State, buttons: u8, last: Option<RawSample> }

/// Every device's state.
#[derive(Default)]
pub struct Machine {
    devices: HashMap<Device, DeviceState>,
    pub frame: FrameInfo,
    pub curve: TransferCurve,
}

impl Machine {
    fn event(&self, s: &RawSample, kind: Kind, target: Target) -> Event {
        Event {
            kind, device: s.device, source: Source::Live, target, x: s.x, y: s.y,
            pressure: if s.device == Device::Mouse { 1.0 } else { self.curve.eval(s.pressure) },
            pressure_raw: s.pressure, tilt: s.tilt, rotation: s.rotation, time: s.time,
        }
    }

    /// One sample: the events it makes, in order, pushed to `out`.
    pub fn feed(&mut self, s: RawSample, out: &mut Vec<Event>) {
        let d = *self.devices.entry(s.device).or_insert(DeviceState { state: State::Away, buttons: 0, last: None });
        let here = self.frame.target(s.x, s.y);
        let mut state = d.state;
        // Into range.
        if state == State::Away && (s.in_range || s.contact) { out.push(self.event(&s, Kind::ProximityIn, here)); state = State::Near; }
        // Buttons other than the tip.
        for b in 0..8u8 {
            let (was, is) = (d.buttons & (1 << b) != 0, s.buttons & (1 << b) != 0);
            let t = if let State::Down(t) = state { t } else { here };
            if is && !was { out.push(self.event(&s, Kind::ButtonDown(b), t)); }
            if was && !is { out.push(self.event(&s, Kind::ButtonUp(b), t)); }
        }
        state = match (state, s.contact) {
            (State::Near | State::Away, true) => { out.push(self.event(&s, Kind::Down, here)); State::Down(here) }
            (State::Down(t), true) => { out.push(self.event(&s, Kind::Move, t)); State::Down(t) }
            (State::Down(t), false) => { out.push(self.event(&s, Kind::Up { cancelled: false }, t)); State::Near }
            (st, false) => { if s.in_range { out.push(self.event(&s, Kind::Hover, here)); } st }
        };
        // Out of range (a stroke still down is ended first).
        if !s.in_range && !s.contact && state != State::Away {
            out.push(self.event(&s, Kind::ProximityOut, here));
            state = State::Away;
        }
        self.devices.insert(s.device, DeviceState { state, buttons: s.buttons, last: Some(s) });
    }

    /// End every stroke in progress (focus lost, display change, sleep,
    /// context lost): an `Up { cancelled }` at the last position of each.
    pub fn cancel_all(&mut self, now: Micros, out: &mut Vec<Event>) {
        let ended: Vec<(Device, RawSample, Target)> = self.devices.iter()
            .filter_map(|(d, st)| match (st.state, st.last) { (State::Down(t), Some(l)) => Some((*d, l, t)), _ => None })
            .collect();
        for (d, mut last, t) in ended {
            last.time = now;
            out.push(self.event(&last, Kind::Up { cancelled: true }, t));
            if let Some(st) = self.devices.get_mut(&d) { st.state = State::Near; }
        }
    }

    /// Whether some device is down (a stroke in progress).
    pub fn any_down(&self) -> bool { self.devices.values().any(|d| matches!(d.state, State::Down(_))) }

    /// Whether `device` is down on the canvas.
    pub fn down_on_canvas(&self, device: Device) -> bool { matches!(self.devices.get(&device).map(|d| d.state), Some(State::Down(Target::Canvas))) }
}

/// Keep only the last of consecutive hovers of a device (outside strokes,
/// only the newest position matters).
pub fn coalesce_hover(events: &mut Vec<Event>) {
    let mut out: Vec<Event> = Vec::with_capacity(events.len());
    for e in events.drain(..) {
        if let (Kind::Hover, Some(last)) = (e.kind, out.last_mut()) {
            if last.kind == Kind::Hover && last.device == e.device { *last = e; continue; }
        }
        out.push(e);
    }
    *events = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(device: Device, x: f64, contact: bool, in_range: bool, t: u64) -> RawSample {
        RawSample { device, x, y: 10.0, pressure: 0.5, tilt: (0.0, 0.0), rotation: 0.0, contact, in_range, buttons: 0, time: Micros(t) }
    }

    fn kinds(ev: &[Event]) -> Vec<Kind> { ev.iter().map(|e| e.kind).collect() }

    #[test]
    fn a_pen_stroke() {
        let mut m = Machine { frame: FrameInfo { canvas: (0.0, 0.0, 100.0, 100.0), ui_wants_pointer: false }, ..Default::default() };
        let mut out = Vec::new();
        for (x, c, r) in [(5.0, false, true), (6.0, true, true), (7.0, true, true), (8.0, false, true), (9.0, false, false)] {
            m.feed(s(Device::Pen, x, c, r, 0), &mut out);
        }
        assert_eq!(kinds(&out), vec![Kind::ProximityIn, Kind::Hover, Kind::Down, Kind::Move, Kind::Up { cancelled: false }, Kind::ProximityOut]);
        assert!(out.iter().all(|e| e.target == Target::Canvas));
    }

    #[test]
    fn the_target_is_locked_at_down() {
        let mut m = Machine { frame: FrameInfo { canvas: (0.0, 0.0, 100.0, 100.0), ui_wants_pointer: false }, ..Default::default() };
        let mut out = Vec::new();
        m.feed(s(Device::Mouse, 50.0, true, true, 0), &mut out);
        m.feed(s(Device::Mouse, 150.0, true, true, 1), &mut out);
        m.feed(s(Device::Mouse, 150.0, false, true, 2), &mut out);
        assert!(out.iter().filter(|e| matches!(e.kind, Kind::Move | Kind::Up { .. })).all(|e| e.target == Target::Canvas), "a stroke over a docker stays on the canvas");
        // A click started with a menu open over the canvas never paints.
        m.frame.ui_wants_pointer = true;
        out.clear();
        m.feed(s(Device::Mouse, 50.0, true, true, 3), &mut out);
        m.frame.ui_wants_pointer = false;
        m.feed(s(Device::Mouse, 51.0, true, true, 4), &mut out);
        assert!(out.iter().filter(|e| matches!(e.kind, Kind::Down | Kind::Move)).all(|e| e.target == Target::Ui));
    }

    #[test]
    fn every_down_gets_its_up() {
        let mut m = Machine::default();
        let mut out = Vec::new();
        m.feed(s(Device::Pen, 1.0, true, true, 0), &mut out);
        m.feed(s(Device::Mouse, 1.0, true, true, 0), &mut out);
        out.clear();
        m.cancel_all(Micros(9), &mut out);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.kind == Kind::Up { cancelled: true } && e.time == Micros(9)));
        assert!(!m.any_down());
        // A pen leaving range while down: up, then out.
        m.feed(s(Device::Pen, 1.0, true, true, 0), &mut out);
        out.clear();
        m.feed(s(Device::Pen, 1.0, false, false, 0), &mut out);
        assert_eq!(kinds(&out), vec![Kind::Up { cancelled: false }, Kind::ProximityOut]);
    }

    #[test]
    fn the_global_curve_applies_to_pens_not_mice() {
        let mut m = Machine { curve: TransferCurve::new(&[(0.0, 0.0), (0.5, 0.2), (1.0, 1.0)]), ..Default::default() };
        let mut out = Vec::new();
        m.feed(s(Device::Pen, 1.0, true, true, 0), &mut out);
        m.feed(s(Device::Mouse, 1.0, true, true, 0), &mut out);
        let pen = out.iter().find(|e| e.device == Device::Pen && e.is_down()).map(|e| (e.pressure, e.pressure_raw));
        assert!(pen.is_some_and(|(p, r)| (p - 0.2).abs() < 0.01 && r == 0.5));
        assert!(out.iter().filter(|e| e.device == Device::Mouse).all(|e| e.pressure == 1.0));
    }

    #[test]
    fn buttons_and_hover_coalescing() {
        let mut m = Machine::default();
        let mut out = Vec::new();
        let mut b = s(Device::Pen, 1.0, false, true, 0);
        m.feed(b, &mut out);
        b.buttons = 1;
        m.feed(b, &mut out);
        b.buttons = 0;
        m.feed(b, &mut out);
        assert!(kinds(&out).contains(&Kind::ButtonDown(0)) && kinds(&out).contains(&Kind::ButtonUp(0)));
        let mut h = Vec::new();
        for x in 0..5 { m.feed(s(Device::Pen, x as f64, false, true, x), &mut h); }
        coalesce_hover(&mut h);
        assert_eq!(h.iter().filter(|e| e.kind == Kind::Hover).count(), 1);
        assert_eq!(h.last().map(|e| e.x), Some(4.0));
    }

    /// Random sequences of samples on random devices, with cancels: the
    /// downs and ups always pair up per device.
    #[test]
    fn fuzz_downs_and_ups_pair_up() {
        let mut r = aurastudio_core::Rng::new(7);
        for _ in 0..200 {
            let mut m = Machine::default();
            let mut out = Vec::new();
            for t in 0..200 {
                if r.chance(0.05) { m.cancel_all(Micros(t), &mut out); continue; }
                let d = [Device::Pen, Device::Eraser, Device::Mouse, Device::Touch(1)][r.below(4) as usize];
                let sample = RawSample { device: d, x: r.range(-10.0, 110.0) as f64, y: 0.0, pressure: r.unit(), tilt: (0.0, 0.0), rotation: 0.0, contact: r.chance(0.5), in_range: r.chance(0.8), buttons: r.below(4) as u8, time: Micros(t) };
                m.feed(sample, &mut out);
            }
            m.cancel_all(Micros(999), &mut out);
            for d in [Device::Pen, Device::Eraser, Device::Mouse, Device::Touch(1)] {
                let mut depth = 0i32;
                for e in out.iter().filter(|e| e.device == d) {
                    if e.is_down() { depth += 1; assert_eq!(depth, 1, "two downs in a row"); }
                    if e.is_up() { depth -= 1; assert_eq!(depth, 0, "an up without a down"); }
                }
                assert_eq!(depth, 0, "a down without its up");
            }
        }
    }
}

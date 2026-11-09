//! The one stream of pointing events: every device, live or replayed.

use aurastudio_core::Micros;

/// What points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    Pen,
    /// The pen turned over.
    Eraser,
    Mouse,
    /// A finger (its contact id).
    Touch(u32),
}

/// Where an event comes from: a strokelog event keeps its original device
/// (pen and mouse don't drive brushes the same way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source { Live, Strokelog }

/// Who receives it: tagged from the last frame's layout and locked at
/// down (a stroke started on the canvas stays on it, a click started on
/// the interface never paints).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target { Canvas, Ui }

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The tip touches (the left button for a mouse).
    Down,
    Move,
    /// The tip lifts; `cancelled`: ended by the system (focus lost, sleep,
    /// pen out of range during the stroke), not by the hand.
    Up { cancelled: bool },
    /// Moving without contact.
    Hover,
    /// A pen barrel button or a mouse button other than the left one.
    ButtonDown(u8),
    ButtonUp(u8),
    ProximityIn,
    ProximityOut,
    /// Wheel or touchpad scroll, in lines (fractions allowed).
    Wheel { dx: f32, dy: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    pub kind:     Kind,
    pub device:   Device,
    pub source:   Source,
    pub target:   Target,
    /// Window client pixels, physical, with fractions.
    pub x:        f64,
    pub y:        f64,
    /// 0..1 after the app's global curve (mouse: 1).
    pub pressure: f32,
    /// 0..1 as the device gave it (the strokelog keeps this one).
    pub pressure_raw: f32,
    /// Degrees, −90..90 toward +x / +y (W3C Pointer Events); 0 without.
    pub tilt:     (f32, f32),
    /// Degrees 0..360 (barrel rotation); 0 without.
    pub rotation: f32,
    /// On the app's one clock.
    pub time:     Micros,
}

impl Event {
    #[cfg(test)]
    pub fn is_down(&self) -> bool { matches!(self.kind, Kind::Down) }
    #[cfg(test)]
    pub fn is_up(&self) -> bool { matches!(self.kind, Kind::Up { .. }) }
}

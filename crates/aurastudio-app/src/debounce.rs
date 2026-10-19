//! Mouse button bounce filter, in front of everything (UI, canvas, tools).
//!
//! A worn mouse switch "chatters": while held, it opens and closes again
//! within a few ms. Without a filter, that is a real release then a real
//! press: a slider drag stops, a stroke ends, an arc gets validated. Each
//! left button release is held back `window` (View menu, 20 ms by
//! default: old mice bounce for 15-20 ms); a press coming back in that
//! time cancels both (the button stays down). Cursor moves in between are
//! held too, so nothing sees them before it knows whether the button was
//! released: after a real release they come after it (no stroke tail),
//! after a bounce they continue the drag.

use std::time::{Duration, Instant};

/// Default window: old mice bounce for 15-20 ms.
pub const DEFAULT_MS: u32 = 20;
#[cfg(test)]
pub const BOUNCE: Duration = Duration::from_millis(DEFAULT_MS as u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { LeftPress, LeftRelease, Move, Other }

pub struct Debounce<E> {
    /// How long a release is held back (0: no filter).
    pub window: Duration,
    /// The release held back, until when.
    held:  Option<(Instant, E)>,
    /// Cursor moves since.
    moves: Vec<E>,
    /// Bounces ignored so far (logged).
    pub bounces: u64,
    /// When the last release came in, and what the next press found:
    /// the gap between them, for the log (a bounce longer than the window
    /// shows there as a short release).
    released_at: Option<Instant>,
    pub last_gap: Option<Duration>,
}

impl<E> Default for Debounce<E> {
    fn default() -> Self {
        Self { window: Duration::from_millis(DEFAULT_MS as u64), held: None, moves: Vec::new(), bounces: 0, released_at: None, last_gap: None }
    }
}

impl<E> Debounce<E> {
    /// An event came in: the events to handle now, in order.
    pub fn event(&mut self, ev: E, kind: Kind, now: Instant) -> Vec<E> {
        let mut out = self.poll(now);
        if kind == Kind::LeftPress { self.last_gap = self.released_at.take().map(|t| now.duration_since(t)); }
        if kind == Kind::LeftRelease { self.released_at = Some(now); }
        if self.window.is_zero() {
            out.push(ev);
            return out;
        }
        match kind {
            Kind::LeftRelease => {
                // A release while one is held: can't happen (a press comes
                // between); hand the old one over.
                out.extend(self.flush());
                self.held = Some((now + self.window, ev));
            }
            Kind::LeftPress if self.held.is_some() => {
                // Back within BOUNCE (poll handed it over otherwise): the
                // button never went up. Drop both, the moves go on.
                self.held = None;
                self.bounces += 1;
                out.append(&mut self.moves);
            }
            Kind::Move if self.held.is_some() => self.moves.push(ev),
            _ => out.push(ev),
        }
        out
    }

    /// Time passed: the held release and its moves once BOUNCE is over.
    pub fn poll(&mut self, now: Instant) -> Vec<E> {
        match self.held {
            Some((t, _)) if now >= t => self.flush(),
            _ => Vec::new(),
        }
    }

    /// Everything held, now (focus lost...).
    pub fn flush(&mut self) -> Vec<E> {
        let mut out = Vec::new();
        if let Some((_, e)) = self.held.take() { out.push(e); }
        out.append(&mut self.moves);
        out
    }

    /// When `poll` has something to hand over.
    pub fn deadline(&self) -> Option<Instant> { self.held.as_ref().map(|(t, _)| *t) }

    /// A release is held back (the button may still be down).
    pub fn holding(&self) -> bool { self.held.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, n: u64) -> Instant { t0 + Duration::from_millis(n) }

    #[test]
    fn bounce_is_dropped_moves_continue() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        assert_eq!(d.event("press", Kind::LeftPress, t0), vec!["press"]);
        assert_eq!(d.event("m1", Kind::Move, ms(t0, 10)), vec!["m1"]);
        assert!(d.event("release", Kind::LeftRelease, ms(t0, 20)).is_empty());
        assert!(d.event("m2", Kind::Move, ms(t0, 25)).is_empty());
        assert_eq!(d.event("press2", Kind::LeftPress, ms(t0, 32)), vec!["m2"]);
        assert_eq!(d.last_gap, Some(Duration::from_millis(12)));
        assert_eq!(d.bounces, 1);
        assert!(!d.holding());
    }

    #[test]
    fn real_release_comes_first_then_the_moves() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        d.event("press", Kind::LeftPress, t0);
        assert!(d.event("release", Kind::LeftRelease, ms(t0, 100)).is_empty());
        assert!(d.event("m", Kind::Move, ms(t0, 105)).is_empty());
        assert_eq!(d.event("key", Kind::Other, ms(t0, 110)), vec!["key"]);
        assert_eq!(d.deadline(), Some(ms(t0, 120)));
        assert!(d.poll(ms(t0, 119)).is_empty());
        assert_eq!(d.poll(ms(t0, 120)), vec!["release", "m"]);
        // A click after that is a click.
        assert_eq!(d.event("press", Kind::LeftPress, ms(t0, 300)), vec!["press"]);
    }

    #[test]
    fn late_press_without_poll_gets_the_release_first() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        d.event("release", Kind::LeftRelease, t0);
        assert_eq!(d.event("press", Kind::LeftPress, ms(t0, 30)), vec!["release", "press"]);
        assert_eq!(d.bounces, 0);
    }
}

#[cfg(test)]
mod off {
    use super::*;
    #[test]
    fn zero_window_passes_everything() {
        let t0 = Instant::now();
        let mut d = Debounce { window: Duration::ZERO, ..Default::default() };
        assert_eq!(d.event("r", Kind::LeftRelease, t0), vec!["r"]);
        assert_eq!(d.event("p", Kind::LeftPress, t0), vec!["p"]);
    }
}

//! Mouse button bounce filter, in front of everything (UI, canvas, tools).
//!
//! A worn mouse switch "chatters": while held, it opens and closes again
//! within a few ms. Without a filter, that is a real release then a real
//! press: a slider drag stops, a stroke ends, an arc gets validated. Each
//! left button release is held back BOUNCE; a press coming back in that
//! time cancels both (the button stays down). Cursor moves in between are
//! held too, so nothing sees them before it knows whether the button was
//! released: after a real release they come after it (no stroke tail),
//! after a bounce they continue the drag.

use std::time::{Duration, Instant};

/// Longer than any switch bounce (a few ms, 20 at worst), shorter than a
/// deliberate release and new click.
pub const BOUNCE: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { LeftPress, LeftRelease, Move, Other }

pub struct Debounce<E> {
    /// The release held back, until when.
    held:  Option<(Instant, E)>,
    /// Cursor moves since.
    moves: Vec<E>,
    /// Bounces ignored so far (logged).
    pub bounces: u64,
}

impl<E> Default for Debounce<E> {
    fn default() -> Self { Self { held: None, moves: Vec::new(), bounces: 0 } }
}

impl<E> Debounce<E> {
    /// An event came in: the events to handle now, in order.
    pub fn event(&mut self, ev: E, kind: Kind, now: Instant) -> Vec<E> {
        let mut out = self.poll(now);
        match kind {
            Kind::LeftRelease => {
                // A release while one is held: can't happen (a press comes
                // between); hand the old one over.
                out.extend(self.flush());
                self.held = Some((now + BOUNCE, ev));
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
        assert_eq!(d.event("press2", Kind::LeftPress, ms(t0, 30)), vec!["m2"]);
        assert_eq!(d.bounces, 1);
        assert!(!d.holding());
    }

    #[test]
    fn real_release_comes_first_then_the_moves() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        d.event("press", Kind::LeftPress, t0);
        assert!(d.event("release", Kind::LeftRelease, ms(t0, 100)).is_empty());
        assert!(d.event("m", Kind::Move, ms(t0, 110)).is_empty());
        assert_eq!(d.event("key", Kind::Other, ms(t0, 120)), vec!["key"]);
        assert_eq!(d.deadline(), Some(ms(t0, 150)));
        assert!(d.poll(ms(t0, 149)).is_empty());
        assert_eq!(d.poll(ms(t0, 150)), vec!["release", "m"]);
        // A click after that is a click.
        assert_eq!(d.event("press", Kind::LeftPress, ms(t0, 300)), vec!["press"]);
    }

    #[test]
    fn late_press_without_poll_gets_the_release_first() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        d.event("release", Kind::LeftRelease, t0);
        assert_eq!(d.event("press", Kind::LeftPress, ms(t0, 80)), vec!["release", "press"]);
        assert_eq!(d.bounces, 0);
    }
}

//! The arc tool's state between clicks (it lives across presses and
//! releases), modeled on OpenToonz's arc / multi-arc tool. Kept apart
//! from the window code so press / release / move sequences can be
//! tested, mouse button bounce included (filtered before, debounce.rs).

use crate::shapes;

type P = (f32, f32);

/// Distance (screen pixels) at which an arc's end sticks to the first
/// point of the chain, closing the outline (OpenToonz: 5).
pub const ARC_SNAP: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArcState {
    /// Start placed; the end follows the cursor. `held`: the button is
    /// down (dragging to the end); up: waiting for a click on the end.
    Placing { start: P, end: P, held: bool },
    /// Both ends placed: the cursor is the curve's control point (it pulls
    /// the curve toward itself); a click validates.
    Bending { start: P, end: P, control: P },
}

/// What the tool needs from the window at each event.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    /// Screen pixels per canvas pixel.
    pub zoom: f32,
    /// Shift held: the end snaps to 15° steps.
    pub shift: bool,
    /// Chain mode: the next arc starts where the last one ended.
    pub chain_arcs: bool,
}

#[derive(Default)]
pub struct ArcTool {
    pub state: Option<ArcState>,
    /// Arcs validated so far (chain mode): one path, painted as one stroke
    /// when the outline is done; `marks`: where each arc starts in it.
    chain: Vec<P>,
    marks: Vec<usize>,
    /// The press that validated an arc: its release starts nothing.
    skip_release: bool,
}

impl ArcTool {
    pub fn active(&self) -> bool { self.state.is_some() }

    /// A release now concerns the tool.
    pub fn wants_release(&self) -> bool { self.state.is_some() || self.skip_release }

    /// Forget a pending release (tool switch, focus lost).
    pub fn reset_release(&mut self) {
        self.skip_release = false;
    }

    /// Where the arc's end goes for the cursor at `at`: Shift snaps the
    /// direction (15° steps), and near the chain's first point it sticks
    /// to it.
    fn end_for(&self, start: P, at: P, cx: Ctx) -> P {
        let at = if cx.shift { shapes::snap_angle(start, at) } else { at };
        match self.chain.first() {
            Some(&f) if dist(at, f) * cx.zoom < ARC_SNAP => f,
            _ => at,
        }
    }

    /// Button pressed on the canvas. Returns a path to paint when this
    /// press ends the outline.
    pub fn press(&mut self, at: P, cx: Ctx) -> Option<Vec<P>> {
        let near = |a: P, b: P| dist(a, b) * cx.zoom < ARC_SNAP;
        let mut painted = None;
        self.state = match self.state {
            None => Some(ArcState::Placing { start: at, end: at, held: true }),
            // Clicking again on the last end point (a double click right
            // after validating): the outline is done, open.
            Some(ArcState::Placing { start, held: false, .. }) if !self.chain.is_empty() && near(at, start) => {
                painted = self.finish();
                None
            }
            Some(ArcState::Placing { start, .. }) => Some(ArcState::Placing { start, end: self.end_for(start, at, cx), held: true }),
            Some(a @ ArcState::Bending { end, .. }) => {
                // Validate: add it to the chain (one stroke, painted when
                // the outline is done).
                let skip = usize::from(!self.chain.is_empty());
                self.marks.push(self.chain.len());
                self.chain.extend(path_of(a).into_iter().skip(skip));
                self.skip_release = true;
                let closed = self.chain.len() > 2 && self.chain.first() == Some(&end);
                if !cx.chain_arcs || closed {
                    painted = self.finish();
                    None
                } else {
                    Some(ArcState::Placing { start: end, end: at, held: false })
                }
            }
        };
        painted
    }

    /// Button released (anywhere).
    pub fn release(&mut self, at: P, cx: Ctx) {
        if std::mem::take(&mut self.skip_release) { return; }
        if let Some(ArcState::Placing { start, held: true, .. }) = self.state {
            let end = self.end_for(start, at, cx);
            let far = (end.0 - start.0).abs().max((end.1 - start.1).abs()) * cx.zoom >= 4.0;
            // Dragged to the end: bend now (the control point starts on
            // the end: a straight line); a click on the start: wait for a
            // second click on the end.
            self.state = Some(if far {
                ArcState::Bending { start, end, control: end }
            } else {
                ArcState::Placing { start, end, held: false }
            });
        }
    }

    /// Cursor moved (button up or down). True if the preview changed.
    pub fn moved(&mut self, at: P, cx: Ctx) -> bool {
        self.state = match self.state {
            Some(ArcState::Placing { start, held, .. }) => Some(ArcState::Placing { start, end: self.end_for(start, at, cx), held }),
            Some(ArcState::Bending { start, end, .. }) => Some(ArcState::Bending { start, end, control: at }),
            None => return false,
        };
        true
    }

    /// The outline is done (Enter, a click on its last point, closing it,
    /// or leaving the tool): the validated arcs, to paint as one stroke.
    pub fn finish(&mut self) -> Option<Vec<P>> {
        let chain = std::mem::take(&mut self.chain);
        self.marks.clear();
        self.state = None;
        (chain.len() >= 2).then_some(chain)
    }

    /// Escape: drop the outline in progress, nothing painted.
    pub fn cancel(&mut self) {
        self.chain.clear();
        self.marks.clear();
        self.state = None;
    }

    /// Ctrl+Z while building an outline: take back its last arc.
    pub fn undo(&mut self, cursor: P) {
        let Some(mark) = self.marks.pop() else { self.cancel(); return };
        // Back to where that arc started (the first arc: nothing left).
        self.chain.truncate(mark);
        self.state = self.chain.last().map(|&s| ArcState::Placing { start: s, end: cursor, held: false });
    }

    /// The path (the chain validated so far plus the current arc), its
    /// handles, and the control lines (start, control point, end).
    pub fn preview(&self) -> Option<(Vec<P>, Vec<P>, Vec<P>)> {
        let a = self.state?;
        let mut path = self.chain.clone();
        path.extend(path_of(a));
        let (start, end, guide) = match a {
            ArcState::Placing { start, end, .. } => (start, end, Vec::new()),
            ArcState::Bending { start, end, control } => (start, end, vec![start, control, end]),
        };
        let mut handles = vec![start, end];
        // The chain's first point: an end dropped on it closes the outline.
        if let Some(&first) = self.chain.first() { handles.push(first); }
        Some((path, handles, guide))
    }
}

/// The arc as it would be painted now.
fn path_of(a: ArcState) -> Vec<P> {
    match a {
        ArcState::Placing { start, end, .. } => shapes::line(start, end),
        ArcState::Bending { start, end, control } => shapes::arc(start, end, control),
    }
}

fn dist(a: P, b: P) -> f32 { ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() }

#[cfg(test)]
mod tests {
    use super::*;

    const CX: Ctx = Ctx { zoom: 1.0, shift: false, chain_arcs: true };
    const SINGLE: Ctx = Ctx { zoom: 1.0, shift: false, chain_arcs: false };

    use crate::debounce::{Debounce, Kind, BOUNCE};
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy)]
    enum Ev { Down(P), Up(P), Move(P) }

    /// Drives the tool like the window does: events through the bounce
    /// filter, a clock in ms.
    struct Mouse { tool: ArcTool, filter: Debounce<Ev>, t0: Instant, ms: u64, painted: Vec<Vec<P>>, cx: Ctx }

    impl Mouse {
        fn new(cx: Ctx) -> Self { Self { tool: ArcTool::default(), filter: Debounce::default(), t0: Instant::now(), ms: 0, painted: Vec::new(), cx } }
        fn now(&self) -> Instant { self.t0 + Duration::from_millis(self.ms) }
        fn handle(&mut self, evs: Vec<Ev>) {
            for e in evs {
                match e {
                    Ev::Down(at) => if let Some(p) = self.tool.press(at, self.cx) { self.painted.push(p) },
                    Ev::Up(at) => if self.tool.wants_release() { self.tool.release(at, self.cx) },
                    Ev::Move(at) => { self.tool.moved(at, self.cx); }
                }
            }
        }
        fn send(&mut self, e: Ev, k: Kind) -> &mut Self {
            let now = self.now();
            let evs = self.filter.event(e, k, now);
            self.handle(evs);
            self
        }
        fn wait(&mut self, ms: u64) -> &mut Self {
            self.ms += ms;
            let now = self.now();
            let evs = self.filter.poll(now);
            self.handle(evs);
            self
        }
        fn down(&mut self, at: P) -> &mut Self { self.send(Ev::Down(at), Kind::LeftPress) }
        fn up(&mut self, at: P) -> &mut Self { self.send(Ev::Up(at), Kind::LeftRelease) }
        /// Move in steps of 8 ms (a 125 Hz mouse).
        fn drag(&mut self, from: P, to: P, steps: u32) -> &mut Self {
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                self.wait(8);
                self.send(Ev::Move((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)), Kind::Move);
            }
            self
        }
        fn enter(&mut self) -> &mut Self {
            self.wait(BOUNCE.as_millis() as u64);
            if let Some(p) = self.tool.finish() { self.painted.push(p); }
            self
        }
    }

    /// Farthest distance of the path from the straight line a-b.
    fn bulge(path: &[P], a: P, b: P) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        path.iter().map(|p| ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len).fold(0.0, f32::max)
    }

    /// Drag start → end, release, move to bend, click: one curved arc.
    fn one_arc(m: &mut Mouse, a: P, b: P, c: P) {
        m.down(a).drag(a, b, 30).up(b).wait(200).drag(b, c, 20).wait(150).down(c).wait(90).up(c).wait(BOUNCE.as_millis() as u64);
    }

    #[test]
    fn drag_bend_click_paints_a_curve() {
        let mut m = Mouse::new(SINGLE);
        one_arc(&mut m, (100.0, 100.0), (400.0, 100.0), (300.0, 250.0));
        assert_eq!(m.painted.len(), 1);
        let p = &m.painted[0];
        assert!(dist(p[0], (100.0, 100.0)) < 0.01 && dist(*p.last().unwrap(), (400.0, 100.0)) < 0.1);
        assert!(bulge(p, (100.0, 100.0), (400.0, 100.0)) > 60.0, "should be curved");
        assert!(!m.tool.active());
    }

    #[test]
    fn click_click_then_bend_paints_a_curve() {
        let mut m = Mouse::new(SINGLE);
        let (a, b, c) = ((50.0, 50.0), (250.0, 80.0), (150.0, 200.0));
        m.down(a).wait(90).up(a).wait(60).drag(a, b, 25).down(b).wait(90).up(b).wait(60).drag(b, c, 20).down(c).wait(90).up(c).wait(60);
        assert_eq!(m.painted.len(), 1);
        assert!(bulge(&m.painted[0], a, b) > 40.0);
    }

    /// The reported bug: the button bounces mid-drag (release + press a
    /// few ms apart). It used to end the drag there and validate a
    /// straight arc from the start to that point.
    #[test]
    fn bounce_mid_drag_is_ignored() {
        for chain in [SINGLE, CX] {
            for bounce_ms in [1, 5, 10, 15, 19] {
                let mut m = Mouse::new(chain);
                let (a, mid, b, c) = ((100.0, 100.0), (250.0, 100.0), (400.0, 100.0), (300.0, 250.0));
                m.down(a).drag(a, mid, 15).up(mid).wait(bounce_ms).down(mid).drag(mid, b, 15).up(b).wait(60);
                assert!(m.painted.is_empty(), "painted mid-drag ({bounce_ms} ms)");
                assert_eq!(m.tool.state, Some(ArcState::Bending { start: a, end: b, control: b }), "{bounce_ms} ms");
                m.wait(200).drag(b, c, 20).down(c).wait(90).up(c).enter();
                assert_eq!(m.painted.len(), 1, "{bounce_ms} ms");
                let p = &m.painted[0];
                assert!(dist(*p.last().unwrap(), b) < 0.1 && bulge(p, a, b) > 60.0);
            }
        }
    }

    #[test]
    fn several_bounces_in_one_drag() {
        let mut m = Mouse::new(SINGLE);
        let (a, b) = ((0.0, 0.0), (500.0, 0.0));
        m.down(a);
        let mut x = 0.0;
        for _ in 0..5 {
            m.drag((x, 0.0), (x + 100.0, 0.0), 10);
            x += 100.0;
            m.up((x, 0.0)).wait(10).down((x, 0.0));
        }
        m.up(b).wait(60);
        assert!(m.painted.is_empty());
        assert_eq!(m.tool.state, Some(ArcState::Bending { start: a, end: b, control: b }));
    }

    /// A bounce on the validating click: chain mode must not take it for
    /// the double click that ends the outline, single mode must not start
    /// a new arc.
    #[test]
    fn bounce_on_the_validating_click() {
        let (a, b, c) = ((100.0, 100.0), (400.0, 100.0), (300.0, 250.0));
        let mut m = Mouse::new(CX);
        m.down(a).drag(a, b, 30).up(b).wait(200).drag(b, c, 20).down(c).wait(40).up(c).wait(8).down(c).wait(60).up(c).wait(60);
        assert!(m.painted.is_empty(), "the outline ended");
        assert!(matches!(m.tool.state, Some(ArcState::Placing { start, held: false, .. }) if start == b));

        let mut m = Mouse::new(SINGLE);
        m.down(a).drag(a, b, 30).up(b).wait(200).drag(b, c, 20).down(c).wait(40).up(c).wait(8).down(c).wait(60).up(c).wait(60);
        assert_eq!(m.painted.len(), 1);
        assert!(!m.tool.active(), "a bounce started a new arc");
    }

    /// Slower than a bounce: real clicks keep working (a quick second
    /// click on the last point ends the chain).
    #[test]
    fn real_double_click_still_ends_the_chain() {
        let (a, b, c) = ((100.0, 100.0), (400.0, 100.0), (300.0, 250.0));
        let mut m = Mouse::new(CX);
        m.down(a).drag(a, b, 30).up(b).wait(200).drag(b, c, 20).down(c).wait(70).up(c);
        // Back on the end point, second click well after the bounce window.
        m.drag(c, b, 5).wait(BOUNCE.as_millis() as u64 + 20).down(b).wait(70).up(b).wait(60);
        assert_eq!(m.painted.len(), 1);
        assert!(!m.tool.active());
    }

    #[test]
    fn chained_arcs_close_and_paint_once() {
        let mut m = Mouse::new(CX);
        let (a, b, c) = ((100.0, 100.0), (300.0, 100.0), (200.0, 300.0));
        one_arc(&mut m, a, b, (200.0, 50.0));
        assert!(m.painted.is_empty());
        // Next arc from b: click on c, bend, click.
        m.wait(100).drag(b, c, 20).down(c).wait(80).up(c).wait(60).drag(c, (300.0, 250.0), 10).wait(100).down((300.0, 250.0)).wait(80).up((300.0, 250.0)).wait(60);
        // Last arc ends on the first point (within the snap): closes.
        m.wait(100).drag(c, (102.0, 103.0), 20).down((102.0, 103.0)).wait(80).up((102.0, 103.0)).wait(60).drag(a, (120.0, 250.0), 10).wait(100).down((120.0, 250.0)).wait(80).up((120.0, 250.0)).wait(60);
        assert_eq!(m.painted.len(), 1, "closed outline painted once");
        let p = &m.painted[0];
        assert_eq!(p.first(), p.last());
    }

    #[test]
    fn undo_and_cancel() {
        let mut m = Mouse::new(CX);
        let (a, b) = ((0.0, 0.0), (200.0, 0.0));
        one_arc(&mut m, a, b, (100.0, 80.0));
        m.tool.undo((50.0, 50.0));
        assert!(!m.tool.active() && m.tool.finish().is_none());
        one_arc(&mut m, a, b, (100.0, 80.0));
        m.tool.cancel();
        assert!(m.tool.finish().is_none() && m.painted.is_empty());
    }

    /// Random presses, releases, moves and waits: never panics, never
    /// leaves a held placing state without the button down for long.
    #[test]
    fn random_sequences() {
        let mut seed = 99u64;
        let mut rnd = |n: u64| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) % n };
        for round in 0..300 {
            let mut m = Mouse::new(if round % 2 == 0 { CX } else { SINGLE });
            let mut down = false;
            for _ in 0..60 {
                let at = (rnd(800) as f32, rnd(600) as f32);
                match rnd(5) {
                    0 if !down => { m.down(at); down = true; }
                    1 if down => { m.up(at); down = false; }
                    2 => { m.wait(rnd(120)); }
                    3 => { m.send(Ev::Move(at), Kind::Move); }
                    _ => { let p = m.tool.preview(); assert!(p.is_none() || !p.unwrap().0.is_empty()); }
                }
                if !down && !m.filter.holding() {
                    assert!(!matches!(m.tool.state, Some(ArcState::Placing { held: true, .. })), "held with the button up");
                }
            }
        }
    }
}

//! Alt + right drag over the canvas (Photoshop): horizontal changes the
//! brush size, vertical its hardness.
//!
//! Never both at once: Photoshop changes both with every move, so a drag
//! that isn't perfectly level also nudges the hardness and the exact size
//! is hard to reach. Here the drag first picks an axis from its first few
//! pixels (within 45° of horizontal: size; else hardness) and only that
//! value changes; the other part of each move is dropped. The axis can
//! change within the same drag: right to grow the brush, then up to
//! soften it. It switches once the last `WINDOW` points of movement go
//! more along the other axis (past 45°), so a slight drift never does.

/// Points of movement before the first axis is picked.
const START: f32 = 4.0;
/// Points of movement over which a switch of axis is judged.
const WINDOW: f32 = 10.0;
/// Hardness change per point (0.5 %: 200 points for the whole range).
const HARDNESS_PER_POINT: f32 = 0.005;
/// Diameter range, pixels.
pub const MIN_SIZE: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis { Size, Hardness }

pub struct SizeDrag {
    /// Where the drag started (window pixels): the brush is shown there.
    pub anchor: (f32, f32),
    /// The value being changed (None: not picked yet).
    pub axis:   Option<Axis>,
    /// The last pointer position (points).
    last:       (f32, f32),
    /// Movement since the axis was last judged (points).
    window:     (f32, f32),
    /// Exact diameter and hardness (shown rounded).
    size:       f32,
    hardness:   f32,
    max_size:   f32,
    /// False: the brush has no hardness to change (size only).
    hardness_ok: bool,
}

impl SizeDrag {
    /// `at`: the press in window pixels and in points (the movement is
    /// measured in points: the same feel at any display scaling).
    pub fn new(anchor: (f32, f32), at: (f32, f32), size: f32, hardness: f32, max_size: f32, hardness_ok: bool) -> Self {
        Self {
            anchor, axis: None, last: at, window: (0.0, 0.0),
            size: size.clamp(MIN_SIZE, max_size), hardness: hardness.clamp(0.0, 1.0), max_size, hardness_ok,
        }
    }

    /// Diameter, whole pixels.
    pub fn size(&self) -> f32 { self.size.round().clamp(MIN_SIZE, self.max_size) }

    /// Hardness, whole percents (0..1).
    pub fn hardness(&self) -> f32 { (self.hardness * 100.0).round() / 100.0 }

    /// The pointer moved to `to` (points).
    pub fn moved(&mut self, to: (f32, f32)) {
        let d = (to.0 - self.last.0, to.1 - self.last.1);
        self.last = to;
        self.window.0 += d.0;
        self.window.1 += d.1;
        let travel = self.window.0.hypot(self.window.1);
        match self.axis {
            None => {
                if travel < START { return; }
                // The first movement counts in full along the axis picked.
                let axis = self.dominant();
                self.axis = Some(axis);
                let w = std::mem::take(&mut self.window);
                self.change(axis, w);
            }
            Some(axis) => {
                self.change(axis, d);
                if travel < WINDOW { return; }
                let now = self.dominant();
                let w = std::mem::take(&mut self.window);
                if now != axis {
                    // The turn's points went the new way: they count.
                    self.axis = Some(now);
                    self.change(now, w);
                }
            }
        }
    }

    /// The axis the movement since the last judgment goes along most
    /// (exactly 45°: size).
    fn dominant(&self) -> Axis {
        if !self.hardness_ok || self.window.0.abs() >= self.window.1.abs() { Axis::Size } else { Axis::Hardness }
    }

    fn change(&mut self, axis: Axis, d: (f32, f32)) {
        match axis {
            // Right grows the brush: a pixel per point up to 100 px (exact
            // small sizes), then 1 % per point (big brushes in reach).
            Axis::Size => self.size = (self.size + d.0 * (self.size / 100.0).max(1.0)).clamp(MIN_SIZE, self.max_size),
            // Up softens the brush, down hardens it.
            Axis::Hardness => self.hardness = (self.hardness + d.1 * HARDNESS_PER_POINT).clamp(0.0, 1.0),
        }
    }

    /// What the readout next to the brush says.
    pub fn label(&self) -> String {
        match self.axis {
            Some(Axis::Hardness) => format!("Hardness {:.0} %", self.hardness() * 100.0),
            _ => format!("{:.0} px", self.size()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(s: &mut SizeDrag, from: (f32, f32), path: &[(f32, f32)]) {
        let mut p = from;
        for &(dx, dy) in path {
            // In 1-point steps, as a real pointer.
            let n = dx.abs().max(dy.abs()).ceil().max(1.0) as usize;
            for _ in 0..n {
                p = (p.0 + dx / n as f32, p.1 + dy / n as f32);
                s.moved(p);
            }
        }
    }

    fn new(size: f32, hardness: f32) -> SizeDrag { SizeDrag::new((0.0, 0.0), (0.0, 0.0), size, hardness, 1000.0, true) }

    #[test]
    fn a_slightly_slanted_horizontal_drag_changes_only_the_size() {
        let mut s = new(20.0, 0.8);
        // 40 points right, 15 down (about 20°): Photoshop would also have
        // changed the hardness.
        drag(&mut s, (0.0, 0.0), &[(40.0, 15.0)]);
        assert_eq!(s.size(), 60.0);
        assert_eq!(s.hardness(), 0.8);
        assert_eq!(s.axis, Some(Axis::Size));
    }

    #[test]
    fn a_slanted_vertical_drag_changes_only_the_hardness() {
        let mut s = new(50.0, 1.0);
        drag(&mut s, (0.0, 0.0), &[(-12.0, -40.0)]);
        assert_eq!(s.size(), 50.0);
        assert_eq!(s.hardness(), 0.8);
    }

    #[test]
    fn right_then_up_in_one_drag_grows_then_softens() {
        let mut s = new(10.0, 1.0);
        drag(&mut s, (0.0, 0.0), &[(50.0, 0.0), (0.0, -60.0)]);
        // The size: 50 points right (the turn's first points go up: no
        // size there).
        assert_eq!(s.size(), 60.0);
        // Nearly all of the 60 points up soften.
        let h = s.hardness();
        assert!((0.69..=0.75).contains(&h), "hardness {h}");
        assert_eq!(s.axis, Some(Axis::Hardness));
    }

    #[test]
    fn a_small_wobble_does_not_switch() {
        let mut s = new(30.0, 0.5);
        drag(&mut s, (0.0, 0.0), &[(20.0, 0.0), (6.0, 4.0), (6.0, -4.0), (20.0, 0.0)]);
        assert_eq!(s.hardness(), 0.5);
        assert_eq!(s.axis, Some(Axis::Size));
    }

    #[test]
    fn values_stay_in_range_and_big_brushes_change_faster() {
        let mut s = new(5.0, 0.5);
        drag(&mut s, (0.0, 0.0), &[(-100.0, 0.0)]);
        assert_eq!(s.size(), 1.0);
        let mut s = new(500.0, 0.5);
        drag(&mut s, (0.0, 0.0), &[(10.0, 0.0)]);
        assert!(s.size() > 545.0, "1 % per point at 500 px: {}", s.size());
        drag(&mut s, (10.0, 0.0), &[(2000.0, 0.0)]);
        assert_eq!(s.size(), 1000.0);
        let mut s = new(5.0, 0.5);
        drag(&mut s, (0.0, 0.0), &[(0.0, 500.0)]);
        assert_eq!(s.hardness(), 1.0);
    }

    #[test]
    fn without_hardness_only_the_size_changes() {
        let mut s = SizeDrag::new((0.0, 0.0), (0.0, 0.0), 20.0, 1.0, 1000.0, false);
        drag(&mut s, (0.0, 0.0), &[(0.0, -80.0), (10.0, 0.0)]);
        assert_eq!(s.hardness(), 1.0);
        assert_eq!(s.size(), 30.0);
    }
}

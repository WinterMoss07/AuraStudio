//! Onion skin, OpenToonz's way: marks on the timeline say which drawings
//! of the active animation folder show, faded and tinted, behind its
//! current drawing. Mobile marks count drawings from the current one
//! (−1: the drawing before, whatever its hold), fixed marks are frames
//! (their drawing stays shown wherever the current frame is). Each mark
//! has its opacity, or the automatic fade by distance (OpenToonz's paper
//! thickness). Drawings before are tinted with one color, after with the
//! other: OpenToonz's correction, `rgb + (1 − opacity) × tint`.

use std::collections::BTreeMap;

use crate::{LayerStack, Node};

/// The marks of a document (saved with it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Onion {
    /// Shown (the marks stay when off).
    pub enabled: bool,
    /// Drawings from the current one (never 0) → opacity, None: automatic.
    pub mobile:  BTreeMap<i32, Option<f32>>,
    /// Frames → opacity, None: automatic.
    pub fixed:   BTreeMap<u32, Option<f32>>,
}

/// How the ghosts look (Preferences > Animation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionLook {
    pub before:    [u8; 3],
    pub after:     [u8; 3],
    /// 0..100: how fast the drawings fade with distance (OpenToonz's
    /// paper thickness, 50 by default).
    pub thickness: u8,
}

impl Default for OnionLook {
    fn default() -> Self { Self { before: [255, 0, 0], after: [0, 255, 0], thickness: 50 } }
}

impl OnionLook {
    /// Automatic opacity of a drawing `d` drawings away (OpenToonz's
    /// getOnionSkinFade: 0.35 + d × step, at most 0.95 faded).
    pub fn fade(&self, d: u32) -> f32 {
        // OpenToonz's table: 0 → 0, 10 → 0.05, 50 → 0.12, 90 → 0.3, 100 → 0.6.
        let t = self.thickness.min(100) as f32;
        let pts = [(0.0, 0.0), (10.0, 0.05), (50.0, 0.12), (90.0, 0.3), (100.0, 0.6)];
        let step = pts.windows(2).find(|w| t <= w[1].0).map_or(0.6, |w| w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / (w[1].0 - w[0].0));
        1.0 - (0.35 + d as f32 * step).clamp(0.35, 0.95)
    }
}

/// One drawing shown as a ghost: its frame folder, tint and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ghost {
    pub frame:   u64,
    pub tint:    [f32; 3],
    pub opacity: f32,
}

/// The onion state of a stack: the marks, their look, and the ghosts they
/// give now (folder, ghosts far to near).
#[derive(Clone, Debug, Default)]
pub(crate) struct OnionState {
    pub marks:  Onion,
    pub look:   OnionLook,
    /// Off for a while (playback), marks kept.
    pub paused: bool,
    pub ghosts: Vec<(u64, Vec<Ghost>)>,
}

/// The ghosts of folder `id`, if any.
pub(crate) fn ghosts_of(g: &[(u64, Vec<Ghost>)], id: u64) -> Option<&[Ghost]> {
    g.iter().find(|x| x.0 == id).map(|x| &x.1[..]).filter(|v| !v.is_empty())
}

/// `cs` tinted for ghost `g` (straight color, 0..1).
#[inline]
pub(crate) fn tinted(cs: [f32; 3], g: &Ghost) -> [f32; 3] {
    let k = 1.0 - g.opacity;
    [(cs[0] + k * g.tint[0]).min(1.0), (cs[1] + k * g.tint[1]).min(1.0), (cs[2] + k * g.tint[2]).min(1.0)]
}

impl LayerStack {
    pub fn onion(&self) -> &Onion { &self.onion.marks }

    pub fn set_onion(&mut self, marks: Onion) {
        self.onion.marks = marks;
        self.update_onion();
    }

    pub fn onion_look(&self) -> OnionLook { self.onion.look }

    pub fn set_onion_look(&mut self, look: OnionLook) {
        self.onion.look = look;
        self.update_onion();
    }

    /// Ghosts off (playback) or back on.
    pub fn pause_onion(&mut self, paused: bool) {
        if self.onion.paused == paused { return; }
        self.onion.paused = paused;
        self.update_onion();
    }

    /// The ghosts shown now (for the merge; empty when off).
    pub fn ghosts(&self) -> &[(u64, Vec<Ghost>)] { &self.onion.ghosts }

    /// The drawings the marks show now, for the active animation folder.
    pub(crate) fn update_onion(&mut self) {
        self.onion.ghosts.clear();
        let o = &self.onion;
        if !o.marks.enabled || o.paused { return; }
        let Some((folder, _)) = self.anim_of(self.active) else { return };
        if !self.node(folder).is_some_and(Node::visible) { return; }
        let frames = self.frames(folder);
        let Some(cur) = frames.iter().rposition(|f| f.0 <= self.time) else {
            // Before the first drawing: nothing current, nothing to compare.
            return;
        };
        let look = o.look;
        let color = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);
        let mut shown: Vec<(u32, Ghost)> = Vec::new();
        let add = |i: usize, opacity: Option<f32>, shown: &mut Vec<(u32, Ghost)>| {
            if i == cur || shown.iter().any(|g| g.1.frame == frames[i].1) { return; }
            let d = i.abs_diff(cur) as u32;
            let tint = color(if i < cur { look.before } else { look.after });
            let opacity = opacity.map_or_else(|| look.fade(d), |v| v.clamp(0.0, 1.0));
            if opacity > 0.0 { shown.push((d, Ghost { frame: frames[i].1, tint, opacity })); }
        };
        for (&off, &op) in &o.marks.mobile {
            let i = cur as i64 + off as i64;
            if off != 0 && (0..frames.len() as i64).contains(&i) { add(i as usize, op, &mut shown); }
        }
        for (&f, &op) in &o.marks.fixed {
            if let Some(i) = frames.iter().rposition(|x| x.0 <= f) { add(i, op, &mut shown); }
        }
        // Far ones first: nearer drawings over them.
        shown.sort_by_key(|g| std::cmp::Reverse(g.0));
        self.onion.ghosts = vec![(folder, shown.into_iter().map(|g| g.1).collect())];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pick;

    #[test]
    fn the_fade_is_opentoonz_s() {
        let l = OnionLook::default();
        assert!((l.fade(1) - 0.53).abs() < 1e-4 && (l.fade(2) - 0.41).abs() < 1e-4);
        assert!((l.fade(50) - 0.05).abs() < 1e-4, "never quite gone");
        assert!((OnionLook { thickness: 0, ..l }.fade(5) - 0.65).abs() < 1e-4, "no thickness: all alike");
    }

    /// A folder with drawings at 1, 4 and 6 (black squares at x = 10, 30,
    /// 50), the current frame 5 (the drawing of 4, held).
    fn scene() -> (LayerStack, u64) {
        let mut s = LayerStack::new(80, 30);
        let folder = s.add_animation_folder("A");
        for (n, x) in [(1u32, 10u32), (4, 30), (6, 50)] {
            let cel = s.make_frame(folder, n).unwrap();
            assert!(s.set_layer_rect(cel, (x, 10, x + 10, 20), &[0, 0, 0, 255].repeat(100)));
        }
        s.set_time(5);
        let f4 = s.frame_at(folder, 4).unwrap();
        s.set_active(f4);
        (s, folder)
    }

    fn px(s: &LayerStack, x: u32) -> [u8; 4] { s.composite_rect((x, 15, x + 1, 16)).try_into().unwrap() }

    #[test]
    fn marks_count_drawings_and_tint_them() {
        let (mut s, _) = scene();
        assert_eq!(px(&s, 15), [255; 4], "no onion: only the current drawing");
        s.set_onion(Onion { enabled: true, mobile: [(-1, None), (1, None)].into(), fixed: Default::default() });
        // The drawing before (frame 1), red: black + (1 − 0.53) × red, at
        // 53 % over white.
        let before = px(&s, 15);
        assert_eq!(before, [183, 120, 120, 255]);
        // After (frame 6): green.
        let after = px(&s, 55);
        assert_eq!(after, [120, 183, 120, 255]);
        // The current drawing on top, untouched.
        assert_eq!(px(&s, 35), [0, 0, 0, 255]);
        // Not in what is exported or picked.
        let export = s.composite_pick((15, 15, 16, 16), Pick::EXPORT, false);
        assert_eq!(&export[..], &[255, 255, 255, 255]);
    }

    #[test]
    fn a_fixed_mark_stays_and_a_mark_s_opacity_wins() {
        let (mut s, folder) = scene();
        s.set_onion(Onion { enabled: true, mobile: Default::default(), fixed: [(2, Some(1.0))].into() });
        // Frame 2 holds the drawing of 1: shown at full opacity, no tint.
        assert_eq!(px(&s, 15), [0, 0, 0, 255]);
        s.set_time(7);
        let f6 = s.frame_at(folder, 6).unwrap();
        s.set_active(f6);
        assert_eq!(px(&s, 15), [0, 0, 0, 255], "still there from another frame");
        // Off: gone, marks kept.
        let mut o = s.onion().clone();
        o.enabled = false;
        s.set_onion(o);
        assert_eq!(px(&s, 15), [255; 4]);
        assert_eq!(s.onion().fixed.len(), 1);
    }

    #[test]
    fn the_general_merge_shows_the_same_ghosts() {
        let (mut s, _) = scene();
        s.set_onion(Onion { enabled: true, mobile: [(-1, None), (1, Some(0.7))].into(), fixed: Default::default() });
        let plain = s.composite_rect((0, 0, 80, 30));
        // An empty clipped layer on top: the general merge, same pixels.
        let (active, paper) = (s.active_id(), s.layer_ids()[0]);
        s.set_active(paper);
        let top = s.add_layer("clipped");
        if let Some(n) = s.node_mut(top) { n.props_mut().clip = true; }
        s.set_active(active);
        assert!(s.needs_general_merge());
        let general = s.composite_rect((0, 0, 80, 30));
        let worst = plain.iter().zip(&general).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "off by {worst}");
    }

    #[test]
    fn the_stroke_cache_shows_the_ghosts_too() {
        let (mut s, folder) = scene();
        // On drawing 4 itself (not a hold): its cel is painted.
        s.set_time(4);
        s.set_onion(Onion { enabled: true, mobile: [(-1, None), (-2, Some(0.3)), (1, None)].into(), fixed: Default::default() });
        let Some(crate::Target::Layer(cel)) = s.paint_target() else { panic!("a cel to paint") };
        assert!(s.begin_stroke(cel));
        let full = s.composite_rect((0, 0, 80, 30));
        let cached = s.composite_rect_stroke((0, 0, 80, 30));
        assert_eq!(full, cached);
        let _ = folder;
    }
}

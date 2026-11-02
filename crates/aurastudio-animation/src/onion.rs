//! Onion skin, OpenToonz's way: marks on the timeline say which sheets
//! of the active animation folder show, faded and tinted, behind its
//! current sheet. Mobile marks count from the current one, in sheets
//! (−1: the sheet before, whatever its hold; Krita, Clip Studio) or in
//! frames (−1: the frame before, OpenToonz; a sheet already shown, the
//! current one included, is not shown twice). Fixed marks are frames
//! (their sheet stays shown wherever the current frame is). Each mark has
//! its opacity, or the automatic fade by distance (OpenToonz's paper
//! thickness). Sheets before are tinted with one color, after with the
//! other, as OpenToonz does for raster images (quickPut's colorScale):
//! `1 − (1 − rgb)(1 − tint)` per channel, so black ink takes the tint
//! exactly and white stays white, then the opacity as the alpha. (Its
//! vector images add `(1 − opacity) × tint` instead: washed out.)

use std::collections::BTreeMap;

use aurastudio_layers::{Ghost, LayerStack, Node};

use crate::sheets::Sheets;

/// The marks of a document (saved with it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Onion {
    /// Shown (the marks stay when off).
    pub enabled: bool,
    /// Offsets from the current sheet or frame (never 0) → opacity,
    /// None: automatic.
    pub mobile:  BTreeMap<i32, Option<f32>>,
    /// Frames → opacity, None: automatic.
    pub fixed:   BTreeMap<u32, Option<f32>>,
}

/// How the ghosts look (Preferences > Animation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionLook {
    pub before:    [u8; 3],
    pub after:     [u8; 3],
    /// 0..100: how fast the sheets fade with distance (OpenToonz's
    /// paper thickness, 50 by default).
    pub thickness: u8,
    /// Mobile marks count frames (OpenToonz), not sheets.
    pub by_frames: bool,
}

impl Default for OnionLook {
    fn default() -> Self { Self { before: [255, 0, 0], after: [0, 255, 0], thickness: 50, by_frames: false } }
}

impl OnionLook {
    /// Automatic opacity of a sheet `d` sheets away (OpenToonz's
    /// getOnionSkinFade: 0.35 + d × step, at most 0.95 faded).
    pub fn fade(&self, d: u32) -> f32 {
        // OpenToonz's table: 0 → 0, 10 → 0.05, 50 → 0.12, 90 → 0.3, 100 → 0.6.
        let t = self.thickness.min(100) as f32;
        let pts = [(0.0, 0.0), (10.0, 0.05), (50.0, 0.12), (90.0, 0.3), (100.0, 0.6)];
        let step = pts.windows(2).find(|w| t <= w[1].0).map_or(0.6, |w| w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / (w[1].0 - w[0].0));
        1.0 - (0.35 + d as f32 * step).clamp(0.35, 0.95)
    }
}


/// The sheets the marks show at `time`, for the active animation folder:
/// folder → ghosts, far to near (nearer sheets over them).
pub(crate) fn ghosts(stack: &LayerStack, marks: &Onion, look: OnionLook, time: u32) -> Vec<(u64, Vec<Ghost>)> {
    if !marks.enabled { return Vec::new(); }
    let Some((folder, _)) = stack.anim_of(stack.active_id()) else { return Vec::new() };
    if !stack.node(folder).is_some_and(Node::visible) { return Vec::new(); }
    let sheets = stack.sheets(folder);
    // Before the first sheet: nothing current, nothing to compare.
    let Some(cur) = sheets.iter().rposition(|f| f.0 <= time) else { return Vec::new() };
    let color = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);
    let mut shown: Vec<(u32, Ghost)> = Vec::new();
    let add = |i: usize, d: u32, opacity: Option<f32>, shown: &mut Vec<(u32, Ghost)>| {
        if i == cur || shown.iter().any(|g| g.1.sheet == sheets[i].1) { return; }
        let tint = color(if i < cur { look.before } else { look.after });
        let opacity = opacity.map_or_else(|| look.fade(d), |v| v.clamp(0.0, 1.0));
        if opacity > 0.0 { shown.push((d, Ghost { sheet: sheets[i].1, tint, opacity })); }
    };
    if look.by_frames {
        // The sheet each frame shows; the distance counts the sheets shown
        // on that side, nearest first (OpenToonz).
        let (mut back, mut front) = (0, 0);
        let mut ms: Vec<(i32, Option<f32>)> = marks.mobile.iter().map(|(k, v)| (*k, *v)).collect();
        ms.sort_by_key(|m| m.0.unsigned_abs());
        for (off, op) in ms {
            let t = time as i64 + off as i64;
            if off == 0 || t < 1 { continue; }
            let Some(i) = sheets.iter().rposition(|x| x.0 as i64 <= t) else { continue };
            if i == cur || shown.iter().any(|g| g.1.sheet == sheets[i].1) { continue; }
            let d = if off < 0 { back += 1; back } else { front += 1; front };
            add(i, d, op, &mut shown);
        }
    } else {
        for (&off, &op) in &marks.mobile {
            let i = cur as i64 + off as i64;
            if off != 0 && (0..sheets.len() as i64).contains(&i) { add(i as usize, off.unsigned_abs(), op, &mut shown); }
        }
    }
    for (&f, &op) in &marks.fixed {
        if let Some(i) = sheets.iter().rposition(|x| x.0 <= f) { add(i, i.abs_diff(cur) as u32, op, &mut shown); }
    }
    shown.sort_by_key(|g| std::cmp::Reverse(g.0));
    vec![(folder, shown.into_iter().map(|g| g.1).collect())]
}

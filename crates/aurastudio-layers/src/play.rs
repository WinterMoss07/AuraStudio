//! What animation playback needs to show any frame without merging the
//! stack again: the stack cut into parts in order, each animation folder a
//! part of its own, the layers between them merged once into static parts;
//! every drawing (frame folder) merged once on its own. All of it kept as
//! 16 px tiles where something shows: one alpha byte per pixel when a tile
//! has a single color (line art), RGBA otherwise. The GPU draws the parts
//! of a frame over white, as the screen merge does.
//!
//! Only stacks where that equals the merge (in exact arithmetic): no
//! visible filter layer or clipping, Normal blending throughout (an
//! isolated folder that holds no animation folder may blend inside), and
//! no isolated folder below 100 % around an animation folder.

use aurastudio_canvas::Rect;

use crate::anim::FolderKind;
use crate::compose::Pick;
use crate::{BlendMode, FolderBlend, LayerStack, Node};

/// Side of the playback tiles, pixels.
pub const PLAY_TILE: u32 = 16;
const PT: usize = PLAY_TILE as usize;

/// One tile of a part (tile coordinates, in PLAY_TILE units).
pub enum PlayTile {
    /// One color, its coverage per pixel.
    Alpha { x: u16, y: u16, rgb: [u8; 3], a: Box<[u8; PT * PT]> },
    /// Straight RGBA.
    Rgba { x: u16, y: u16, px: Box<[u8; PT * PT * 4]> },
}

impl PlayTile {
    pub fn pos(&self) -> (u16, u16) {
        match self { PlayTile::Alpha { x, y, .. } | PlayTile::Rgba { x, y, .. } => (*x, *y) }
    }
}

/// The tiles of a part or a drawing.
#[derive(Default)]
pub struct PlayTiles {
    pub tiles: Vec<PlayTile>,
}

impl std::fmt::Debug for PlayTiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PlayTiles({} tiles, {} bytes)", self.tiles.len(), self.bytes())
    }
}

impl PlayTiles {
    /// Bytes of pixels held.
    pub fn bytes(&self) -> usize {
        self.tiles.iter().map(|t| match t { PlayTile::Alpha { .. } => PT * PT, PlayTile::Rgba { .. } => PT * PT * 4 }).sum()
    }
}

/// A part of the stack, bottom to top.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayPart {
    /// The layers between two animation folders (index into the static
    /// parts).
    Static(usize),
    /// An animation folder: its frames (number, frame folder id).
    Anim { folder: u64, frames: Vec<(u32, u64)> },
}

/// How a stack is played from the cache.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayPlan {
    pub width:  u32,
    pub height: u32,
    pub parts:  Vec<PlayPart>,
    /// The leaf nodes (layers, filter layers, folders without animation)
    /// of each static part.
    statics: Vec<Vec<u64>>,
    /// Every leaf node and animation folder, for the merges of one part.
    units:   Vec<u64>,
}

impl PlayPlan {
    /// The drawing each animation part shows at frame `t` (None: none yet).
    pub fn shown_at(&self, t: u32) -> Vec<Option<u64>> {
        self.parts.iter().filter_map(|p| match p {
            PlayPart::Anim { frames, .. } => Some(frames.iter().rev().find(|f| f.0 <= t).map(|f| f.1)),
            PlayPart::Static(_) => None,
        }).collect()
    }

    pub fn statics(&self) -> usize { self.statics.len() }

    /// Every drawing of the plan.
    pub fn drawings(&self) -> Vec<u64> {
        self.parts.iter().flat_map(|p| match p {
            PlayPart::Anim { frames, .. } => frames.iter().map(|f| f.1).collect(),
            PlayPart::Static(_) => Vec::new(),
        }).collect()
    }
}

/// Holds an animation folder somewhere inside.
fn holds_animation(n: &Node) -> bool {
    match n {
        Node::Folder(f) => f.anim().is_some() || f.children.iter().any(holds_animation),
        _ => false,
    }
}

/// Every visible node blends Normal (or a folder passes through) down
/// from here; an isolated Normal folder may hold anything when it holds
/// no animation folder.
fn plays(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| {
        if !n.visible() { return true; }
        match n {
            Node::Layer(l) => l.blend == BlendMode::Normal,
            Node::Filter(_) => false,
            Node::Folder(f) => match f.blend {
                FolderBlend::Isolated(m) if m != BlendMode::Normal => false,
                FolderBlend::Isolated(_) if !holds_animation(n) => true,
                FolderBlend::Isolated(_) if f.opacity < 1.0 && f.anim().is_none() => false,
                _ => plays(&f.children),
            },
        }
    })
}

/// The leaf units bottom to top: (id, is an animation folder).
fn units(nodes: &[Node], out: &mut Vec<(u64, bool)>) {
    for n in nodes {
        match n {
            Node::Folder(f) if f.anim().is_some() => out.push((f.id, true)),
            Node::Folder(f) if holds_animation(n) => units(&f.children, out),
            _ => out.push((n.id(), false)),
        }
    }
}

impl LayerStack {
    /// How to play this stack from the cache, or None when the cache would
    /// not show what the merge shows.
    pub fn play_plan(&self) -> Option<PlayPlan> {
        if self.needs_general_merge() || !plays(&self.root) { return None; }
        let mut all = Vec::new();
        units(&self.root, &mut all);
        let mut parts = Vec::new();
        let mut statics: Vec<Vec<u64>> = Vec::new();
        let mut run: Vec<u64> = Vec::new();
        for &(id, anim) in &all {
            if !anim { run.push(id); continue; }
            if !self.node(id).is_some_and(Node::visible) { continue; }
            if !run.is_empty() { parts.push(PlayPart::Static(statics.len())); statics.push(std::mem::take(&mut run)); }
            parts.push(PlayPart::Anim { folder: id, frames: self.frames(id) });
        }
        if !run.is_empty() { parts.push(PlayPart::Static(statics.len())); statics.push(run); }
        Some(PlayPlan { width: self.width, height: self.height, parts, statics, units: all.into_iter().map(|u| u.0).collect() })
    }

    /// Static part `k` on its own. At the bottom, its opaque white tiles
    /// are left out (frames are drawn over white).
    pub fn play_static(&mut self, plan: &PlayPlan, k: usize) -> PlayTiles {
        let keep = &plan.statics[k];
        let full = (0, 0, self.width, self.height);
        let mut tiles = self.merge_only(plan, |id| keep.contains(&id), None, full);
        if plan.parts.first() == Some(&PlayPart::Static(k)) {
            let (w, h) = (self.width as usize, self.height as usize);
            tiles.tiles.retain(|t| {
                let PlayTile::Alpha { x, y, rgb: [255, 255, 255], a } = t else { return true };
                // (Only the pixels on the canvas count, at its edges.)
                let (tw, th) = (PT.min(w - *x as usize * PT), PT.min(h - *y as usize * PT));
                !(0..th).all(|yy| a[yy * PT..yy * PT + tw].iter().all(|&v| v == 255))
            });
        }
        tiles
    }

    /// Drawing `frame` (a frame folder of the plan) on its own, with its
    /// folder's opacity.
    pub fn play_drawing(&mut self, plan: &PlayPlan, frame: u64) -> PlayTiles {
        let Some((folder, _)) = self.anim_of(frame) else { return PlayTiles::default() };
        // Where its visible cels show.
        let mut area: Option<Rect> = None;
        if let Some(Node::Folder(f)) = self.node(frame) {
            for c in f.children.iter().filter(|c| c.visible()) {
                if let Node::Layer(l) = c {
                    if let Some(b) = l.tiles.visible_bounds() { area = Some(area.map_or(b, |a| aurastudio_canvas::union(a, b))); }
                }
            }
        }
        let Some(area) = area else { return PlayTiles::default() };
        self.merge_only(plan, |id| id == folder, Some((folder, frame)), area)
    }

    /// Frame `t` drawn from the parts over white, as the GPU does (f64,
    /// then rounded): RGB rows. For the tests and the fuzzing.
    #[doc(hidden)]
    pub fn play_cpu(&mut self, plan: &PlayPlan, t: u32) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut acc = vec![[1.0f64; 3]; w * h];
        let shown = plan.shown_at(t);
        let mut k = 0;
        for p in plan.parts.clone() {
            let tiles = match p {
                PlayPart::Static(i) => self.play_static(plan, i),
                PlayPart::Anim { .. } => { k += 1; match shown[k - 1] { Some(d) => self.play_drawing(plan, d), None => continue } }
            };
            for tile in &tiles.tiles {
                let (tx, ty) = tile.pos();
                for y in 0..PT {
                    for x in 0..PT {
                        let (cx, cy) = (tx as usize * PT + x, ty as usize * PT + y);
                        if cx >= w || cy >= h { continue; }
                        let (c, a) = match tile {
                            PlayTile::Alpha { rgb, a, .. } => (*rgb, a[y * PT + x]),
                            PlayTile::Rgba { px, .. } => { let i = (y * PT + x) * 4; ([px[i], px[i + 1], px[i + 2]], px[i + 3]) }
                        };
                        let a = a as f64 / 255.0;
                        let d = &mut acc[cy * w + cx];
                        for i in 0..3 { d[i] = c[i] as f64 / 255.0 * a + d[i] * (1.0 - a); }
                    }
                }
            }
        }
        acc.iter().flat_map(|p| [0, 1, 2].map(|i| (p[i] * 255.0).round() as u8)).collect()
    }

    /// The merge of `rect` (grown to whole tiles) with only the units
    /// `keep` takes visible (and, in `show`'s folder, only its frame),
    /// over transparency, as tiles. Visibility is put back after.
    fn merge_only(&mut self, plan: &PlayPlan, keep: impl Fn(u64) -> bool, show: Option<(u64, u64)>, rect: Rect) -> PlayTiles {
        let mut saved: Vec<(u64, bool)> = Vec::new();
        for &id in &plan.units {
            if let Some(n) = self.node_mut(id) {
                saved.push((id, n.visible()));
                if !keep(id) { *n.visible_mut() = false; }
            }
        }
        if let Some((folder, frame)) = show {
            if let Some(Node::Folder(f)) = self.node_mut(folder) {
                for c in &mut f.children {
                    if matches!(c, Node::Folder(cf) if matches!(cf.kind, FolderKind::Frame(_))) {
                        saved.push((c.id(), c.visible()));
                        *c.visible_mut() = c.id() == frame;
                    }
                }
            }
        }
        let t = PLAY_TILE;
        let r = (rect.0 / t * t, rect.1 / t * t, rect.2.div_ceil(t).saturating_mul(t).min(self.width), rect.3.div_ceil(t).saturating_mul(t).min(self.height));
        let px = if r.0 < r.2 && r.1 < r.3 { self.composite_pick_par(r, Pick::ALL, true) } else { Vec::new() };
        for (id, v) in saved.into_iter().rev() {
            if let Some(n) = self.node_mut(id) { *n.visible_mut() = v; }
        }
        to_tiles(&px, r)
    }
}

/// Straight RGBA rows of `r` (corner on the tile grid) as tiles: the
/// empty ones left out, single-color ones as alpha.
fn to_tiles(px: &[u8], r: Rect) -> PlayTiles {
    let mut out = PlayTiles::default();
    let w = (r.2 - r.0) as usize;
    if w == 0 { return out; }
    let h = (r.3 - r.1) as usize;
    for ty in (0..h).step_by(PT) {
        for tx in (0..w).step_by(PT) {
            let (tw, th) = (PT.min(w - tx), PT.min(h - ty));
            let mut rgba = [0u8; PT * PT * 4];
            let mut color: Option<[u8; 3]> = None;
            let (mut shows, mut single) = (false, true);
            for y in 0..th {
                let row = &px[((ty + y) * w + tx) * 4..((ty + y) * w + tx + tw) * 4];
                rgba[y * PT * 4..y * PT * 4 + tw * 4].copy_from_slice(row);
                for p in row.chunks_exact(4).filter(|p| p[3] > 0) {
                    shows = true;
                    let c = [p[0], p[1], p[2]];
                    if *color.get_or_insert(c) != c { single = false; }
                }
            }
            if !shows { continue; }
            let (x, y) = (((r.0 as usize + tx) / PT) as u16, ((r.1 as usize + ty) / PT) as u16);
            // (Transparent pixels of a partial tile stay at alpha 0.)
            for yy in 0..PT {
                for xx in 0..PT {
                    if yy >= th || xx >= tw { rgba[(yy * PT + xx) * 4 + 3] = 0; }
                }
            }
            out.tiles.push(match (single, color) {
                (true, Some(rgb)) => {
                    let mut a = Box::new([0u8; PT * PT]);
                    for (d, p) in a.iter_mut().zip(rgba.chunks_exact(4)) { *d = p[3]; }
                    PlayTile::Alpha { x, y, rgb, a }
                }
                _ => PlayTile::Rgba { x, y, px: Box::new(rgba) },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack with paper, an animation folder of two drawings and a
    /// layer over it.
    fn scene() -> (LayerStack, u64, [u64; 2]) {
        let mut s = LayerStack::new(70, 50);
        let folder = s.add_animation_folder("A");
        let c1 = s.make_frame(folder, 1).unwrap();
        let c2 = s.make_frame(folder, 3).unwrap();
        let frame = |s: &LayerStack, cel: u64| s.anim_of(cel).unwrap().1.unwrap();
        let (f1, f2) = (frame(&s, c1), frame(&s, c2));
        for (cel, x, c) in [(c1, 5u32, [200u8, 0, 0]), (c2, 30, [0, 0, 200])] {
            let px: Vec<u8> = (0..20 * 20).flat_map(|i| if i % 3 == 0 { [c[0], c[1], c[2], 255] } else { [0, 0, 0, 0] }).collect();
            s.set_layer_rect(cel, (x, 10, x + 20, 30), &px);
        }
        let top = s.add_layer("over");
        let px: Vec<u8> = (0..10 * 10).flat_map(|i| [i as u8, 90, 40, (i * 2) as u8]).collect();
        s.set_layer_rect(top, (40, 30, 50, 40), &px);
        (s, folder, [f1, f2])
    }

    #[test]
    fn the_parts_over_white_give_the_merge_at_every_frame() {
        let (mut s, folder, _) = scene();
        let plan = s.play_plan().expect("a plain scene plays");
        assert_eq!(plan.parts.len(), 3, "paper, the folder, the layer over it");
        assert!(matches!(&plan.parts[1], PlayPart::Anim { folder: f, frames } if *f == folder && frames.len() == 2));
        for t in 1..=4 {
            s.set_time(t);
            let merged = s.composite_pick((0, 0, 70, 50), Pick::ALL, false);
            let want: Vec<u8> = merged.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let got = s.play_cpu(&plan, t);
            let worst = want.iter().zip(&got).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
            assert!(worst <= 1, "frame {t}: off by {worst}");
        }
    }

    #[test]
    fn line_art_is_kept_as_alpha_and_the_stack_is_left_as_it_was() {
        let (mut s, _, [f1, f2]) = scene();
        let plan = s.play_plan().unwrap();
        s.set_time(1);
        let before: Vec<bool> = [f1, f2].iter().map(|id| s.node(*id).unwrap().visible()).collect();
        let d = s.play_drawing(&plan, f2);
        assert!(!d.tiles.is_empty() && d.tiles.iter().all(|t| matches!(t, PlayTile::Alpha { rgb: [0, 0, 200], .. })), "one color: alpha tiles");
        // 20 × 20 px at x 30..50, y 10..30: 2 × 3 tiles of 16.
        assert_eq!(d.tiles.len(), 6);
        assert_eq!(d.bytes(), 6 * 256);
        // The white paper under everything: nothing to keep.
        assert!(s.play_static(&plan, 0).tiles.is_empty());
        let after: Vec<bool> = [f1, f2].iter().map(|id| s.node(*id).unwrap().visible()).collect();
        assert_eq!(before, after, "visibility put back");
    }

    #[test]
    fn a_multiply_layer_or_a_filter_layer_falls_back_to_the_merge() {
        let (mut s, _, _) = scene();
        let id = s.layer_ids()[0];
        if let Some(Node::Layer(l)) = s.node_mut(id) { l.blend = BlendMode::Multiply; }
        assert!(s.play_plan().is_none());
    }
}

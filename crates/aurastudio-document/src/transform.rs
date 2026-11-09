//! A transform under way: the layers it moves as they were, what is left
//! of them once what moves is lifted out, and what moves (the selected
//! pixels, else all of them), shown warped until applied (one undo step,
//! the selection moved along) or cancelled. Layers are whole surfaces
//! shared tile by tile: putting one back costs nothing.

use aurastudio_core::{blend, Rect, Rgba8};
use aurastudio_layers::{LayerStack, Node};
use aurastudio_raster::Image;
use aurastudio_selection::{Mode, Selection};
use aurastudio_transform::{render, Interp, Source, Warp};

use crate::document::Before;
use crate::history::Step;
use crate::region::{Done, Region};
use crate::Document;

pub(crate) struct Session {
    /// What moves, canvas pixels.
    pub src:   Rect,
    targets:   Vec<Target>,
    /// The selection when it began, and as a source to move it along.
    selection: Option<(Selection, Source)>,
    /// What the last preview drew.
    shown:     Rect,
}

struct Target {
    id:     u64,
    before: Image,
    /// The layer with what moves taken out.
    base:   Image,
    source: Source,
}

/// What a transform's live preview needs (the GPU puts it together while a
/// handle is dragged): the merged frame under the moved layers and over
/// them, and what moves.
pub struct Lift {
    pub src:    Rect,
    /// The part of the frame `below` and `above` hold (around the source:
    /// a transform moved further asks for a bigger one).
    pub area:   Rect,
    /// `area` merged up to the top moved layer (what is left of them in),
    /// straight RGBA.
    pub below:  Vec<Rgba8>,
    /// What is over them in `area`, on transparency; `None`: nothing.
    pub above:  Option<Vec<Rgba8>>,
    /// What moves of each layer, bottom up.
    pub layers: Vec<LiftLayer>,
}

impl std::fmt::Debug for Lift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lift {{ src: {:?}, area: {:?}, layers: {}, above: {} }}", self.src, self.area, self.layers.len(), self.above.is_some())
    }
}

pub struct LiftLayer {
    /// The source rect with a transparent pixel all around: (width + 2) ×
    /// (height + 2), premultiplied RGBA.
    pub premul:  Vec<Rgba8>,
    pub opacity: f32,
}

/// What moves of `img`: its pixels in `src`, through `mask` if any.
fn lifted(img: &Image, src: Rect, mask: Option<&[u8]>) -> Vec<Rgba8> {
    let mut px = img.read(src);
    if let Some(m) = mask { for (p, c) in px.iter_mut().zip(m) { p[3] = aurastudio_core::math::mul255(p[3], *c); } }
    px
}

/// How far past the frame a preview draws (a warp thrown far away).
fn reach(s: &LayerStack) -> Rect { s.frame().grown(s.width.max(s.height) as i32) }

/// Pieces a big rect is drawn by.
const PIECE: u32 = 256;

impl Session {
    pub(crate) fn befores(&self) -> impl Iterator<Item = (u64, &Image)> { self.targets.iter().map(|t| (t.id, &t.before)) }
}

impl Document {
    pub(crate) fn transform_begin(&mut self, ids: &[u64]) -> Done {
        let mut done = self.transform_cancel();
        done.join(self.filter_cancel());
        let ids: Vec<u64> = ids.iter().copied().filter(|&id| matches!(self.stack.node(id), Some(Node::Layer(_))) && !self.stack.is_locked(id)).collect();
        if ids.is_empty() { done.refused = Some("Layer is locked or is a folder"); return done; }
        // What shows of them, inside the selection.
        let mut src = Rect::EMPTY;
        for &id in &ids {
            if let Some(b) = self.stack.image(id).and_then(|i| i.pixel_bounds()) { src = src.union(&b); }
        }
        if let Some(s) = &self.selection { src = src.intersect(&s.bbox()); }
        if src.is_empty() { done.refused = Some("Nothing to transform"); return done; }
        let mask = self.selection.as_ref().map(|s| s.mask(src));
        let mut targets = Vec::new();
        for id in ids {
            let Some(l) = self.stack.layer(id) else { continue };
            let before = l.surface.clone();
            let clear = LayerStack::clear_pixel(l.background);
            let px = lifted(&before, src, mask.as_deref());
            let mut base = before.clone();
            match &self.selection {
                Some(sel) => {
                    let m = sel.as_mask();
                    for c in base.chunks(src, PIECE) {
                        let k = m.read(c);
                        if k.iter().all(|v| *v == 0) { continue; }
                        let px: Vec<Rgba8> = base.read(c).into_iter().zip(&k).map(|(p, &k)| aurastudio_filters::mix(p, clear, k)).collect();
                        base.write(c, &px);
                    }
                }
                // All of it moves: what is left is empty.
                None => base.fill_rect(src, clear),
            }
            targets.push(Target { id, source: Source::new(src, &px), before, base });
        }
        let selection = self.selection.clone().map(|s| {
            let px: Vec<Rgba8> = s.mask(src).into_iter().map(|c| [0, 0, 0, c]).collect();
            (s, Source::new(src, &px))
        });
        self.transform = Some(Session { src, targets, selection, shown: Rect::EMPTY });
        done.other = true;
        done
    }

    /// The live preview's pieces for the transform under way, over `area`
    /// of the frame (None: around the source, as far again as its size,
    /// 256 px at least).
    pub fn transform_lift(&mut self, area: Option<Rect>) -> Option<Lift> {
        aurastudio_core::zone!("transform lift");
        let s = self.transform.as_ref()?;
        let src = s.src;
        let frame = self.stack.frame();
        let area = area.unwrap_or_else(|| src.grown(src.w().max(src.h()).max(256) as i32)).union(&src.intersect(&frame)).intersect(&frame);
        let order = self.stack.layer_ids();
        let rank = |id: u64| order.iter().position(|o| *o == id).unwrap_or(0);
        let top = s.targets.iter().map(|t| t.id).max_by_key(|id| rank(*id))?;
        let mask = s.selection.as_ref().map(|(sel, _)| sel.mask(src));
        let (sw, sh) = (src.w() as usize, src.h() as usize);
        let mut layers: Vec<(usize, LiftLayer)> = s.targets.iter().map(|t| {
            let px = lifted(&t.before, src, mask.as_deref());
            let mut premul = vec![[0u8; 4]; (sw + 2) * (sh + 2)];
            for (y, row) in px.chunks_exact(sw).enumerate() {
                let out = &mut premul[(y + 1) * (sw + 2) + 1..][..sw];
                for (o, p) in out.iter_mut().zip(row) { *o = blend::premultiply_u8(*p); }
            }
            let opacity = self.stack.node(t.id).map_or(0.0, |n| if n.visible() { n.opacity() } else { 0.0 });
            (rank(t.id), LiftLayer { premul, opacity })
        }).collect();
        layers.sort_by_key(|l| l.0);
        // Merged with what moves taken out: the bases swapped in, then back
        // (no copy of the stack).
        let mut s = self.transform.take()?;
        let swap = |stack: &mut LayerStack, s: &mut Session| for t in &mut s.targets { if let Some(i) = stack.image_mut(t.id) { std::mem::swap(i, &mut t.base); } };
        swap(&mut self.stack, &mut s);
        let split = aurastudio_compose::merge_split(&self.stack, top, area);
        swap(&mut self.stack, &mut s);
        self.transform = Some(s);
        let (below, above) = split?;
        Some(Lift { src, area, below, above, layers: layers.into_iter().map(|l| l.1).collect() })
    }

    pub(crate) fn transform_preview(&mut self, warp: &Warp, interp: Interp) -> Done {
        aurastudio_core::zone!("transform preview");
        let limit = reach(&self.stack);
        let Some(s) = self.transform.as_mut() else { return Done::default() };
        let dest = warp.dest_rect(s.src).map_or(Rect::EMPTY, |d| d.intersect(&limit));
        let mut region = Region::Nothing;
        region.add_rect(s.shown);
        region.add_rect(s.src);
        region.add_rect(dest);
        s.shown = dest;
        for t in &s.targets {
            let mut img = t.base.clone();
            for c in img.chunks(dest, PIECE) {
                let mut px = img.read(c);
                render(&t.source, warp, interp, c, &mut px);
                img.write(c, &px);
            }
            if let Some(i) = self.stack.image_mut(t.id) { *i = img; }
        }
        Done { region, layers: s.targets.iter().map(|t| t.id).collect(), ..Done::default() }
    }

    pub(crate) fn transform_apply(&mut self, warp: &Warp, interp: Interp) -> Done {
        let mut done = self.transform_preview(warp, interp);
        let Some(s) = self.transform.take() else { return done };
        let mut steps = Vec::new();
        for t in &s.targets {
            let Some(img) = self.stack.image_mut(t.id) else { continue };
            img.compact(s.shown.union(&s.src).snapped_out(img.tile_size()));
            let before = Before(t.before.snapshot(), t.before.fill());
            let patch = img.diff(&before.0);
            if !patch.is_empty() { steps.push(Step::Pixels { layer: t.id, patch, fill: None }); }
        }
        // The selection goes along.
        if let Some((old, source)) = &s.selection {
            let space = self.space();
            let dest = warp.dest_rect(s.src).map_or(Rect::EMPTY, |d| d.intersect(&space.frame()));
            // (What was selected outside the moved part stays.)
            let rest = Selection::combine(Some(old), Selection::rect(space, s.src), Mode::Subtract);
            let moved = if dest.is_empty() { rest } else {
                let mut px = vec![[0u8; 4]; dest.area() as usize];
                render(source, warp, interp, dest, &mut px);
                let mask: Vec<u8> = px.iter().map(|p| p[3]).collect();
                Selection::combine(rest.as_ref(), Selection::from_mask(space, dest, &mask), Mode::Add)
            };
            if moved != self.selection {
                steps.push(Step::Select(std::mem::replace(&mut self.selection, moved)));
                done.selection = true;
            }
        }
        if !steps.is_empty() {
            self.history.push("Transform", Step::Group { steps, undone: false });
            done.other = true;
        }
        done
    }

    pub(crate) fn transform_cancel(&mut self) -> Done {
        let Some(s) = self.transform.take() else { return Done::default() };
        let layers = s.targets.iter().map(|t| t.id).collect();
        for t in s.targets { if let Some(i) = self.stack.image_mut(t.id) { *i = t.before; } }
        let mut region = Region::Nothing;
        region.add_rect(s.shown);
        region.add_rect(s.src);
        Done { region, layers, ..Done::default() }
    }
}

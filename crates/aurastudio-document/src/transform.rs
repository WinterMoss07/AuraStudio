//! A transform under way: the layers it moves as they were, what is left
//! of them once what moves is lifted out, and what moves (the selected
//! pixels, else all of them), shown transformed until applied (one undo
//! step, the selection moved along) or cancelled.

use aurastudio_core::Rect;
use aurastudio_transform::{render, Interp, Source, Warp};
use aurastudio_layers::{Node};
use aurastudio_selection::{Selection};
use aurastudio_raster::Tiles;

use crate::Document;

pub(crate) struct Session {
    /// What moves, canvas pixels.
    src:       Rect,
    targets:   Vec<Target>,
    /// The selection when it began (what was lifted), and as a source to
    /// move it along.
    selection: Option<(Selection, Source)>,
    /// Pixels changed by the last preview.
    shown:     Rect,
    /// Pixels any preview wrote (put back by Cancel, saved for undo).
    touched:   Rect,
}

/// What a transform's live preview needs (the GPU puts it together while
/// a handle is dragged): the whole canvas under the moved layers and over
/// them, and what moves.
pub struct Lift {
    pub src:    Rect,
    /// Merged image up to the top moved layer (what is left of them in),
    /// canvas-sized RGBA.
    pub below:  Vec<u8>,
    /// What is over them, on transparency (straight RGBA); None: nothing.
    pub above:  Option<Vec<u8>>,
    /// What moves of each layer, bottom up.
    pub layers: Vec<LiftLayer>,
}

impl std::fmt::Debug for Lift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lift {{ src: {:?}, layers: {}, above: {} }}", self.src, self.layers.len(), self.above.is_some())
    }
}

pub struct LiftLayer {
    /// The source rectangle with a transparent pixel all around:
    /// (width + 2) × (height + 2), premultiplied RGBA.
    pub premul:  Vec<u8>,
    pub opacity: f32,
}

/// What moves of `before`: its pixels in `src`, through `mask` if any.
fn lifted(before: &Tiles, src: Rect, mask: Option<&[u8]>) -> Vec<u8> {
    let mut px = before.read_rect(src);
    if let Some(m) = mask {
        for (p, c) in px.chunks_exact_mut(4).zip(m) { p[3] = (p[3] as u32 * *c as u32 / 255) as u8; }
    }
    px
}

struct Target {
    id:     u64,
    before: Tiles,
    /// The layer with what moves taken out.
    base:   Tiles,
    source: Source,
}

/// The union of `a` and `b`, an empty one counting for nothing.
fn union(a: Rect, b: Rect) -> Rect {
    if a.2 <= a.0 || a.3 <= a.1 { return b; }
    if b.2 <= b.0 || b.3 <= b.1 { return a; }
    aurastudio_core::union(a, b)
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3));
    (r.0 < r.2 && r.1 < r.3).then_some(r)
}

impl Document {
    /// Start transforming paint layers `ids` (locked ones, folders and
    /// filter layers are left out). The selection, if any, says what
    /// moves. Returns what moves, or why nothing can.
    pub fn transform_begin(&mut self, ids: &[u64]) -> Result<Rect, &'static str> {
        self.transform_cancel();
        self.filter_cancel();
        let ids: Vec<u64> = ids.iter().copied().filter(|&id| matches!(self.layers.node(id), Some(Node::Layer(_))) && !self.layers.is_locked(id)).collect();
        if ids.is_empty() { return Err("Layer is locked or is a folder"); }
        // What shows of them, inside the selection.
        let mut src = (0, 0, 0, 0);
        for &id in &ids {
            let Some(Node::Layer(l)) = self.layers.node(id) else { continue };
            if let Some(b) = l.tiles.pixel_bounds() { src = union(src, b); }
        }
        if let Some(s) = &self.selection { src = intersect(src, s.bbox()).unwrap_or((0, 0, 0, 0)); }
        if src.2 <= src.0 || src.3 <= src.1 { return Err("Nothing to transform"); }
        let mask = self.selection.as_ref().map(|s| s.mask(src));
        let mut targets = Vec::new();
        for id in ids {
            let Some(Node::Layer(l)) = self.layers.node(id) else { continue };
            let before = l.tiles.clone();
            let px = lifted(&before, src, mask.as_deref());
            let base = match (&self.selection, &mask) {
                (Some(sel), Some(_)) => {
                    let mut base = before.clone();
                    let r = sel.bbox();
                    let old = base.read_rect(r);
                    let mut cleared = before.fill().repeat(old.len() / 4);
                    sel.blend_rect(r, &old, &mut cleared);
                    base.write_rect(r, &cleared);
                    base
                }
                _ => {
                    // All of it moves: what is left is empty (the pixels
                    // around kept as they are, invisible colors too).
                    let mut base = before.clone();
                    base.write_rect(src, &before.fill().repeat((src.2 - src.0) as usize * (src.3 - src.1) as usize));
                    base
                }
            };
            targets.push(Target { id, source: Source::new(src, &px), before, base });
        }
        let selection = self.selection.clone().map(|s| {
            let m = s.mask(src);
            let px: Vec<u8> = m.iter().flat_map(|c| [0, 0, 0, *c]).collect();
            (s, Source::new(src, &px))
        });
        self.transform = Some(Session { src, targets, selection, shown: (0, 0, 0, 0), touched: (0, 0, 0, 0) });
        Ok(src)
    }

    pub fn transforming(&self) -> bool { self.transform.is_some() }

    /// The live preview's pieces for the transform just begun.
    pub fn transform_lift(&mut self) -> Option<Lift> {
        let s = self.transform.as_ref()?;
        let src = s.src;
        let ids: Vec<u64> = s.targets.iter().map(|t| t.id).collect();
        // The top one in the stack order.
        let order = self.layers.layer_ids();
        let top = *ids.iter().max_by_key(|id| order.iter().position(|o| o == *id))?;
        let mask = s.selection.as_ref().map(|(sel, _)| sel.mask(src));
        let (sw, sh) = ((src.2 - src.0) as usize, (src.3 - src.1) as usize);
        let mut layers: Vec<(usize, LiftLayer)> = s.targets.iter().map(|t| {
            let px = lifted(&t.before, src, mask.as_deref());
            let mut premul = vec![0u8; (sw + 2) * (sh + 2) * 4];
            for (y, row) in px.chunks_exact(sw * 4).enumerate() {
                let out = &mut premul[((y + 1) * (sw + 2) + 1) * 4..][..sw * 4];
                for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    let a = p[3] as u32;
                    o.copy_from_slice(&[(p[0] as u32 * a / 255) as u8, (p[1] as u32 * a / 255) as u8, (p[2] as u32 * a / 255) as u8, p[3]]);
                }
            }
            let opacity = self.layers.node(t.id).map_or(0.0, |n| if n.visible() { n.opacity() } else { 0.0 });
            (order.iter().position(|o| *o == t.id).unwrap_or(0), LiftLayer { premul, opacity })
        }).collect();
        layers.sort_by_key(|l| l.0);
        // Merged with what moves taken out (swapped in, then back).
        let mut s = self.transform.take()?;
        let swap = |d: &mut Document, s: &mut Session| for t in &mut s.targets {
            if let Some(Node::Layer(l)) = d.layers.node_mut(t.id) { std::mem::swap(&mut l.tiles, &mut t.base); }
        };
        swap(self, &mut s);
        let split = self.layers.merge_split(top);
        swap(self, &mut s);
        self.transform = Some(s);
        let (below, above) = split?;
        Some(Lift { src, below, above, layers: layers.into_iter().map(|l| l.1).collect() })
    }

    /// What the transform under way moves.
    pub fn transform_source(&self) -> Option<Rect> { self.transform.as_ref().map(|s| s.src) }

    /// Show the layers transformed by `warp`. Returns the area to redraw.
    pub fn transform_preview(&mut self, warp: &Warp, interp: Interp) -> Option<Rect> {
        let (w, h) = (self.layers.width, self.layers.height);
        let s = self.transform.as_mut()?;
        let dest = warp.dest_rect(s.src, w, h).unwrap_or((0, 0, 0, 0));
        let now = union(s.src, dest);
        let area = union(s.shown, now);
        s.shown = now;
        s.touched = union(s.touched, area);
        let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
        if aw == 0 || ah == 0 { return None; }
        for t in &s.targets {
            let mut px = t.base.read_rect(area);
            render(&t.source, warp, interp, area, &mut px);
            self.layers.set_layer_rect(t.id, area, &px);
        }
        Some(area)
    }

    /// Keep the transform (one undo step, the selection moved along).
    /// Returns the layers and the area changed.
    pub fn transform_apply(&mut self, warp: &Warp, interp: Interp) -> Option<(Vec<u64>, Rect)> {
        let (w, h) = (self.layers.width, self.layers.height);
        self.transform_preview(warp, interp);
        let s = self.transform.take()?;
        let area = s.touched;
        let mut steps = Vec::new();
        for t in &s.targets {
            if self.layers.node(t.id).is_none() { continue; }
            steps.push((t.id, area, t.before.read_rect(area)));
            if let Some(Node::Layer(l)) = self.layers.node_mut(t.id) { l.tiles.drop_fill_in(area); }
        }
        // The selection goes along.
        let moved = s.selection.as_ref().map(|(old, src)| {
            let dest = warp.dest_rect(s.src, w, h)?;
            let (dw, dh) = ((dest.2 - dest.0) as usize, (dest.3 - dest.1) as usize);
            let mut px = vec![0u8; dw * dh * 4];
            render(src, warp, interp, dest, &mut px);
            let mask: Vec<u8> = px.chunks_exact(4).map(|p| p[3]).collect();
            // (What was selected outside the moved part stays.)
            let rest = Selection::combine(Some(old), Selection::rect(w, h, s.src), aurastudio_selection::SelectMode::Subtract);
            Selection::combine(rest.as_ref(), Selection::from_mask(w, h, dest, &mask), aurastudio_selection::SelectMode::Add)
        });
        self.record_tree("Layers");
        let sel_step = match moved {
            Some(new) if new != self.selection => Some(std::mem::replace(&mut self.selection, new)),
            _ => None,
        };
        self.history.push_group("Transform", steps, sel_step);
        self.touch();
        Some((s.targets.iter().map(|t| t.id).collect(), area))
    }

    /// Put the layers back as they were. Returns them and the area.
    pub fn transform_cancel(&mut self) -> Option<(Vec<u64>, Rect)> {
        let s = self.transform.take()?;
        let r = s.touched;
        if r.2 > r.0 && r.3 > r.1 {
            for t in &s.targets { self.layers.set_layer_rect(t.id, r, &t.before.read_rect(r)); }
        }
        Some((s.targets.iter().map(|t| t.id).collect(), r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocId;
    use aurastudio_transform::corners;

    fn px(d: &Document, id: u64, x: u32, y: u32) -> [u8; 4] {
        let Some(Node::Layer(l)) = d.layers.node(id) else { panic!() };
        l.tiles.pixel(x, y)
    }

    #[test]
    fn move_apply_undo_cancel() {
        let mut d = Document::new(DocId(1), "t", 100, 100);
        let id = d.layers.add_layer("ink");
        d.layers.set_layer_rect(id, (10, 10, 30, 30), &[0u8, 0, 255, 255].repeat(400));
        let src = d.transform_begin(&[id]).unwrap();
        assert_eq!(src, (10, 10, 30, 30));
        let moved = Warp { quad: corners((50, 60, 70, 80)), mesh: None };
        d.transform_preview(&moved, Interp::Bicubic).unwrap();
        assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
        assert_eq!(px(&d, id, 15, 15)[3], 0);
        // Cancel: as it was.
        d.transform_cancel().unwrap();
        assert_eq!(px(&d, id, 15, 15), [0, 0, 255, 255]);
        assert_eq!(px(&d, id, 55, 65)[3], 0);
        // Apply: one step, undone at once.
        d.transform_begin(&[id]).unwrap();
        d.transform_preview(&Warp { quad: corners((0, 0, 5, 5)), mesh: None }, Interp::Bilinear);
        d.record_tree("New layer");
        let n = d.history().undo_len();
        d.transform_apply(&moved, Interp::Bicubic).unwrap();
        assert!(!d.transforming());
        assert_eq!(d.history().undo_len(), n + 1);
        assert_eq!(d.history().undo_label(), Some("Transform"));
        assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
        assert_eq!(px(&d, id, 2, 2)[3], 0, "the old preview is gone");
        d.undo();
        assert_eq!(px(&d, id, 15, 15), [0, 0, 255, 255]);
        assert_eq!(px(&d, id, 55, 65)[3], 0);
        d.redo();
        assert_eq!(px(&d, id, 55, 65), [0, 0, 255, 255]);
        assert!(d.transform_begin(&[]).is_err());
        let empty = d.layers.add_layer("empty");
        assert_eq!(d.transform_begin(&[empty]), Err("Nothing to transform"));
    }

    #[test]
    fn a_selection_says_what_moves_and_goes_along() {
        let mut d = Document::new(DocId(1), "t", 100, 100);
        let (a, b) = (d.layers.add_layer("a"), d.layers.add_layer("b"));
        d.layers.set_layer_rect(a, (0, 0, 100, 100), &[255u8, 0, 0, 255].repeat(10_000));
        d.layers.set_layer_rect(b, (0, 0, 100, 100), &[0u8, 255, 0, 255].repeat(10_000));
        d.selection = Selection::rect(100, 100, (0, 0, 20, 20));
        let src = d.transform_begin(&[a, b]).unwrap();
        assert_eq!(src, (0, 0, 20, 20));
        let warp = Warp { quad: corners((50, 50, 70, 70)), mesh: None };
        d.transform_apply(&warp, Interp::Bicubic).unwrap();
        for (id, c) in [(a, [255, 0, 0, 255]), (b, [0, 255, 0, 255])] {
            assert_eq!(px(&d, id, 5, 5)[3], 0, "lifted out");
            assert_eq!(px(&d, id, 30, 5), c, "the rest stays");
            assert_eq!(px(&d, id, 60, 60), c);
        }
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (50, 50, 70, 70));
        d.undo();
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (0, 0, 20, 20));
        assert_eq!(px(&d, a, 5, 5), [255, 0, 0, 255]);
        assert_eq!(px(&d, b, 5, 5), [0, 255, 0, 255]);
        // Saved: the group's steps one by one.
        let (_, redo) = d.history().export(100);
        assert_eq!(redo.iter().filter(|s| s.label == "Transform").count(), 2);
    }

    /// The live preview's pieces, put back together where nothing moved,
    /// give the image as it was; what moves is not in the part under it.
    #[test]
    fn the_lift_pieces_make_the_image() {
        let mut d = Document::new(DocId(1), "t", 100, 80);
        let id = d.layers.add_layer("ink");
        d.layers.set_layer_rect(id, (10, 10, 60, 50), &[0u8, 0, 255, 255].repeat(50 * 40));
        let top = d.layers.add_layer("top");
        d.layers.set_layer_rect(top, (0, 0, 100, 20), &[255u8, 0, 0, 120].repeat(100 * 20));
        let whole = d.layers.composite_rect((0, 0, 100, 80));
        d.layers.set_active(id);
        let src = d.transform_begin(&[id]).unwrap();
        let lift = d.transform_lift().unwrap();
        assert_eq!(lift.src, src);
        // Under: the ink taken out (paper white there).
        assert_eq!(&lift.below[(30 * 100 + 30) * 4..][..3], &[255, 255, 255]);
        let above = lift.above.as_ref().expect("the top layer shows");
        let (sw, sh) = ((src.2 - src.0) as usize, (src.3 - src.1) as usize);
        assert_eq!(lift.layers.len(), 1);
        assert_eq!(lift.layers[0].premul.len(), (sw + 2) * (sh + 2) * 4);
        for y in 0..80usize {
            for x in 0..100usize {
                let mut c: Vec<f32> = lift.below[(y * 100 + x) * 4..][..3].iter().map(|v| *v as f32).collect();
                if x >= src.0 as usize && x < src.2 as usize && y >= src.1 as usize && y < src.3 as usize {
                    let p = &lift.layers[0].premul[((y - src.1 as usize + 1) * (sw + 2) + x - src.0 as usize + 1) * 4..][..4];
                    let a = p[3] as f32 / 255.0;
                    for k in 0..3 { c[k] = p[k] as f32 + c[k] * (1.0 - a); }
                }
                let o = &above[(y * 100 + x) * 4..][..4];
                let a = o[3] as f32 / 255.0;
                for k in 0..3 {
                    let v = o[k] as f32 * a + c[k] * (1.0 - a);
                    assert!((v - whole[(y * 100 + x) * 4 + k] as f32).abs() <= 2.0, "at {x},{y}");
                }
            }
        }
        // The layer itself is untouched by it.
        assert_eq!(px(&d, id, 30, 30), [0, 0, 255, 255]);
    }
}

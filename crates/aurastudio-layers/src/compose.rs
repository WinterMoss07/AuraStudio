//! The general merge: clipping masks, filter layers, and merges of some
//! layers only (the fill tool's references, exports without the drafts).
//!
//! The plain merge (`composite_row_opaque`, a few ns per pixel) goes row
//! by row through the tree. A filter layer needs the merge of what is
//! under it over a whole area (a blur reads its neighbors), so this one
//! goes node by node over a block: the area asked for, grown by what the
//! filter layers reach, in bands. It works in premultiplied floats: slower,
//! used only when the tree needs it.
//!
//! Clipping (CSP, Photoshop with "blend clipped layers as group"): a
//! clipped node shows only where its base — the nearest node under it
//! that isn't clipped, in the same folder — has pixels. The base and its
//! clipped nodes are merged on their own (each clipped node with its own
//! blend mode and opacity, over the base), then that is blended down
//! with the base's blend mode and opacity: lowering the base's opacity
//! fades the whole group. A hidden base hides what is clipped to it; a
//! clipped node at the bottom of its folder has no base and shows as it
//! is.

use aurastudio_canvas::blend::{blend_over, unit_to_u8, Premul};
use aurastudio_canvas::{BlendMode, Rect};

use crate::{FolderBlend, LayerStack, Node};

/// Which nodes a merge takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// Leave out the draft layers (and what draft folders hold).
    pub skip_drafts:     bool,
    /// Only the reference layers (and what reference folders hold).
    pub references_only: bool,
    /// Leave out this node (0: none): the layer being painted, which
    /// mustn't stop its own strokes (CSP's "exclude editing layer").
    pub skip:            u64,
    /// Stop at this node (0: never): the merge of what is under it (a
    /// filter layer's dialog shows what it filters).
    pub until:           u64,
}

impl Pick {
    pub const ALL: Pick = Pick { skip_drafts: false, references_only: false, skip: 0, until: 0 };
    /// What an exported image shows (CSP leaves drafts out).
    pub const EXPORT: Pick = Pick { skip_drafts: true, ..Pick::ALL };
}

/// Rows per band of the general merge (plus the margin above and below).
const BAND: u32 = 128;
/// The most a stack of filter layers may read around a pixel.
const MAX_MARGIN: u32 = 4096;

/// A block of premultiplied pixels.
struct Block {
    rect: Rect,
    w:    usize,
    px:   Vec<Premul>,
}

impl Block {
    fn new(rect: Rect, start: Premul) -> Block {
        let w = (rect.2 - rect.0) as usize;
        Block { rect, w, px: vec![start; w * (rect.3 - rect.1) as usize] }
    }
}

/// How much each pixel of a block lets through (clipping), 0..1.
type Mask = Vec<f32>;

impl LayerStack {
    /// The tree has something only the general merge does: a visible
    /// filter layer, or a clipped node with a base.
    pub fn needs_general_merge(&self) -> bool {
        fn walk(nodes: &[Node]) -> bool {
            nodes.iter().enumerate().any(|(i, n)| n.visible() && match n {
                Node::Filter(_) => true,
                _ if n.props().clip && i > 0 => true,
                Node::Folder(f) => walk(&f.children),
                _ => false,
            })
        }
        walk(&self.root)
    }

    /// How far around a pixel the whole merge reads (the visible filter
    /// layers' reach, stacked). 0: each pixel on its own.
    pub fn merge_margin(&self) -> u32 { margin(&self.root, Pick::ALL).min(MAX_MARGIN) }

    /// The merge of the nodes `pick` takes, over `rect` (clipped to the
    /// canvas): on white (an opaque image), or on transparency
    /// (`transparent`: what the picked layers cover). RGBA rows, straight.
    pub fn composite_pick(&self, rect: Rect, pick: Pick, transparent: bool) -> Vec<u8> {
        let r = (rect.0, rect.1, rect.2.min(self.width), rect.3.min(self.height));
        if r.0 >= r.2 || r.1 >= r.3 { return Vec::new(); }
        let mut out = vec![0u8; (r.2 - r.0) as usize * (r.3 - r.1) as usize * 4];
        if pick == Pick::ALL && !transparent && !self.needs_general_merge() {
            self.composite_rect_to(r, &mut out);
        } else {
            self.general_to(r, pick, transparent, &mut out);
        }
        out
    }

    /// The general merge of `rect` (inside the canvas) into `out`, by
    /// bands.
    pub(crate) fn general_to(&self, rect: Rect, pick: Pick, transparent: bool, out: &mut [u8]) {
        let m = margin(&self.root, pick).min(MAX_MARGIN);
        let n = (rect.2 - rect.0) as usize;
        let start = if transparent { [0.0; 4] } else { [1.0; 4] };
        let band = self.band_rows(rect, m);
        let mut y = rect.1;
        while y < rect.3 {
            let y1 = (y + band).min(rect.3);
            let ext = (rect.0.saturating_sub(m), y.saturating_sub(m), (rect.2 + m).min(self.width), (y1 + m).min(self.height));
            let mut b = Block::new(ext, start);
            let mut stop = false;
            compose(&self.root, &mut b, 1.0, pick, false, None, &mut stop);
            for yy in y..y1 {
                let src = &b.px[(yy - ext.1) as usize * b.w + (rect.0 - ext.0) as usize..][..n];
                let dst = &mut out[(yy - rect.1) as usize * n * 4..][..n * 4];
                for (d, p) in dst.chunks_exact_mut(4).zip(src) {
                    d.copy_from_slice(&straight(*p));
                }
            }
            y = y1;
        }
    }
}

impl LayerStack {
    /// Rows per band to merge `rect` with a margin of `m`: the margin is
    /// merged again for each band, so all at once when it fits (~24 M
    /// pixels), else bands at least twice the margin tall. Sizes clamped to
    /// the canvas (a margin as big as the canvas made every band merge all
    /// of it again: 50 s on an A4 page).
    fn band_rows(&self, rect: Rect, m: u32) -> u32 {
        let ext_w = ((rect.2 + m).min(self.width) - rect.0.saturating_sub(m)) as u64;
        let ext_h = |band: u32| ((rect.1 + band + m).min(self.height) - rect.1.saturating_sub(m)) as u64;
        const BUDGET: u64 = 24 << 20;
        let all = rect.3 - rect.1;
        let mut band = if ext_w * ext_h(all) <= BUDGET { all } else { BAND.max(2 * m) };
        while band > BAND && band < all && ext_w * ext_h(band) > BUDGET { band /= 2; }
        band.max(1)
    }

    /// `rect` cut in bands to merge one at a time (to show them as they
    /// come): `rows` tall, or as the general merge needs them (bands of
    /// 64 rows each reading a 500 px margin would merge it 16 times over).
    pub fn merge_bands(&self, rect: Rect, rows: u32) -> Vec<Rect> {
        let rows = if self.needs_general_merge() { self.band_rows(rect, self.merge_margin()).max(rows) } else { rows.max(1) };
        (rect.1..rect.3).step_by(rows as usize).map(|y| (rect.0, y, rect.2, (y + rows).min(rect.3))).collect()
    }
}

/// How far the visible filter layers `pick` takes read around a pixel,
/// stacked (each reads the result of the ones under it).
fn margin(nodes: &[Node], pick: Pick) -> u32 {
    nodes.iter().map(|n| {
        if !n.visible() || (pick.skip_drafts && n.props().draft) { return 0; }
        match n {
            Node::Filter(f) => f.filter.reach(),
            Node::Folder(f) => margin(&f.children, pick),
            Node::Layer(_) => 0,
        }
    }).sum()
}

fn straight(p: Premul) -> [u8; 4] {
    if p[3] <= 0.0 { return [255, 255, 255, 0]; }
    let a = p[3].min(1.0);
    [unit_to_u8(p[0] / a), unit_to_u8(p[1] / a), unit_to_u8(p[2] / a), unit_to_u8(a)]
}

/// Whether `n` is drawn (it, or for a folder its content).
fn taken(n: &Node, pick: Pick, in_reference: bool) -> bool {
    if !n.visible() || n.opacity() <= 0.0 || n.id() == pick.skip { return false; }
    if pick.skip_drafts && n.props().draft { return false; }
    if pick.references_only && !in_reference && !n.props().reference && !n.is_folder() { return false; }
    true
}

/// Merge `nodes` (bottom to top) onto `b`. `scale`: the opacity of the
/// pass-through folders around; `in_reference`: inside a reference
/// folder; `mask`: the clipping around them, if any.
/// `stop`: set once `pick.until` is reached; nothing more is drawn.
fn compose(nodes: &[Node], b: &mut Block, scale: f32, pick: Pick, in_reference: bool, mask: Option<&[f32]>, stop: &mut bool) {
    let mut i = 0;
    while i < nodes.len() && !*stop {
        if nodes[i].id() == pick.until { *stop = true; return; }
        // A base and the clipped nodes over it.
        let base = &nodes[i];
        let mut j = i + 1;
        while j < nodes.len() && nodes[j].props().clip { j += 1; }
        let clipped = &nodes[i + 1..j];
        let base_ref = in_reference || base.props().reference;
        let is_filter = matches!(base, Node::Filter(_));
        let wanted = !is_filter && clipped.iter().any(|c| taken(c, pick, base_ref));
        if !taken(base, pick, in_reference) {
            // (A hidden base hides its clipped nodes.)
        } else if !wanted {
            draw(base, b, scale, pick, in_reference, mask, false, stop);
            if is_filter {
                // Clipped to a filter layer: nothing to clip to.
                for c in clipped {
                    if *stop || c.id() == pick.until { *stop = true; return; }
                    if taken(c, pick, in_reference) { draw(c, b, scale, pick, in_reference, mask, false, stop); }
                }
            }
        } else {
            // The group: the base alone (full opacity, normal), then the
            // clipped nodes through its coverage.
            let mut g = Block::new(b.rect, [0.0; 4]);
            draw(base, &mut g, 1.0, pick, in_reference, None, true, stop);
            let cover: Mask = g.px.iter().map(|p| p[3]).collect();
            for c in clipped {
                if *stop || c.id() == pick.until { *stop = true; break; }
                if taken(c, pick, base_ref) { draw(c, &mut g, 1.0, pick, base_ref, Some(&cover), false, stop); }
            }
            let (op, mode) = match base {
                Node::Layer(l) => (l.opacity, l.blend),
                Node::Folder(f) => (f.opacity, match f.blend { FolderBlend::Isolated(m) => m, FolderBlend::PassThrough => BlendMode::Normal }),
                Node::Filter(_) => unreachable!(),
            };
            blend_block(b, &g, op * scale, mode, mask);
        }
        i = j;
    }
}

/// Blend premultiplied block `g` onto `b` at `op`, through `mask`.
fn blend_block(b: &mut Block, g: &Block, op: f32, mode: BlendMode, mask: Option<&[f32]>) {
    for (k, (d, p)) in b.px.iter_mut().zip(&g.px).enumerate() {
        if p[3] <= 0.0 { continue; }
        let m = mask.map_or(1.0, |m| m[k]);
        let cs = [p[0] / p[3], p[1] / p[3], p[2] / p[3]];
        blend_over(d, cs, p[3].min(1.0) * op * m, mode);
    }
}

/// Draw `n` onto `b`, through `mask` if clipped; `alone`: as the base of
/// a clipping group (full opacity, normal blending: the group gets them).
#[allow(clippy::too_many_arguments)]
fn draw(n: &Node, b: &mut Block, scale: f32, pick: Pick, in_reference: bool, mask: Option<&[f32]>, alone: bool, stop: &mut bool) {
    let is_ref = in_reference || n.props().reference;
    match n {
        Node::Layer(l) => {
            if pick.references_only && !is_ref { return; }
            let op = if alone { 1.0 } else { l.opacity * scale };
            let blend = if alone { BlendMode::Normal } else { l.blend };
            let fill = l.tiles.fill();
            let run = crate::fill_run(fill);
            for (row, y) in (b.rect.1..b.rect.3).enumerate() {
                let line = &mut b.px[row * b.w..(row + 1) * b.w];
                let mrow = mask.map(|m| &m[row * b.w..(row + 1) * b.w]);
                l.tiles.for_row(y, b.rect.0, b.rect.2, |x, len, px| {
                    let src = match px {
                        Some(src) => src,
                        None if fill[3] == 0 => return,
                        None => &run[..len * 4],
                    };
                    let o = (x - b.rect.0) as usize;
                    for (k, (d, s)) in line[o..o + len].iter_mut().zip(src.chunks_exact(4)).enumerate() {
                        if s[3] == 0 { continue; }
                        let m = mrow.map_or(1.0, |m| m[o + k]);
                        if m <= 0.0 { continue; }
                        let cs = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
                        blend_over(d, cs, s[3] as f32 / 255.0 * op * m, blend);
                    }
                });
            }
        }
        Node::Folder(f) => match (f.blend, mask, alone) {
            (FolderBlend::PassThrough, None, false) => compose(&f.children, b, scale * f.opacity, pick, is_ref, None, stop),
            // Clipped, isolated, or a group's base: merged on its own first.
            (blend, _, _) => {
                let mode = match blend { FolderBlend::Isolated(m) if !alone => m, _ => BlendMode::Normal };
                let mut t = Block::new(b.rect, [0.0; 4]);
                compose(&f.children, &mut t, 1.0, pick, is_ref, None, stop);
                let op = if alone { 1.0 } else { f.opacity * scale };
                blend_block(b, &t, op, mode, mask);
            }
        },
        Node::Filter(fl) => {
            if pick.references_only && !is_ref { return; }
            let op = if alone { 1.0 } else { fl.opacity * scale };
            // The filter works on straight 8-bit pixels, as on a layer.
            let src: Vec<u8> = b.px.iter().flat_map(|p| straight(*p)).collect();
            let rows = b.px.len() / b.w.max(1);
            let out = fl.filter.apply(&src, b.w, rows);
            for (k, (d, o)) in b.px.iter_mut().zip(out.chunks_exact(4)).enumerate() {
                let t = op * mask.map_or(1.0, |m| m[k]);
                if t <= 0.0 { continue; }
                let a = o[3] as f32 / 255.0;
                let f = [o[0] as f32 / 255.0 * a, o[1] as f32 / 255.0 * a, o[2] as f32 / 255.0 * a, a];
                for c in 0..4 { d[c] += (f[c] - d[c]) * t.min(1.0); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Layer, Props, Tiles};
    use aurastudio_filters::{Filter, Kind};

    const W: u32 = 96;
    const H: u32 = 64;

    /// A stack (no Paper) with a layer painted `rgba` over `r`.
    fn stack() -> LayerStack {
        let mut s = LayerStack::new(W, H);
        if let Some(Node::Layer(bg)) = s.root.first_mut() { bg.tiles = Tiles::new(W, H, [255, 255, 255, 255]); }
        s
    }

    fn paint(s: &mut LayerStack, name: &str, r: Rect, rgba: [u8; 4]) -> u64 {
        let id = s.add_layer(name);
        let n = ((r.2 - r.0) * (r.3 - r.1)) as usize;
        assert!(s.set_layer_rect(id, r, &rgba.repeat(n)));
        id
    }

    fn px(s: &LayerStack, x: u32, y: u32) -> [u8; 4] { s.composite_rect((x, y, x + 1, y + 1)).try_into().unwrap() }

    fn close(a: [u8; 4], b: [u8; 4]) -> bool { a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1) }

    #[test]
    fn general_merge_matches_the_plain_one() {
        let mut s = stack();
        paint(&mut s, "a", (10, 10, 60, 50), [200, 40, 30, 255]);
        let b = paint(&mut s, "b", (30, 0, 90, 40), [20, 90, 200, 128]);
        if let Some(Node::Layer(l)) = s.node_mut(b) { l.blend = BlendMode::Multiply; l.opacity = 0.7; }
        let f = s.add_folder("f");
        if let Some(Node::Folder(f)) = s.node_mut(f) { f.blend = FolderBlend::Isolated(BlendMode::Screen); f.opacity = 0.6; }
        paint(&mut s, "in", (0, 20, 50, 64), [60, 200, 60, 200]);
        assert!(!s.needs_general_merge());
        let plain = s.composite_rect((0, 0, W, H));
        let mut general = vec![0u8; plain.len()];
        s.general_to((0, 0, W, H), Pick::ALL, false, &mut general);
        let worst = plain.iter().zip(&general).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "differ by {worst}");
    }

    #[test]
    fn clipping_shows_only_over_the_base() {
        let mut s = stack();
        let base = paint(&mut s, "base", (0, 0, 48, 64), [255, 0, 0, 255]);
        let c = paint(&mut s, "clipped", (0, 0, 96, 64), [0, 0, 255, 255]);
        let d = paint(&mut s, "clipped 2", (0, 0, 20, 64), [0, 255, 0, 255]);
        s.node_mut(c).unwrap().props_mut().clip = true;
        s.node_mut(d).unwrap().props_mut().clip = true;
        assert!(s.needs_general_merge());
        assert_eq!((s.clip_base(c), s.clip_base(d), s.clip_base(base)), (Some(base), Some(base), None));
        assert!(close(px(&s, 30, 5), [0, 0, 255, 255]), "over the base");
        assert!(close(px(&s, 70, 5), [255, 255, 255, 255]), "beside it: nothing");
        assert!(close(px(&s, 10, 5), [0, 255, 0, 255]), "two clipped on one base");
        // Half the base's opacity fades the whole group (Photoshop): the
        // clipped blue at half over white, no red showing through.
        *s.node_mut(base).unwrap().opacity_mut() = 0.5;
        assert!(close(px(&s, 30, 5), [128, 128, 255, 255]), "{:?}", px(&s, 30, 5));
        // A hidden base hides what is clipped to it.
        *s.node_mut(base).unwrap().visible_mut() = false;
        assert!(close(px(&s, 30, 5), [255, 255, 255, 255]));
        // Clipped at the bottom of the stack (no base): shown as it is.
        let mut t = LayerStack::new(W, H);
        if let Some(Node::Layer(l)) = t.root.first_mut() { l.props.clip = true; }
        assert!(!t.needs_general_merge());
    }

    #[test]
    fn filter_layers_change_what_is_under_them() {
        let mut s = stack();
        paint(&mut s, "gray", (0, 0, 96, 64), [100, 100, 100, 255]);
        let f = s.add_filter_layer(Filter::new(Kind::Invert));
        assert!(close(px(&s, 5, 5), [155, 155, 155, 255]));
        // Opacity: half way.
        *s.node_mut(f).unwrap().opacity_mut() = 0.5;
        assert!(close(px(&s, 5, 5), [128, 128, 128, 255]));
        *s.node_mut(f).unwrap().opacity_mut() = 1.0;
        // Layers over it are not filtered.
        s.set_active(f);
        paint(&mut s, "over", (0, 0, 10, 10), [10, 20, 30, 255]);
        assert_eq!(px(&s, 5, 5), [10, 20, 30, 255]);
        // Clipped to a base: only there.
        let mut t = stack();
        paint(&mut t, "gray", (0, 0, 96, 64), [100, 100, 100, 255]);
        paint(&mut t, "base", (0, 0, 48, 64), [100, 100, 100, 255]);
        let f = t.add_filter_layer(Filter::new(Kind::Invert));
        t.node_mut(f).unwrap().props_mut().clip = true;
        assert!(close(px(&t, 10, 5), [155, 155, 155, 255]));
        assert!(close(px(&t, 70, 5), [100, 100, 100, 255]));
    }

    #[test]
    fn filter_layer_in_an_isolated_folder_keeps_to_it() {
        let mut s = stack();
        paint(&mut s, "outside", (0, 0, 96, 64), [100, 100, 100, 255]);
        let f = s.add_folder("f");
        if let Some(Node::Folder(f)) = s.node_mut(f) { f.blend = FolderBlend::Isolated(BlendMode::Normal); }
        paint(&mut s, "inside", (0, 0, 48, 64), [40, 40, 40, 255]);
        s.add_filter_layer(Filter::new(Kind::Invert));
        assert!(close(px(&s, 10, 5), [215, 215, 215, 255]), "{:?}", px(&s, 10, 5));
        assert!(close(px(&s, 70, 5), [100, 100, 100, 255]), "outside the folder: untouched");
    }

    #[test]
    fn a_blur_layer_gives_the_same_pixels_in_parts() {
        let mut s = stack();
        paint(&mut s, "dot", (40, 24, 56, 40), [0, 0, 0, 255]);
        s.add_filter_layer(Filter { values: vec![6.0], ..Filter::new(Kind::GaussianBlur) });
        let whole = s.composite_rect((0, 0, W, H));
        for r in [(30, 20, 50, 30), (0, 0, 8, 8), (50, 30, 96, 64)] {
            let part = s.composite_rect(r);
            for y in r.1..r.3 {
                for x in r.0..r.2 {
                    let a = &whole[((y * W + x) * 4) as usize..][..4];
                    let b = &part[(((y - r.1) * (r.2 - r.0) + x - r.0) * 4) as usize..][..4];
                    assert_eq!(a, b, "({x}, {y}) in {r:?}");
                }
            }
        }
        // It did blur: gray around the dot.
        let p = px(&s, 37, 32);
        assert!(p[0] > 20 && p[0] < 235, "{p:?}");
    }

    #[test]
    fn picks_leave_out_drafts_and_keep_references() {
        let mut s = stack();
        let ink = paint(&mut s, "ink", (0, 0, 10, 64), [0, 0, 0, 255]);
        let sketch = paint(&mut s, "sketch", (20, 0, 30, 64), [0, 0, 255, 255]);
        s.node_mut(sketch).unwrap().props_mut().draft = true;
        let draft = Pick { skip_drafts: true, ..Pick::ALL };
        let at = |s: &LayerStack, x: u32, pick: Pick, t: bool| -> [u8; 4] { s.composite_pick((x, 5, x + 1, 6), pick, t).try_into().unwrap() };
        assert_eq!(at(&s, 25, Pick::ALL, false), [0, 0, 255, 255]);
        assert_eq!(at(&s, 25, draft, false), [255, 255, 255, 255]);
        // References only: nothing marked, nothing seen.
        let refs = Pick { references_only: true, ..Pick::ALL };
        assert_eq!(at(&s, 5, refs, true)[3], 0);
        s.node_mut(ink).unwrap().props_mut().reference = true;
        assert_eq!(at(&s, 5, refs, true), [0, 0, 0, 255]);
        assert_eq!(at(&s, 25, refs, true)[3], 0, "not a reference");
        // A reference folder: everything in it counts.
        let mut t = stack();
        let f = t.add_folder("lines");
        t.node_mut(f).unwrap().props_mut().reference = true;
        paint(&mut t, "in folder", (0, 0, 10, 64), [0, 0, 0, 255]);
        assert_eq!(at(&t, 5, refs, true), [0, 0, 0, 255]);
        // Skip a layer; stop under a node.
        let skip = Pick { skip: sketch, ..Pick::ALL };
        assert_eq!(at(&s, 25, skip, false), [255, 255, 255, 255]);
        let under = Pick { until: sketch, ..Pick::ALL };
        assert_eq!(at(&s, 25, under, false), [255, 255, 255, 255]);
        assert_eq!(at(&s, 5, under, false), [0, 0, 0, 255], "what is under it stays");
        let _ = Props::default();
        let _ = Layer::new(1, 1, false);
    }
}

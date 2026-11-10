//! The merge: pieces of the rect (each grown by the filter layers' reach)
//! on all cores; in each, node by node onto a block of premultiplied
//! floats.
//!
//! Clipping (Photoshop's "blend clipped layers as group"): a base and the
//! clipped nodes over it are merged on their own, then blended down with
//! the base's mode and opacity. A hidden base hides them.


use aurastudio_core::blend::{blend_over, to_straight_u8, Premul};
use aurastudio_core::{BlendMode, Rect, Rgba8};
use aurastudio_layers::{ghosts_in, Folder, FolderBlend, Ghost, LayerStack, Node, Pick};
use aurastudio_raster::Run;
use rayon::prelude::*;


/// What is behind the merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// Opaque white (the screen, an export, a thumbnail).
    #[default]
    White,
    /// Nothing: what the merged nodes cover (a reference, the part over a
    /// transform).
    Transparent,
}

/// How to merge.
#[derive(Clone, Copy, Default)]
pub struct Opts<'a> {
    pub pick:     Pick,
    pub backdrop: Backdrop,
    /// Draw the onion ghosts (the screen only).
    pub ghosts:   bool,
    /// Nodes drawn as if hidden, ids sorted (the part of the stack under
    /// or over a node, without a copy of the stack).
    pub hidden:   &'a [u64],
}

/// The most a stack of filter layers may read around a pixel.
const MAX_MARGIN: u32 = 4096;

/// A block of premultiplied pixels over `rect`.
pub(crate) struct Block {
    pub rect: Rect,
    pub w:    usize,
    pub px:   Vec<Premul>,
}

/// Blocks given back on each thread, for the next merge to reuse (a merge
/// makes one per piece and per folder, clipping group or ghost in it):
/// at most `SPARE_MAX` of at most `SPARE_BYTES` each.
const SPARE_MAX: usize = 3;
const SPARE_BYTES: usize = 1 << 20;

thread_local! {
    static SPARE: std::cell::RefCell<Vec<Vec<Premul>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Block {
    /// A block of `rect` filled with `start` (its memory reused when this
    /// thread has a spare one).
    pub fn new(rect: Rect, start: Premul) -> Block {
        let n = rect.area() as usize;
        let mut px = SPARE.with(|s| {
            let mut s = s.borrow_mut();
            // (The largest: the least likely to grow.)
            let k = (0..s.len()).max_by_key(|&i| s[i].capacity())?;
            Some(s.swap_remove(k))
        }).unwrap_or_default();
        px.clear();
        px.resize(n, start);
        Block { rect, w: rect.w() as usize, px }
    }

}

impl Drop for Block {
    fn drop(&mut self) {
        let px = std::mem::take(&mut self.px);
        if px.capacity() == 0 || px.capacity() * std::mem::size_of::<Premul>() > SPARE_BYTES { return; }
        let _ = SPARE.try_with(|s| { let mut s = s.borrow_mut(); if s.len() < SPARE_MAX { s.push(px); } });
    }
}

/// How much each pixel of a block lets through (clipping), 0..1.
type Mask<'a> = Option<&'a [f32]>;

/// The context a node is drawn in.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub pick:   Pick,
    pub ghosts: &'a [(u64, Vec<Ghost>)],
    pub hidden: &'a [u64],
}

impl Ctx<'_> {
    fn ghosts_of(&self, id: u64) -> Option<&[Ghost]> { ghosts_in(self.ghosts, id) }
}

/// A part of `rect` on a grid of `side` px: the whole grid cell, its part
/// of `rect`, and that part's rows in the output.
pub(crate) type Part<'a> = (Rect, Rect, Vec<&'a mut [Rgba8]>);

/// `out` (the rows of `rect`) cut along a grid of `side` px (fixed: the
/// same pixels whatever rect asks for them), each part with its own rows,
/// so the parts are written in place on all cores.
pub(crate) fn grid_parts(out: &mut [Rgba8], rect: Rect, side: i32) -> Vec<Part<'_>> {
    let g = rect.snapped_out(side as u32);
    let cols: Vec<(i32, i32, i32)> = (g.x0..g.x1).step_by(side as usize).map(|x| (x, x.max(rect.x0), (x + side).min(rect.x1))).filter(|c| c.1 < c.2).collect();
    let mut parts: Vec<Part> = Vec::new();
    let mut rows = out.chunks_mut(rect.w().max(1) as usize);
    for by in (g.y0..g.y1).step_by(side as usize) {
        let (y0, y1) = (by.max(rect.y0), (by + side).min(rect.y1));
        if y0 >= y1 { continue; }
        let first = parts.len();
        for &(cx, x0, x1) in &cols { parts.push((Rect::new(cx, by, cx + side, by + side), Rect::new(x0, y0, x1, y1), Vec::with_capacity((y1 - y0) as usize))); }
        for _ in y0..y1 {
            let Some(mut row) = rows.next() else { return parts };
            for (k, &(_, x0, x1)) in cols.iter().enumerate() {
                let (seg, rest) = std::mem::take(&mut row).split_at_mut((x1 - x0) as usize);
                row = rest;
                parts[first + k].2.push(seg);
            }
        }
    }
    parts
}

/// Rows of `p` from block `b` (premultiplied) into `rows`, straight.
pub(crate) fn write_rows(b: &Block, p: Rect, rows: &mut [&mut [Rgba8]]) {
    for (k, row) in rows.iter_mut().enumerate() {
        let o = (p.y0 + k as i32 - b.rect.y0) as usize * b.w + (p.x0 - b.rect.x0) as usize;
        let n = row.len();
        for (d, q) in row.iter_mut().zip(&b.px[o..o + n]) { *d = to_straight_u8(*q); }
    }
}

/// The tree has something only the general path handles: a visible
/// filter layer, or a clipped node with a base.
pub fn needs_general(stack: &LayerStack) -> bool {
    fn walk(nodes: &[Node]) -> bool {
        nodes.iter().enumerate().any(|(i, n)| n.visible() && match n {
            Node::Filter(_) => true,
            _ if n.props().clip && i > 0 => true,
            Node::Folder(f) => walk(&f.children),
            _ => false,
        })
    }
    walk(&stack.root)
}

/// How far the visible filter layers `pick` takes read around a pixel,
/// stacked (each reads the result of the ones under it).
pub fn margin(nodes: &[Node], pick: Pick) -> u32 {
    nodes.iter().map(|n| {
        if !n.visible() || (pick.skip_drafts && n.props().draft) { return 0; }
        match n {
            Node::Filter(f) => f.filter.reach(),
            Node::Folder(f) => margin(&f.children, pick),
            _ => 0,
        }
    }).sum::<u32>().min(MAX_MARGIN)
}

/// The merge of `rect`, straight RGBA rows.
pub fn merge(stack: &LayerStack, rect: Rect, opts: Opts) -> Vec<Rgba8> {
    let mut out = vec![[0u8; 4]; rect.area() as usize];
    merge_into(stack, rect, opts, &mut out);
    out
}

/// The merge of `rect` into `out` (rows of `rect`).
pub fn merge_into(stack: &LayerStack, rect: Rect, opts: Opts, out: &mut [Rgba8]) {
    if rect.is_empty() || out.len() != rect.area() as usize { return; }
    merge_parts(stack, grid_parts(out, rect, part_side(stack, opts)), opts);
}

/// The merge of each of `rects` (all their pieces on all cores at once:
/// many small rects keep every core busy).
pub fn merge_rects(stack: &LayerStack, rects: &[Rect], opts: Opts) -> Vec<Vec<Rgba8>> {
    let mut outs: Vec<Vec<Rgba8>> = rects.iter().map(|r| vec![[0u8; 4]; r.area() as usize]).collect();
    let side = part_side(stack, opts);
    let parts: Vec<Part> = outs.iter_mut().zip(rects).filter(|(_, r)| !r.is_empty()).flat_map(|(out, r)| grid_parts(out, *r, side)).collect();
    merge_parts(stack, parts, opts);
    outs
}

/// Side of the pieces merged: 128 px (half a GPU page: the pieces of a
/// page are the page's), larger when filter layers read far around (each
/// piece reads its margin once).
fn part_side(stack: &LayerStack, opts: Opts) -> i32 {
    let m = margin(&stack.root, opts.pick);
    (m * 4).clamp(128, 2048).next_power_of_two() as i32
}

fn merge_parts(stack: &LayerStack, parts: Vec<Part>, opts: Opts) {
    aurastudio_core::zone!("merge");
    let m = margin(&stack.root, opts.pick) as i32;
    let ghosts: &[(u64, Vec<Ghost>)] = if opts.ghosts { stack.ghosts() } else { &[] };
    let ctx = Ctx { pick: opts.pick, ghosts, hidden: opts.hidden };
    let start = match opts.backdrop { Backdrop::White => [1.0; 4], Backdrop::Transparent => [0.0; 4] };
    parts.into_par_iter().for_each(|(_, p, mut rows)| {
        let mut b = Block::new(p.grown(m), start);
        let mut stop = false;
        compose(&stack.root, &mut b, 1.0, ctx, false, None, &mut stop, true);
        write_rows(&b, p, &mut rows);
    });
}

/// `n` is the reference `pick` looks at.
fn is_reference(n: &Node, pick: Pick) -> bool { pick.reference == Some(n.id()) }

/// Whether `n` is drawn (it, or for a folder its content).
fn taken(n: &Node, ctx: Ctx, in_reference: bool) -> bool {
    let pick = ctx.pick;
    if matches!(n, Node::Mask(_)) || !n.visible() || n.opacity() <= 0.0 || n.id() == pick.skip || ctx.hidden.binary_search(&n.id()).is_ok() { return false; }
    if pick.skip_drafts && n.props().draft { return false; }
    if pick.reference.is_some() && !in_reference && !is_reference(n, pick) && !n.is_folder() { return false; }
    true
}

/// A normal, full-opacity paint layer covering the whole block with
/// opaque pixels: nothing under it shows.
fn occludes(n: &Node, b: &Block) -> bool {
    match n {
        Node::Layer(l) => l.visible && l.opacity >= 1.0 && l.blend == BlendMode::Normal && !l.props.clip && l.surface.opaque_in(b.rect),
        _ => false,
    }
}

/// Merge `nodes` (bottom to top) onto `b`. `scale`: the opacity of the
/// pass-through folders around; `in_reference`: inside a reference
/// folder; `mask`: the clipping around them; `stop`: set once
/// `pick.until` is reached (nothing more drawn); `top`: `b` holds only
/// the backdrop so far (an occluder may skip what is under it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose(nodes: &[Node], b: &mut Block, scale: f32, ctx: Ctx, in_reference: bool, mask: Mask, stop: &mut bool, top: bool) {
    let pick = ctx.pick;
    let mut i = 0;
    // Occlusion: start at the topmost opaque cover (whole merges only).
    if top && mask.is_none() && scale >= 1.0 && pick.reference.is_none() && pick.until == 0 {
        if let Some(k) = (0..nodes.len()).rev().find(|&k| taken(&nodes[k], ctx, in_reference) && occludes(&nodes[k], b) && nodes.get(k + 1).is_none_or(|n| !n.props().clip)) {
            i = k;
            aurastudio_core::count!("merge occluded nodes", k);
        }
    }
    while i < nodes.len() && !*stop {
        if nodes[i].id() == pick.until { *stop = true; return; }
        // A base and the clipped nodes over it.
        let base = &nodes[i];
        let mut j = i + 1;
        while j < nodes.len() && nodes[j].props().clip { j += 1; }
        let clipped = &nodes[i + 1..j];
        let base_ref = in_reference || is_reference(base, pick);
        let is_filter = matches!(base, Node::Filter(_));
        let wanted = !is_filter && clipped.iter().any(|c| taken(c, ctx, base_ref));
        if !taken(base, ctx, in_reference) {
            // A hidden base hides its clipped nodes; left out by a reference
            // pick, the reference may be among them.
            if pick.reference.is_some() && !in_reference && base.visible() {
                for c in clipped { if taken(c, ctx, false) { draw(c, b, scale, ctx, false, mask, false, stop); } }
            }
        } else if !wanted {
            draw(base, b, scale, ctx, in_reference, mask, false, stop);
            if is_filter {
                // Clipped to a filter layer: nothing to clip to.
                for c in clipped {
                    if *stop || c.id() == pick.until { *stop = true; return; }
                    if taken(c, ctx, in_reference) { draw(c, b, scale, ctx, in_reference, mask, false, stop); }
                }
            }
        } else {
            // The group: the base alone (full opacity, normal), then the
            // clipped nodes through its coverage.
            let mut g = Block::new(b.rect, [0.0; 4]);
            draw(base, &mut g, 1.0, ctx, in_reference, None, true, stop);
            let cover: Vec<f32> = g.px.iter().map(|p| p[3]).collect();
            for c in clipped {
                if *stop || c.id() == pick.until { *stop = true; break; }
                if taken(c, ctx, base_ref) { draw(c, &mut g, 1.0, ctx, base_ref, Some(&cover), false, stop); }
            }
            let (op, mode) = match base {
                Node::Layer(l) => (l.opacity, l.blend),
                Node::Folder(f) => (f.opacity, match f.blend { FolderBlend::Isolated(m) => m, FolderBlend::PassThrough => BlendMode::Normal }),
                _ => (1.0, BlendMode::Normal),
            };
            blend_block(b, &g.px, op * scale, mode, mask);
        }
        i = j;
    }
}

/// The ghosts `gh` of animation folder `f` onto `b`, far to near: each
/// drawing merged on its own, tinted, faded.
pub(crate) fn ghost_block(f: &Folder, gh: &[Ghost], b: &mut Block, scale: f32, ctx: Ctx) {
    for gst in gh {
        let Some(Node::Folder(sheet)) = f.children.iter().find(|c| c.id() == gst.sheet) else { continue };
        let mut t = Block::new(b.rect, [0.0; 4]);
        let mut stop = false;
        compose(&sheet.children, &mut t, 1.0, Ctx { ghosts: &[], ..ctx }, false, None, &mut stop, false);
        let op = gst.opacity * scale * sheet.opacity;
        for (d, p) in b.px.iter_mut().zip(&t.px) {
            if p[3] <= 0.0 { continue; }
            let cs = gst.tinted([p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
            blend_over(d, cs, p[3].min(1.0) * op, BlendMode::Normal);
        }
    }
}

/// Blend premultiplied pixels `g` onto `b` at `op`, through `mask`.
fn blend_block(b: &mut Block, g: &[Premul], op: f32, mode: BlendMode, mask: Mask) {
    for (k, (d, p)) in b.px.iter_mut().zip(g).enumerate() {
        if p[3] <= 0.0 { continue; }
        let m = mask.map_or(1.0, |m| m[k]);
        let cs = [p[0] / p[3], p[1] / p[3], p[2] / p[3]];
        blend_over(d, cs, p[3].min(1.0) * op * m, mode);
    }
}

/// One straight pixel at alpha `a` (0..1 with opacity) onto `d`.
#[inline(always)]
fn put(d: &mut Premul, s: Rgba8, a: f32, mode: BlendMode) {
    use aurastudio_core::math::from_u8;
    if mode == BlendMode::Normal && a >= 1.0 {
        *d = [from_u8(s[0]), from_u8(s[1]), from_u8(s[2]), 1.0];
        return;
    }
    blend_over(d, [from_u8(s[0]), from_u8(s[1]), from_u8(s[2])], a, mode);
}

/// A paint layer onto `b`.
fn draw_layer(l: &aurastudio_layers::Layer, b: &mut Block, op: f32, blend: BlendMode, mask: Mask) {
    let fill = l.surface.fill();
    if fill[3] == 0 && l.surface.is_fill_in(b.rect) { return; }
    let (x0, w) = (b.rect.x0, b.w);
    for (row, y) in (b.rect.y0..b.rect.y1).enumerate() {
        let line = &mut b.px[row * w..(row + 1) * w];
        let mrow = mask.map(|m| &m[row * w..(row + 1) * w]);
        l.surface.row(y, b.rect.x0, b.rect.x1, |x, run| {
            let o = (x - x0) as usize;
            let dst = &mut line[o..o + run.len()];
            match run {
                Run::Uniform(c, _) if c[3] == 0 => {}
                Run::Uniform(c, _) => {
                    let a = aurastudio_core::math::from_u8(c[3]) * op;
                    for (k, d) in dst.iter_mut().enumerate() {
                        let m = mrow.map_or(1.0, |m| m[o + k]);
                        if m > 0.0 { put(d, c, a * m, blend); }
                    }
                }
                run => for (k, d) in dst.iter_mut().enumerate() {
                    let s = run.at(k);
                    if s[3] == 0 { continue; }
                    let m = mrow.map_or(1.0, |m| m[o + k]);
                    if m > 0.0 { put(d, s, aurastudio_core::math::from_u8(s[3]) * op * m, blend); }
                },
            }
        });
    }
}

/// An isolated folder's content merged on its own over `rect` (from the
/// folder cache when its content is unchanged).
fn folder_alone(f: &Folder, rect: Rect, ctx: Ctx, is_ref: bool, stop: &mut bool) -> Block {
    let mut t = Block::new(rect, [0.0; 4]);
    if let Some(gh) = ctx.ghosts_of(f.id) { ghost_block(f, gh, &mut t, 1.0, ctx); }
    compose(&f.children, &mut t, 1.0, ctx, is_ref, None, stop, false);
    t
}

/// Draw `n` onto `b`, through `mask` if clipped; `alone`: as the base of
/// a clipping group (full opacity, normal blending: the group gets them).
#[allow(clippy::too_many_arguments)]
fn draw(n: &Node, b: &mut Block, scale: f32, ctx: Ctx, in_reference: bool, mask: Mask, alone: bool, stop: &mut bool) {
    let pick = ctx.pick;
    let is_ref = in_reference || is_reference(n, pick);
    match n {
        Node::Layer(l) => {
            if pick.reference.is_some() && !is_ref { return; }
            let op = if alone { 1.0 } else { l.opacity * scale };
            let blend = if alone { BlendMode::Normal } else { l.blend };
            draw_layer(l, b, op, blend, mask);
        }
        Node::Folder(f) => match (f.blend, mask, alone) {
            (FolderBlend::PassThrough, None, false) => {
                if let Some(gh) = ctx.ghosts_of(f.id) { ghost_block(f, gh, b, scale * f.opacity, ctx); }
                compose(&f.children, b, scale * f.opacity, ctx, is_ref, None, stop, false)
            }
            // Clipped, isolated, or a group's base: merged on its own first.
            (blend, _, _) => {
                let mode = match blend { FolderBlend::Isolated(m) if !alone => m, _ => BlendMode::Normal };
                let t = folder_alone(f, b.rect, ctx, is_ref, stop);
                let op = if alone { 1.0 } else { f.opacity * scale };
                blend_block(b, &t.px, op, mode, mask);
            }
        },
        Node::Filter(fl) => {
            if pick.reference.is_some() && !is_ref { return; }
            let op = if alone { 1.0 } else { fl.opacity * scale };
            // The filter works on straight 8-bit pixels, as on a layer.
            let src: Vec<Rgba8> = b.px.iter().map(|p| to_straight_u8(*p)).collect();
            let rows = b.px.len() / b.w.max(1);
            let out = fl.filter.apply(&src, b.w, rows);
            for (k, (d, o)) in b.px.iter_mut().zip(out).enumerate() {
                let t = (op * mask.map_or(1.0, |m| m[k])).min(1.0);
                if t <= 0.0 { continue; }
                let f = aurastudio_core::blend::to_premul(o);
                for c in 0..4 { d[c] += (f[c] - d[c]) * t; }
            }
        }
        Node::Mask(_) => {}
    }
}

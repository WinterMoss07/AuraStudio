//! During a stroke only one layer changes: the merges under and over it
//! are kept per cell of the canvas (Krita), three blends per pixel instead
//! of every layer. Over it, only normal blending can be pre-merged. Cells
//! are keyed by the content of every other node there: an edit elsewhere
//! (visibility, another layer) invalidates exactly the cells it touches.

use std::collections::HashMap;
use std::sync::Arc;

use aurastudio_core::blend::{blend_over, to_straight_u8, Premul};
use aurastudio_core::{BlendMode, Fnv, Rect, Rgba8};
use aurastudio_layers::{Folder, FolderBlend, Ghost, LayerStack, Node, Pick};
use rayon::prelude::*;

use crate::merge::{compose, needs_general, Block, Ctx};

/// Cell side, px.
const CELL: i32 = 64;

/// What the merge draws, in order: a node with the opacity of the
/// pass-through folders around it, or an animation folder's ghosts.
#[derive(Clone, Copy)]
enum Item<'a> {
    Node(&'a Node, f32),
    Ghosts(&'a Folder, f32, &'a [Ghost]),
}

/// Visible nodes in drawing order, pass-through folders opened (their
/// opacity carried down), isolated folders kept whole.
fn flatten<'a>(nodes: &'a [Node], scale: f32, ghosts: &'a [(u64, Vec<Ghost>)], out: &mut Vec<Item<'a>>) {
    for n in nodes {
        if matches!(n, Node::Mask(_)) || !n.visible() || n.opacity() <= 0.0 { continue; }
        match n {
            Node::Folder(f) if f.blend == FolderBlend::PassThrough => {
                if let Some(gh) = ghosts.iter().find(|g| g.0 == f.id).map(|g| &g.1[..]).filter(|g| !g.is_empty()) { out.push(Item::Ghosts(f, scale * f.opacity, gh)); }
                flatten(&f.children, scale * f.opacity, ghosts, out);
            }
            _ => out.push(Item::Node(n, scale)),
        }
    }
}

fn normal(n: &Node) -> bool {
    match n {
        Node::Layer(l) => l.blend == BlendMode::Normal,
        Node::Folder(f) => f.blend == FolderBlend::Isolated(BlendMode::Normal),
        _ => false,
    }
}

struct Cell {
    key:   u64,
    below: Arc<Vec<Premul>>,
    /// `None`: nothing covers the cell from above.
    above: Option<Arc<Vec<Premul>>>,
}

/// The under / over merges of the layer being painted, by cell.
#[derive(Default)]
pub struct StrokeCache {
    layer: u64,
    cells: HashMap<(i32, i32), Cell>,
}

fn draw_items(items: &[Item], b: &mut Block, ctx: Ctx) {
    let mut stop = false;
    for it in items {
        match *it {
            Item::Node(n, s) => draw_scaled(n, b, s, ctx, &mut stop),
            Item::Ghosts(f, s, gh) => {
                for g in gh {
                    let Some(Node::Folder(sheet)) = f.children.iter().find(|c| c.id() == g.sheet) else { continue };
                    let mut t = Block::new(b.rect, [0.0; 4]);
                    let mut st = false;
                    compose(&sheet.children, &mut t, 1.0, Ctx { ghosts: &[], ..ctx }, false, None, &mut st, false);
                    let op = g.opacity * s * sheet.opacity;
                    for (d, p) in b.px.iter_mut().zip(&t.px) {
                        if p[3] <= 0.0 { continue; }
                        let cs = g.tinted([p[0] / p[3], p[1] / p[3], p[2] / p[3]]);
                        blend_over(d, cs, p[3].min(1.0) * op, BlendMode::Normal);
                    }
                }
            }
        }
    }
}

/// One node at the scale of the pass-through folders around it.
fn draw_scaled(n: &Node, b: &mut Block, scale: f32, ctx: Ctx, stop: &mut bool) {
    // (compose of a one-node list draws it as it would be among others.)
    compose(std::slice::from_ref(n), b, scale, ctx, false, None, stop, false);
}

impl StrokeCache {
    /// The merge of `rect` while layer `layer` is painted: the full merge's
    /// pixels (to rounding), from the cells when the tree allows it.
    pub fn merge(&mut self, stack: &LayerStack, layer: u64, rect: Rect) -> Vec<Rgba8> {
        let opts = crate::Opts { ghosts: true, ..Default::default() };
        if rect.is_empty() { return Vec::new(); }
        aurastudio_core::zone!("merge stroke");
        if layer != self.layer { self.cells.clear(); self.layer = layer; }
        let ghosts = stack.ghosts();
        let mut items = Vec::new();
        flatten(&stack.root, 1.0, ghosts, &mut items);
        let Some(k) = items.iter().position(|it| matches!(it, Item::Node(n, _) if n.id() == layer)) else { return crate::merge(stack, rect, opts) };
        let usable = !needs_general(stack) && items[k + 1..].iter().all(|it| match it { Item::Node(n, _) => normal(n), Item::Ghosts(..) => true });
        if !usable { return crate::merge(stack, rect, opts); }
        let (Item::Node(painted, scale), below_items, above_items) = (items[k], &items[..k], &items[k + 1..]) else { return Vec::new() };
        let ctx = Ctx { pick: Pick::ALL, ghosts, cache: None };
        // The settings of everything but the painted layer's pixels.
        let mut sh = Fnv::default();
        for it in &items {
            match *it {
                Item::Node(n, s) => { sh.u64(n.id()).f32(s); if n.id() == layer { sh.u32(1); } }
                Item::Ghosts(f, s, gh) => { sh.u64(f.id).f32(s); for g in gh { sh.u64(g.sheet).f32(g.opacity); for t in g.tint { sh.f32(t); } } }
            }
        }
        let settings = sh.finish();
        let key = |c: Rect| {
            let mut h = Fnv::default();
            h.u64(settings);
            for it in &items {
                match *it {
                    Item::Node(n, _) if n.id() == layer => {}
                    Item::Node(n, _) => crate::key::node_key(&mut h, n, c),
                    Item::Ghosts(f, _, gh) => for g in gh { if let Some(s) = f.children.iter().find(|x| x.id() == g.sheet) { crate::key::node_key(&mut h, s, c); } },
                }
            }
            h.finish()
        };
        let g = rect.snapped_out(CELL as u32);
        let cells: Vec<(i32, i32)> = (g.y0..g.y1).step_by(CELL as usize).flat_map(|y| (g.x0..g.x1).step_by(CELL as usize).map(move |x| (x, y))).collect();
        // Cells to (re)build: none yet, or their content changed.
        let todo: Vec<((i32, i32), u64)> = cells.iter().map(|&c| (c, key(Rect::new(c.0, c.1, c.0 + CELL, c.1 + CELL))))
            .filter(|(c, k)| self.cells.get(c).is_none_or(|e| e.key != *k)).collect();
        aurastudio_core::count!("stroke cache cells built", todo.len());
        let built: Vec<((i32, i32), Cell)> = todo.par_iter().map(|&((x, y), k)| {
            let r = Rect::new(x, y, x + CELL, y + CELL);
            let mut below = Block::new(r, [1.0; 4]);
            draw_items(below_items, &mut below, ctx);
            let above = (!above_items.is_empty()).then(|| {
                let mut a = Block::new(r, [0.0; 4]);
                draw_items(above_items, &mut a, ctx);
                a.px
            }).filter(|px| px.iter().any(|p| p[3] > 0.0));
            ((x, y), Cell { key: k, below: Arc::new(below.px), above: above.map(Arc::new) })
        }).collect();
        for (c, e) in built { self.cells.insert(c, e); }
        // Below, then the painted layer, then what is over it.
        let cells_ref = &self.cells;
        let rows: Vec<(Rect, Vec<Rgba8>)> = cells.par_iter().filter_map(|&(x, y)| {
            let cr = Rect::new(x, y, x + CELL, y + CELL);
            let part = cr.intersect(&rect);
            let e = cells_ref.get(&(x, y))?;
            let mut b = Block { rect: cr, w: CELL as usize, px: (*e.below).clone() };
            let mut stop = false;
            draw_scaled(painted, &mut b, scale, ctx, &mut stop);
            if let Some(a) = &e.above {
                for (d, p) in b.px.iter_mut().zip(a.iter()) {
                    if p[3] <= 0.0 { continue; }
                    let k = 1.0 - p[3].min(1.0);
                    for c in 0..4 { d[c] = p[c] + d[c] * k; }
                }
            }
            let mut out = Vec::with_capacity(part.area() as usize);
            for yy in part.y0..part.y1 {
                let o = (yy - cr.y0) as usize * CELL as usize + (part.x0 - cr.x0) as usize;
                out.extend(b.px[o..o + part.w() as usize].iter().map(|q| to_straight_u8(*q)));
            }
            Some((part, out))
        }).collect();
        let rw = rect.w() as usize;
        let mut out = vec![[0u8; 4]; rect.area() as usize];
        for (p, px) in rows {
            let pw = p.w() as usize;
            for (k, y) in (p.y0..p.y1).enumerate() {
                let o = (y - rect.y0) as usize * rw + (p.x0 - rect.x0) as usize;
                out[o..o + pw].copy_from_slice(&px[k * pw..(k + 1) * pw]);
            }
        }
        out
    }

    /// Cells held.
    pub fn len(&self) -> usize { self.cells.len() }
    pub fn is_empty(&self) -> bool { self.cells.is_empty() }
    pub fn clear(&mut self) { self.cells.clear(); }
}

//! During a stroke only one layer changes: the merges under and over it
//! are kept per cell of the canvas (Krita), three blends per pixel instead
//! of every layer. Over it, only normal blending can be pre-merged. Cells
//! are keyed by the content of every other node there: an edit elsewhere
//! (visibility, another layer) invalidates exactly the cells it touches.
//! With fewer than `MIN_OTHERS` layers around the painted one the merge is
//! direct: it costs about the same (measured: 0.45 against 0.3 ms for
//! 128 × 128 with two layers under) and keeps no cells.

use std::collections::HashMap;
use std::sync::Arc;

use aurastudio_core::blend::Premul;
use aurastudio_core::{BlendMode, Fnv, Rect, Rgba8};
use aurastudio_layers::{ghosts_in, Folder, FolderBlend, Ghost, LayerStack, Node, Pick};
use rayon::prelude::*;

use crate::merge::{compose, ghost_block, grid_parts, needs_general, write_rows, Block, Ctx};

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
                if let Some(gh) = ghosts_in(ghosts, f.id) { out.push(Item::Ghosts(f, scale * f.opacity, gh)); }
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

/// Layers (ghosts counted one by one) other than the painted one from
/// which the cells are worth keeping.
const MIN_OTHERS: usize = 3;

/// Bytes the cells may hold; past it, those used longest ago go.
const BUDGET: usize = 96 << 20;

/// A premultiplied pixel at 16 bits a channel (0..=65535 for 0..1): the
/// merge's precision kept to well under a level of 8 bits, at half the
/// size of f32.
type Px16 = [u16; 4];

#[inline]
fn pack(p: Premul) -> Px16 { p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16) }

#[inline]
fn unpack(p: Px16) -> Premul { p.map(|v| v as f32 * (1.0 / 65535.0)) }

/// A cell's pixels: one color (paper, empty) or all of them.
#[derive(Clone)]
enum Pixels {
    Uniform(Px16),
    Each(Arc<Vec<Px16>>),
}

impl Pixels {
    fn of(px: &[Premul]) -> Self {
        let first = px.first().copied().map(pack).unwrap_or([0; 4]);
        if px.iter().all(|p| pack(*p) == first) { return Pixels::Uniform(first); }
        Pixels::Each(Arc::new(px.iter().map(|p| pack(*p)).collect()))
    }
    fn bytes(&self) -> usize { match self { Pixels::Uniform(_) => 8, Pixels::Each(v) => v.len() * 8 } }
    fn get(&self, i: usize) -> Px16 { match self { Pixels::Uniform(c) => *c, Pixels::Each(v) => v[i] } }
}

struct Cell {
    key:   u64,
    below: Pixels,
    /// `None`: nothing covers the cell from above.
    above: Option<Pixels>,
    /// The merge that last used it (the budget drops the oldest).
    used:  u64,
}

impl Cell {
    fn bytes(&self) -> usize { self.below.bytes() + self.above.as_ref().map_or(0, Pixels::bytes) }
}

/// The under / over merges of the layer being painted, by cell.
#[derive(Default)]
pub struct StrokeCache {
    layer: u64,
    cells: HashMap<(i32, i32), Cell>,
    bytes: usize,
    tick:  u64,
}

fn draw_items(items: &[Item], b: &mut Block, ctx: Ctx) {
    let mut stop = false;
    for it in items {
        match *it {
            Item::Node(n, s) => draw_scaled(n, b, s, ctx, &mut stop),
            Item::Ghosts(f, s, gh) => ghost_block(f, gh, b, s, ctx),
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
        if layer != self.layer { self.clear(); self.layer = layer; }
        self.tick += 1;
        let ghosts = stack.ghosts();
        let mut items = Vec::new();
        flatten(&stack.root, 1.0, ghosts, &mut items);
        let Some(k) = items.iter().position(|it| matches!(it, Item::Node(n, _) if n.id() == layer)) else { return crate::merge(stack, rect, opts) };
        let usable = !needs_general(stack) && items[k + 1..].iter().all(|it| match it { Item::Node(n, _) => normal(n), Item::Ghosts(..) => true });
        let others: usize = items.iter().enumerate().filter(|(i, _)| *i != k).map(|(_, it)| match it { Item::Node(..) => 1, Item::Ghosts(_, _, gh) => gh.len() }).sum();
        if !usable || others < MIN_OTHERS { self.clear(); return crate::merge(stack, rect, opts); }
        let (Item::Node(painted, scale), below_items, above_items) = (items[k], &items[..k], &items[k + 1..]) else { return Vec::new() };
        let ctx = Ctx { pick: Pick::ALL, ghosts, hidden: &[] };
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
            ((x, y), Cell { key: k, below: Pixels::of(&below.px), above: above.map(|a| Pixels::of(&a)), used: 0 })
        }).collect();
        for (c, e) in built {
            self.bytes += e.bytes();
            if let Some(old) = self.cells.insert(c, e) { self.bytes -= old.bytes(); }
        }
        for c in &cells { if let Some(e) = self.cells.get_mut(c) { e.used = self.tick; } }
        self.trim(BUDGET);
        // Below, then the painted layer, then what is over it, straight
        // into each cell's rows of the output.
        let cells_ref = &self.cells;
        let mut out = vec![[0u8; 4]; rect.area() as usize];
        grid_parts(&mut out, rect, CELL).into_par_iter().for_each(|(cr, part, mut rows)| {
            let Some(e) = cells_ref.get(&(cr.x0, cr.y0)) else { return };
            let n = (CELL * CELL) as usize;
            let mut b = Block { rect: cr, w: CELL as usize, px: (0..n).map(|i| unpack(e.below.get(i))).collect() };
            let mut stop = false;
            draw_scaled(painted, &mut b, scale, ctx, &mut stop);
            if let Some(a) = &e.above {
                for (i, d) in b.px.iter_mut().enumerate() {
                    let q = a.get(i);
                    if q[3] == 0 { continue; }
                    let p = unpack(q);
                    let k = 1.0 - p[3].min(1.0);
                    for c in 0..4 { d[c] = p[c] + d[c] * k; }
                }
            }
            write_rows(&b, part, &mut rows);
        });
        out
    }

    /// Drop the cells used longest ago until they hold at most `max`
    /// bytes (those of the last merge stay).
    pub fn trim(&mut self, max: usize) {
        if self.bytes <= max { return; }
        let mut by_age: Vec<((i32, i32), u64)> = self.cells.iter().filter(|(_, e)| e.used < self.tick).map(|(c, e)| (*c, e.used)).collect();
        by_age.sort_unstable_by_key(|x| x.1);
        for (c, _) in by_age {
            if self.bytes <= max { break; }
            if let Some(e) = self.cells.remove(&c) { self.bytes -= e.bytes(); }
        }
    }

    /// Cells held, and their bytes (the memory report).
    pub fn len(&self) -> usize { self.cells.len() }
    pub fn bytes(&self) -> usize { self.bytes }
    pub fn is_empty(&self) -> bool { self.cells.is_empty() }
    pub fn clear(&mut self) {
        self.cells = HashMap::new();
        self.bytes = 0;
    }
}

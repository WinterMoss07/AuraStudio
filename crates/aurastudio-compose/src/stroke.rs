//! During a stroke only one layer changes: the merges under and over it
//! are kept per cell of the canvas (Krita), three blends per pixel instead
//! of every layer. Over it, only normal blending can be pre-merged. Cells
//! are keyed by the content of every other node there: an edit elsewhere
//! (visibility, another layer) invalidates exactly the cells it touches.
//! With fewer than `MIN_OTHERS` layers around the painted one the screen's
//! merge is direct: it costs about the same (measured: 0.45 against 0.3 ms
//! for 128 × 128 with two layers under) and keeps no cells.
//!
//! The same cells serve a Mixer Brush that samples all layers (`around`):
//! one cache for the screen and the brush, shared between the threads
//! that merge and paint (a clone is the same cache).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use aurastudio_core::blend::{to_premul, to_straight_u8, Premul};
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
    fn of_u8(px: &[Rgba8]) -> Self {
        let p: Vec<Premul> = px.iter().map(|q| to_premul(*q)).collect();
        Pixels::of(&p)
    }
    fn bytes(&self) -> usize { match self { Pixels::Uniform(_) => 8, Pixels::Each(v) => v.len() * 8 } }
    fn get(&self, i: usize) -> Px16 { match self { Pixels::Uniform(c) => *c, Pixels::Each(v) => v[i] } }
}

struct Cell {
    key:   u64,
    below: Pixels,
    /// `None`: nothing covers the cell from above.
    above: Option<Pixels>,
}

impl Cell {
    fn bytes(&self) -> usize { self.below.bytes() + self.above.as_ref().map_or(0, Pixels::bytes) }
}

#[derive(Default)]
struct Cells {
    layer: u64,
    /// Each cell, and the merge that last used it (the budget drops the
    /// oldest).
    map:   HashMap<(i32, i32), (Arc<Cell>, u64)>,
    bytes: usize,
    tick:  u64,
}

impl Cells {
    /// Drop the cells used longest ago until they hold at most `max`
    /// bytes (those of the last merge stay).
    fn trim(&mut self, max: usize) {
        if self.bytes <= max { return; }
        let mut by_age: Vec<((i32, i32), u64)> = self.map.iter().filter(|(_, e)| e.1 < self.tick).map(|(c, e)| (*c, e.1)).collect();
        by_age.sort_unstable_by_key(|x| x.1);
        for (c, _) in by_age {
            if self.bytes <= max { break; }
            if let Some((e, _)) = self.map.remove(&c) { self.bytes -= e.bytes(); }
        }
    }
}

/// The under / over merges of the layer being painted, by cell. Cloned,
/// the same cache.
#[derive(Clone, Default)]
pub struct StrokeCache {
    cells: Arc<Mutex<Cells>>,
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

/// How the cells of a stroke on `layer` are made.
enum Plan<'a> {
    /// The tree flattened: under the painted layer, it, over it (normal
    /// blending only), with the pass-through scale of the painted one.
    Items { items: Vec<Item<'a>>, k: usize, others: usize, ghosts: &'a [(u64, Vec<Ghost>)] },
    /// Anything else (the painted layer in an isolated folder, a filter,
    /// clipping, blending over it): each cell is the general merge of what
    /// is under it and over it (Mixer sampling only; the screen merges
    /// directly).
    General,
}


fn cell_rect((x, y): (i32, i32)) -> Rect { Rect::new(x, y, x + CELL, y + CELL) }

/// The cells of the grid `rect` touches, row by row.
fn cells_of(rect: Rect) -> Vec<(i32, i32)> {
    let g = rect.snapped_out(CELL as u32);
    (g.y0..g.y1).step_by(CELL as usize).flat_map(|y| (g.x0..g.x1).step_by(CELL as usize).map(move |x| (x, y))).collect()
}

impl<'a> Plan<'a> {
    fn of(stack: &'a LayerStack, layer: u64, ghosts: &'a [(u64, Vec<Ghost>)]) -> Plan<'a> {
        let mut items = Vec::new();
        flatten(&stack.root, 1.0, ghosts, &mut items);
        let Some(k) = items.iter().position(|it| matches!(it, Item::Node(n, _) if n.id() == layer)) else { return Plan::General };
        let usable = !needs_general(stack) && items[k + 1..].iter().all(|it| match it { Item::Node(n, _) => normal(n), Item::Ghosts(..) => true });
        if !usable { return Plan::General; }
        let others: usize = items.iter().enumerate().filter(|(i, _)| *i != k).map(|(_, it)| match it { Item::Node(..) => 1, Item::Ghosts(_, _, gh) => gh.len() }).sum();
        Plan::Items { items, k, others, ghosts }
    }

    /// The content key of cell `c`: everything but the painted layer's
    /// pixels (and how the cell is made).
    fn key(&self, stack: &LayerStack, layer: u64, c: Rect) -> u64 {
        let mut h = Fnv::default();
        match self {
            Plan::Items { items, .. } => {
                h.u32(1);
                for it in items {
                    match *it {
                        Item::Node(n, s) => { h.u64(n.id()).f32(s); if n.id() == layer { h.u32(1); } }
                        Item::Ghosts(f, s, gh) => { h.u64(f.id).f32(s); for g in gh { h.u64(g.sheet).f32(g.opacity); for t in g.tint { h.f32(t); } } }
                    }
                }
                for it in items {
                    match *it {
                        Item::Node(n, _) if n.id() == layer => {}
                        Item::Node(n, _) => crate::key::node_key(&mut h, n, c),
                        Item::Ghosts(f, _, gh) => for g in gh { if let Some(s) = f.children.iter().find(|x| x.id() == g.sheet) { crate::key::node_key(&mut h, s, c); } },
                    }
                }
            }
            Plan::General => {
                h.u32(2).u64(layer);
                for n in &stack.root { crate::key::node_key_except(&mut h, n, c, layer); }
            }
        }
        h.finish()
    }

    /// Cell `c` made (`stack`: with the painted layer empty for the
    /// general plan).
    fn build(&self, stack: &LayerStack, layer: u64, c: (i32, i32), key: u64) -> Option<Cell> {
        let r = cell_rect(c);
        match self {
            Plan::Items { items, k, ghosts, .. } => {
                let ctx = Ctx { pick: Pick::ALL, ghosts, hidden: &[] };
                let (below_items, above_items) = (&items[..*k], &items[k + 1..]);
                let mut below = Block::new(r, [1.0; 4]);
                draw_items(below_items, &mut below, ctx);
                let above = (!above_items.is_empty()).then(|| {
                    let mut a = Block::new(r, [0.0; 4]);
                    draw_items(above_items, &mut a, ctx);
                    a
                }).filter(|a| a.px.iter().any(|p| p[3] > 0.0));
                Some(Cell { key, below: Pixels::of(&below.px), above: above.map(|a| Pixels::of(&a.px)) })
            }
            Plan::General => {
                let (below, above) = crate::merge_around(stack, layer, r)?;
                let above = above.iter().any(|p| p[3] > 0).then(|| Pixels::of_u8(&above));
                Some(Cell { key, below: Pixels::of_u8(&below), above })
            }
        }
    }
}

impl StrokeCache {
    /// The cells for `rects` (made where missing or stale, on all cores),
    /// each looked up once.
    fn cells_for(&self, plan: &Plan, stack: &LayerStack, layer: u64, rects: &[Rect]) -> HashMap<(i32, i32), Arc<Cell>> {
        let mut want: Vec<(i32, i32)> = rects.iter().flat_map(|r| cells_of(*r)).collect();
        want.sort_unstable();
        want.dedup();
        let keys: Vec<((i32, i32), u64)> = want.par_iter().map(|&c| (c, plan.key(stack, layer, cell_rect(c)))).collect();
        let mut got: HashMap<(i32, i32), Arc<Cell>> = HashMap::with_capacity(keys.len());
        let todo: Vec<((i32, i32), u64)> = {
            let mut g = self.lock();
            if layer != g.layer { *g = Cells { layer, ..Cells::default() }; }
            g.tick += 1;
            let tick = g.tick;
            keys.iter().copied().filter(|(c, k)| match g.map.get_mut(c) {
                Some(e) if e.0.key == *k => { e.1 = tick; got.insert(*c, Arc::clone(&e.0)); false }
                _ => true,
            }).collect()
        };
        aurastudio_core::count!("stroke cache cells built", todo.len());
        let built: Vec<((i32, i32), Arc<Cell>)> = todo.par_iter().filter_map(|&(c, k)| plan.build(stack, layer, c, k).map(|e| (c, Arc::new(e)))).collect();
        let mut g = self.lock();
        if g.layer == layer {
            let tick = g.tick;
            for (c, e) in &built {
                g.bytes += e.bytes();
                if let Some((old, _)) = g.map.insert(*c, (Arc::clone(e), tick)) { g.bytes -= old.bytes(); }
            }
            g.trim(BUDGET);
        }
        got.extend(built);
        got
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cells> { self.cells.lock().unwrap_or_else(PoisonError::into_inner) }

    /// The merge of `rect` while layer `layer` is painted: the full merge's
    /// pixels (to rounding), from the cells when the tree allows it.
    pub fn merge(&self, stack: &LayerStack, layer: u64, rect: Rect) -> Vec<Rgba8> {
        self.merge_rects(stack, layer, &[rect]).pop().unwrap_or_default()
    }

    /// `merge` of each of `rects` (all their cells on all cores at once).
    pub fn merge_rects(&self, stack: &LayerStack, layer: u64, rects: &[Rect]) -> Vec<Vec<Rgba8>> {
        let opts = crate::Opts { ghosts: true, ..Default::default() };
        aurastudio_core::zone!("merge stroke");
        let plan = Plan::of(stack, layer, stack.ghosts());
        let (items, k) = match &plan {
            Plan::Items { items, k, others, .. } if *others >= MIN_OTHERS => (items, *k),
            // (Nothing built: cells a Mixer Brush made stay.)
            _ => return crate::merge::merge_rects(stack, rects, opts),
        };
        let Item::Node(painted, scale) = items[k] else { return rects.iter().map(|r| vec![[0; 4]; r.area() as usize]).collect() };
        let ctx = Ctx { pick: Pick::ALL, ghosts: stack.ghosts(), hidden: &[] };
        let cells = self.cells_for(&plan, stack, layer, rects);
        // Below, then the painted layer, then what is over it, straight
        // into each cell's rows of the outputs.
        let mut outs: Vec<Vec<Rgba8>> = rects.iter().map(|r| vec![[0u8; 4]; r.area() as usize]).collect();
        let parts: Vec<_> = outs.iter_mut().zip(rects).flat_map(|(out, r)| grid_parts(out, *r, CELL)).collect();
        parts.into_par_iter().for_each(|(cr, part, mut rows)| {
            let Some(e) = cells.get(&(cr.x0, cr.y0)) else { return };
            let mut b = Block::new(cr, [0.0; 4]);
            match &e.below {
                Pixels::Uniform(c) => b.px.fill(unpack(*c)),
                Pixels::Each(v) => for (d, q) in b.px.iter_mut().zip(v.iter()) { *d = unpack(*q); },
            }
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
        outs
    }

    /// What a Mixer Brush that samples all layers sees around `layer` over
    /// `rect`: the merge under it (on white) and over it (on
    /// transparency), straight RGBA rows. `stack`: the stack as the stroke
    /// began, the painted layer empty in it. From the same cells as the
    /// screen's merge (no onion ghosts: they are not paint).
    pub fn around(&self, stack: &LayerStack, layer: u64, rect: Rect) -> (Vec<Rgba8>, Vec<Rgba8>) {
        let n = rect.area() as usize;
        let (mut under, mut over) = (vec![[0u8; 4]; n], vec![[0u8; 4]; n]);
        let inside = rect.intersect(&stack.frame());
        if inside.is_empty() { return (under, over); }
        let plan = Plan::of(stack, layer, &[]);
        let cells = self.cells_for(&plan, stack, layer, &[inside]);
        let w = rect.w() as usize;
        for (&(cx, cy), e) in &cells {
            let part = cell_rect((cx, cy)).intersect(&inside);
            for y in part.y0..part.y1 {
                for x in part.x0..part.x1 {
                    let i = ((y - cy) * CELL + (x - cx)) as usize;
                    let o = (y - rect.y0) as usize * w + (x - rect.x0) as usize;
                    under[o] = to_straight_u8(unpack(e.below.get(i)));
                    if let Some(a) = &e.above { over[o] = to_straight_u8(unpack(a.get(i))); }
                }
            }
        }
        (under, over)
    }

    /// Drop the cells used longest ago until they hold at most `max`
    /// bytes (those of the last merge stay).
    pub fn trim(&self, max: usize) { self.lock().trim(max); }

    /// Cells held, and their bytes (the memory report).
    pub fn len(&self) -> usize { self.lock().map.len() }
    pub fn bytes(&self) -> usize { self.lock().bytes }
    pub fn is_empty(&self) -> bool { self.lock().map.is_empty() }
    /// Everything dropped (as new), for every clone.
    pub fn clear(&self) { *self.lock() = Cells::default(); }
}

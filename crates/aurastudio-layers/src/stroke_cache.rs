//! During a stroke only one layer changes: the merges under and over it
//! are kept per tile (Krita), three blends per pixel instead of every
//! layer. Over it, only normal blending can be pre-merged.

use aurastudio_core::{Rect, TILE};

use crate::compose::Ghosts;
use crate::onion::{ghosts_of, Ghost};
use crate::{composite_row, composite_row_opaque, ghost_row, ghost_row_opaque, BlendMode, Folder, FolderBlend, LayerStack, Node, Tile};

/// Per tile, built the first time the stroke's display needs it.
#[derive(Default)]
pub(crate) struct StrokeCache {
    /// Some(true / false) once worked out: can this stroke use the cache?
    usable: Option<bool>,
    /// Opaque merge of the layers under the painted one.
    below:  Vec<Option<Tile>>,
    /// Premultiplied merge of the layers over it (alpha: how much they
    /// cover); None where nothing covers the tile (set with `below`).
    above:  Vec<Option<Tile>>,
}

/// What the merge draws, in order: a node, or the onion skin's ghosts of
/// an animation folder (drawn before its sheets).
#[derive(Clone, Copy)]
enum Item<'a> {
    Node(&'a Node, f32),
    Ghosts(&'a Folder, f32, &'a [Ghost]),
}

/// Visible nodes in drawing order, pass-through folders opened (their
/// opacity carried down), isolated folders kept whole.
fn flatten<'a>(nodes: &'a [Node], scale: f32, ghosts: Ghosts<'a>, out: &mut Vec<Item<'a>>) {
    for n in nodes {
        if !n.visible() || n.opacity() <= 0.0 { continue; }
        match n {
            Node::Folder(f) if f.blend == FolderBlend::PassThrough => {
                if let Some(gh) = ghosts_of(ghosts, f.id) { out.push(Item::Ghosts(f, scale * f.opacity, gh)); }
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
        Node::Filter(_) => false,
    }
}

impl LayerStack {
    /// Another layer changed during a stroke (visibility, opacity... from
    /// the layer panel): the merges under / over are stale.
    pub fn drop_stroke_cache(&mut self) {
        self.stroke.cache = StrokeCache::default();
    }

    /// The merge of `rect` while a stroke is painted (composite_rect's
    /// result, from the cache when it can).
    #[profiling::function]
    pub fn composite_rect_stroke(&mut self, rect: Rect) -> Vec<u8> {
        let r = (rect.0, rect.1, rect.2.min(self.width), rect.3.min(self.height));
        if r.0 >= r.2 || r.1 >= r.3 { return Vec::new(); }
        let id = self.stroke.layer;
        if self.stroke.base.is_empty() { return self.composite_rect(r); }

        let ghosts = &self.onion.ghosts[..];
        let mut items = Vec::new();
        flatten(&self.root, 1.0, ghosts, &mut items);
        let Some(k) = items.iter().position(|it| matches!(it, Item::Node(n, _) if n.id() == id)) else { return self.composite_rect(r) };
        // (Clipping and filter layers: the general merge, no cache.)
        let general = self.needs_general_merge();
        let usable = *self.stroke.cache.usable.get_or_insert_with(|| !general && items[k + 1..].iter().all(|it| match it { Item::Node(n, _) => normal(n), Item::Ghosts(..) => true }));
        if !usable { return self.composite_rect(r); }

        let tiles = self.stroke.base.len();
        let cache = &mut self.stroke.cache;
        if cache.below.len() != tiles {
            cache.below = (0..tiles).map(|_| None).collect();
            cache.above = (0..tiles).map(|_| None).collect();
        }
        let (w, h, cols) = (self.width, self.height, self.width.div_ceil(TILE));
        let Item::Node(layer, scale) = items[k] else { unreachable!() };
        let (below_items, above_items) = (&items[..k], &items[k + 1..]);
        let mut group = Vec::new();

        // Fill the caches of the tiles this rectangle touches.
        for ty in r.1 / TILE..r.3.div_ceil(TILE) {
            for tx in r.0 / TILE..r.2.div_ceil(TILE) {
                let i = (ty * cols + tx) as usize;
                if cache.below[i].is_some() { continue; }
                let (x0, y0) = (tx * TILE, ty * TILE);
                let (tw, th) = ((w - x0).min(TILE) as usize, (h - y0).min(TILE));
                let mut below = crate::new_tile([255, 255, 255, 255]);
                // (Nothing over the painted layer: no tile to make.)
                let mut above = (!above_items.is_empty()).then(|| crate::new_tile([0, 0, 0, 0]));
                let mut prow = vec![[0.0f32; 4]; if above.is_some() { tw } else { 0 }];
                let mut covered = false;
                for y in 0..th {
                    let o = (y * TILE * 4) as usize;
                    let dst = &mut below[o..o + tw * 4];
                    for it in below_items {
                        match *it {
                            Item::Node(n, s) => composite_row_opaque(std::slice::from_ref(n), y0 + y, x0, dst, s, &mut group, ghosts),
                            Item::Ghosts(f, s, gh) => ghost_row_opaque(f, gh, y0 + y, x0, dst, s),
                        }
                    }
                    let Some(above) = above.as_mut() else { continue };
                    prow.iter_mut().for_each(|p| *p = [0.0; 4]);
                    for it in above_items {
                        match *it {
                            Item::Node(n, s) => composite_row(std::slice::from_ref(n), y0 + y, x0, &mut prow, s, ghosts),
                            Item::Ghosts(f, s, gh) => ghost_row(f, gh, y0 + y, x0, &mut prow, s),
                        }
                    }
                    for (d, p) in above[o..o + tw * 4].chunks_exact_mut(4).zip(&prow) {
                        for c in 0..4 { d[c] = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8; }
                        covered |= d[3] != 0;
                    }
                }
                cache.below[i] = Some(below);
                cache.above[i] = above.filter(|_| covered);
            }
        }

        // Below, then the painted layer, then what is over it.
        let n = (r.2 - r.0) as usize;
        let mut out = vec![0u8; n * (r.3 - r.1) as usize * 4];
        for (y, row) in (r.1..r.3).zip(out.chunks_exact_mut(n * 4)) {
            let mut x = r.0;
            while x < r.2 {
                let (tx, ty) = (x / TILE, y / TILE);
                let end = ((tx + 1) * TILE).min(r.2);
                let i = (ty * cols + tx) as usize;
                let o = (((y % TILE) * TILE + x % TILE) * 4) as usize;
                let len = (end - x) as usize * 4;
                let seg = &mut row[(x - r.0) as usize * 4..][..len];
                seg.copy_from_slice(&cache.below[i].as_ref().unwrap()[o..o + len]);
                composite_row_opaque(std::slice::from_ref(layer), y, x, seg, scale, &mut group, ghosts);
                if let Some(above) = &cache.above[i] {
                    // d = d × (1 - A) + C over every byte (vectorizes; the
                    // alpha byte gets A = 0, C = 0: stays 255).
                    let above = &above[o..o + len];
                    let mut k4 = [0u8; TILE as usize * 4];
                    for (k, a) in k4[..len].chunks_exact_mut(4).zip(above.chunks_exact(4)) {
                        let k1 = 255 - a[3];
                        k.copy_from_slice(&[k1, k1, k1, 255]);
                    }
                    let mut c4 = [0u8; TILE as usize * 4];
                    for (c, a) in c4[..len].chunks_exact_mut(4).zip(above.chunks_exact(4)) {
                        c.copy_from_slice(&[a[0], a[1], a[2], 0]);
                    }
                    for ((d, &k), &c) in seg.iter_mut().zip(&k4[..len]).zip(&c4[..len]) {
                        *d = ((*d as u16 * k as u16 + 127) / 255 + c as u16).min(255) as u8;
                    }
                }
                x = end;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::{BlendMode, Folder, FolderBlend, Layer, LayerStack, Node, Tiles};

    const W: u32 = 150;
    const H: u32 = 90;

    fn layer(id: u64, seed: u32, blend: BlendMode, opacity: f32) -> Node {
        let mut l = Layer::new(W, H, false);
        l.id = id;
        l.blend = blend;
        l.opacity = opacity;
        let mut px = vec![0u8; (W * H * 4) as usize];
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
            let v = (i as u32).wrapping_mul(2654435761).wrapping_add(seed * 7919);
            // Patches: some pixels empty, some partly covered.
            if (i as u32 / 7 + seed).is_multiple_of(3) { continue; }
            p.copy_from_slice(&[(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8, ((v >> 4) as u8) | 0x20]);
        }
        l.tiles = Tiles::from_rgba(W, H, &px, [0, 0, 0, 0]);
        Node::Layer(l)
    }

    fn folder(id: u64, blend: FolderBlend, opacity: f32, children: Vec<Node>) -> Node {
        Node::Folder(Folder { id, name: "f".into(), visible: true, opacity, blend, locked: false, collapsed: false, children, props: Default::default(), kind: Default::default() })
    }

    fn max_diff(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    /// The cached display of a stroke matches the full merge (to rounding),
    /// before and after the painted layer changes; with a non-normal layer
    /// above, the full merge is used.
    #[test]
    fn cached_merge_matches_the_full_one() {
        for multiply_above in [false, true] {
            let mut s = LayerStack::new(W, H);
            let bg = s.root.remove(0);
            let top_mode = if multiply_above { BlendMode::Multiply } else { BlendMode::Normal };
            s.root = vec![
                bg,
                layer(10, 1, BlendMode::Multiply, 0.8),
                folder(20, FolderBlend::PassThrough, 0.7, vec![
                    layer(21, 2, BlendMode::Screen, 1.0),
                    layer(22, 3, BlendMode::Normal, 0.9), // painted
                    layer(23, 4, BlendMode::Normal, 0.5),
                ]),
                folder(30, FolderBlend::Isolated(BlendMode::Normal), 0.6, vec![layer(31, 5, BlendMode::Overlay, 1.0)]),
                layer(40, 6, top_mode, 0.4),
            ];
            assert!(s.begin_stroke(22));
            for rect in [(0, 0, W, H), (10, 5, 140, 80), (63, 63, 70, 70)] {
                let full = s.composite_rect(rect);
                let cached = s.composite_rect_stroke(rect);
                assert!(max_diff(&full, &cached) <= 2, "rect {rect:?}: off by {}", max_diff(&full, &cached));
            }
            // The painted layer changes: the cache (under / over) stays valid.
            let px = [200u8, 30, 90, 255].repeat(40 * 30);
            assert!(s.set_layer_rect(22, (20, 20, 60, 50), &px));
            let full = s.composite_rect((0, 0, W, H));
            let cached = s.composite_rect_stroke((0, 0, W, H));
            assert!(max_diff(&full, &cached) <= 2, "after painting: off by {}", max_diff(&full, &cached));
            if multiply_above { assert_eq!(full, cached, "fallback must be the full merge"); }
            s.end_stroke();
        }
    }
}

#[cfg(test)]
mod bench {
    use crate::LayerStack;

    /// Display merge of a 1000 px brush area, 30 fully painted layers,
    /// painting the 15th: full merge vs cached.
    ///     cargo test --release -p aurastudio-layers bench_stroke_cache -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_stroke_cache() {
        let (w, h) = (2480, 3508);
        let mut s = LayerStack::new_document(w, h);
        let mut ids = Vec::new();
        for i in 0..30u8 {
            let id = s.add_layer("L");
            s.set_active(id);
            let px = [i.wrapping_mul(37), 120, 200 - i, 180].repeat(1000 * 1000);
            assert!(s.set_layer_rect(id, (500, 500, 1500, 1500), &px));
            ids.push(id);
        }
        s.begin_stroke(ids[15]);
        let r = (600, 600, 1600, 1600);
        let t = std::time::Instant::now();
        for _ in 0..5 { std::hint::black_box(s.composite_rect(r)); }
        let full = t.elapsed() / 5;
        s.composite_rect_stroke(r);
        let t = std::time::Instant::now();
        for _ in 0..5 { std::hint::black_box(s.composite_rect_stroke(r)); }
        let cached = t.elapsed() / 5;
        println!("1000x1000 px, 32 layers: full merge {full:?}, cached {cached:?}");
    }
}

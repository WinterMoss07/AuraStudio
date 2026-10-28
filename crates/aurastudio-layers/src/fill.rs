//! Fill tool: the area of similar color around a point, found on the
//! merge (drafts left out), the reference or the layer itself, tile by
//! tile as the fill goes.

use aurastudio_core::{Rect, TILE};

use crate::{LayerStack, Node, Pick, Selection};

/// What the fill looks at to find the area (CSP's "Refer multiple").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refer {
    /// Only the layer filled.
    Layer,
    /// Every visible layer, merged.
    #[default]
    All,
    /// The reference layer of the filled layer's folder (or the nearest
    /// folder around with one, up to the top level).
    References,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillParams {
    /// Largest difference per channel still counted as "the same color".
    pub tolerance:   u8,
    pub refer:       Refer,
    /// Leave the draft layers out of what is looked at (CSP's "Exclude
    /// draft layers").
    pub skip_drafts: bool,
    /// Grow the area by this many pixels, under the edges of anti-aliased
    /// lines (no white halo between the line and the fill).
    pub expand:      u32,
}

impl Default for FillParams {
    fn default() -> Self { Self { tolerance: 32, refer: Refer::All, skip_drafts: true, expand: 1 } }
}

impl FillParams {
    /// The merge looked at to fill `layer` (None: the layer itself).
    pub fn pick(&self, layers: &LayerStack, layer: u64) -> Option<Pick> {
        match self.refer {
            Refer::Layer => None,
            Refer::All => Some(Pick { skip_drafts: self.skip_drafts, ..Pick::ALL }),
            Refer::References => Some(Pick {
                skip_drafts: self.skip_drafts,
                reference: Some(layers.reference_for(layer).unwrap_or(0)),
                ..Pick::ALL
            }),
        }
    }
}

/// The area: one bit per pixel (a word per tile row), and its bounds.
pub struct Area {
    cols: usize,
    bits: Vec<u64>,
    pub bbox: Rect,
}

const _: () = assert!(TILE == 64);

impl Area {
    fn new(w: u32, h: u32) -> Self {
        let cols = w.div_ceil(TILE) as usize;
        Self { cols, bits: vec![0; cols * h as usize], bbox: (0, 0, 0, 0) }
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.bits[y as usize * self.cols + (x / 64) as usize] >> (x % 64) & 1 == 1
    }

    fn set_run(&mut self, y: u32, x0: u32, x1: u32) {
        let row = y as usize * self.cols;
        let mut x = x0;
        while x < x1 {
            let (c, b) = ((x / 64) as usize, x % 64);
            let n = (x1 - x).min(64 - b);
            let ones = if n == 64 { u64::MAX } else { ((1u64 << n) - 1) << b };
            self.bits[row + c] |= ones;
            x += n;
        }
    }
}

/// Which pixels may be filled (similar to the seed color, selected),
/// worked out a whole tile at a time the first time the fill reaches it.
struct Candidates<'a> {
    layers:    &'a LayerStack,
    layer:     Option<&'a crate::Tiles>,
    pick:      Pick,
    selection: Option<&'a Selection>,
    seed:      [u8; 4],
    tolerance: u8,
    cols:      u32,
    /// Per tile: one word per row.
    tiles:     Vec<Option<Box<[u64; 64]>>>,
}

impl Candidates<'_> {
    /// Row `y` of tile column `c`: bit i is pixel (c * 64 + i, y).
    fn word(&mut self, y: u32, c: u32) -> u64 {
        let i = ((y / TILE) * self.cols + c) as usize;
        if self.tiles[i].is_none() { self.tiles[i] = Some(self.make(c, y / TILE)); }
        self.tiles[i].as_ref().unwrap()[(y % TILE) as usize]
    }

    fn make(&self, tx: u32, ty: u32) -> Box<[u64; 64]> {
        let (w, h) = (self.layers.width, self.layers.height);
        let (x0, y0) = (tx * TILE, ty * TILE);
        let (tw, th) = ((w - x0).min(TILE), (h - y0).min(TILE));
        let mut out = Box::new([0u64; 64]);
        let ok = |px: &[u8]| similar([px[0], px[1], px[2], px[3]], self.seed, self.tolerance);
        let merged;
        // Pixels of the tile, rows of `stride` pixels; None: all fill color.
        let (px, stride): (Option<&[u8]>, u32) = match self.layer {
            Some(t) => match t.tile(tx, ty) {
                Some(tile) => (Some(&tile[..]), TILE),
                None => (None, 0),
            },
            None => {
                merged = self.layers.composite_pick((x0, y0, x0 + tw, y0 + th), self.pick, false);
                (Some(&merged[..]), tw)
            }
        };
        let fill_ok = self.layer.is_some_and(|t| ok(&t.fill()));
        for y in 0..th {
            let mut word = 0u64;
            for x in 0..tw {
                let m = match px {
                    Some(p) => ok(&p[((y * stride + x) * 4) as usize..]),
                    None => fill_ok,
                };
                if m { word |= 1 << x; }
            }
            if let Some(s) = self.selection {
                for x in 0..tw {
                    if word >> x & 1 == 1 && !s.contains(x0 + x, y0 + y) { word &= !(1 << x); }
                }
            }
            out[y as usize] = word;
        }
        out
    }
}

fn similar(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    // Fully transparent pixels are alike whatever their color.
    if a[3] == 0 && b[3] == 0 { return true; }
    a.iter().zip(&b).all(|(&p, &q)| p.abs_diff(q) <= tol)
}

/// The area to fill from (x, y) on `layer`, inside `selection` if any.
/// None if the point is off the canvas or outside the selection.
#[profiling::function]
pub fn flood(layers: &LayerStack, layer: u64, x: u32, y: u32, p: FillParams, selection: Option<&Selection>) -> Option<Area> {
    flood_with(layers, layer, x, y, p, selection, p.pick(layers, layer))
}

/// `flood`, looking at the merge `pick` takes (None: the layer itself).
pub fn flood_with(layers: &LayerStack, layer: u64, x: u32, y: u32, p: FillParams, selection: Option<&Selection>, pick: Option<Pick>) -> Option<Area> {
    let (w, h) = (layers.width, layers.height);
    if x >= w || y >= h { return None; }
    if selection.is_some_and(|s| !s.contains(x, y)) { return None; }
    let merged = pick.is_some();
    let pick = pick.unwrap_or_default();
    let layer_tiles = match layers.node(layer)? {
        Node::Layer(l) if !merged => Some(&l.tiles),
        Node::Layer(_) => None,
        Node::Folder(_) | Node::Filter(_) => return None,
    };
    let seed = match layer_tiles {
        Some(t) => t.pixel(x, y),
        None => { let m = layers.composite_pick((x, y, x + 1, y + 1), pick, false); [m[0], m[1], m[2], m[3]] }
    };
    let cols = w.div_ceil(TILE);
    let mut cand = Candidates {
        layers, layer: layer_tiles, pick, selection, seed, tolerance: p.tolerance, cols,
        tiles: vec![None; (cols * h.div_ceil(TILE)) as usize],
    };
    let mut area = Area::new(w, h);
    // Fillable and not filled yet, a word at a time.
    let free = |c: &mut Candidates, a: &Area, y: u32, col: u32| c.word(y, col) & !a.bits[y as usize * a.cols + col as usize];
    // Scanline fill: whole runs of a row at a time.
    let mut stack = vec![(x, y)];
    let (mut bx0, mut by0, mut bx1, mut by1) = (x, y, x + 1, y + 1);
    while let Some((sx, sy)) = stack.pop() {
        if free(&mut cand, &area, sy, sx / 64) >> (sx % 64) & 1 == 0 { continue; }
        // Run end on the right: first pixel not free.
        let mut x1 = sx;
        while x1 < w {
            let b = x1 % 64;
            let n = (!(free(&mut cand, &area, sy, x1 / 64) >> b)).trailing_zeros();
            x1 += n;
            if n < 64 - b { break; }
        }
        let x1 = x1.min(w);
        // Run start on the left.
        let mut x0 = sx;
        loop {
            let b = x0 % 64;
            let n = (!(free(&mut cand, &area, sy, x0 / 64) << (63 - b))).leading_zeros();
            if n < b + 1 || x0 < 64 { x0 = x0 + 1 - n; break; }
            x0 -= n;
        }
        area.set_run(sy, x0, x1);
        (bx0, bx1, by0, by1) = (bx0.min(x0), bx1.max(x1), by0.min(sy), by1.max(sy + 1));
        for ny in [sy.wrapping_sub(1), sy + 1] {
            if ny >= h { continue; }
            // One seed per run of free pixels on the next row, under x0..x1.
            let mut carry = 0u64;
            for c in x0 / 64..x1.div_ceil(64) {
                let lo = if c == x0 / 64 { u64::MAX << (x0 % 64) } else { u64::MAX };
                let hi_end = x1 - c * 64;
                let hi = if hi_end >= 64 { u64::MAX } else { (1u64 << hi_end) - 1 };
                let m = free(&mut cand, &area, ny, c) & lo & hi;
                let mut starts = m & !(m << 1 | carry);
                carry = m >> 63;
                while starts != 0 {
                    stack.push((c * 64 + starts.trailing_zeros(), ny));
                    starts &= starts - 1;
                }
            }
        }
    }
    area.bbox = (bx0, by0, bx1, by1);
    for _ in 0..p.expand { grow(&mut area, w, h, selection); }
    Some(area)
}

/// Every pixel like the one at (x, y), wherever it is (the magic wand
/// without "contiguous"), looked at as `flood_with` does.
pub fn similar_with(layers: &LayerStack, layer: u64, x: u32, y: u32, p: FillParams, pick: Option<Pick>) -> Option<Area> {
    let (w, h) = (layers.width, layers.height);
    if x >= w || y >= h { return None; }
    let merged = pick.is_some();
    let pick = pick.unwrap_or_default();
    let layer_tiles = match layers.node(layer)? {
        Node::Layer(l) if !merged => Some(&l.tiles),
        Node::Layer(_) => None,
        Node::Folder(_) | Node::Filter(_) => return None,
    };
    let seed = match layer_tiles {
        Some(t) => t.pixel(x, y),
        None => { let m = layers.composite_pick((x, y, x + 1, y + 1), pick, false); [m[0], m[1], m[2], m[3]] }
    };
    let cols = w.div_ceil(TILE);
    let mut cand = Candidates {
        layers, layer: layer_tiles, pick, selection: None, seed, tolerance: p.tolerance, cols,
        tiles: vec![None; (cols * h.div_ceil(TILE)) as usize],
    };
    let mut area = Area::new(w, h);
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    for ty in 0..h.div_ceil(TILE) {
        for c in 0..cols {
            for yy in ty * TILE..((ty + 1) * TILE).min(h) {
                let word = cand.word(yy, c);
                let edge = w - c * 64;
                let word = if edge < 64 { word & ((1u64 << edge) - 1) } else { word };
                if word == 0 { continue; }
                area.bits[yy as usize * area.cols + c as usize] = word;
                bx0 = bx0.min(c * 64 + word.trailing_zeros());
                bx1 = bx1.max(c * 64 + 64 - word.leading_zeros());
                by0 = by0.min(yy);
                by1 = by1.max(yy + 1);
            }
            // Done with that tile: free it.
            cand.tiles[(ty * cols + c) as usize] = None;
        }
    }
    if bx0 == u32::MAX { return None; }
    area.bbox = (bx0, by0, bx1, by1);
    for _ in 0..p.expand { grow(&mut area, w, h, None); }
    Some(area)
}

/// Add every pixel next to the area (8 neighbors), whole words at a time.
fn grow(area: &mut Area, w: u32, h: u32, selection: Option<&Selection>) {
    let (_, y0, _, y1) = area.bbox;
    let (ny0, ny1) = (y0.saturating_sub(1), (y1 + 1).min(h));
    let cols = area.cols;
    // The last word of a row only holds the pixels left of the edge.
    let last_mask = if w.is_multiple_of(64) { u64::MAX } else { (1u64 << (w % 64)) - 1 };
    let row = |y: u32| &area.bits[y as usize * cols..(y as usize + 1) * cols];
    let mut out = vec![0u64; (ny1 - ny0) as usize * cols];
    let mut v = vec![0u64; cols];
    for y in ny0..ny1 {
        // Rows above, at and below, merged; then spread one pixel sideways.
        v.copy_from_slice(row(y));
        if y > 0 { for (a, b) in v.iter_mut().zip(row(y - 1)) { *a |= b; } }
        if y + 1 < h { for (a, b) in v.iter_mut().zip(row(y + 1)) { *a |= b; } }
        let o = &mut out[(y - ny0) as usize * cols..][..cols];
        for c in 0..cols {
            let left_in = if c > 0 { v[c - 1] >> 63 } else { 0 };
            let right_in = if c + 1 < cols { v[c + 1] << 63 } else { 0 };
            o[c] = v[c] | v[c] << 1 | left_in | v[c] >> 1 | right_in;
        }
        o[cols - 1] &= last_mask;
    }
    for y in ny0..ny1 {
        let o = &out[(y - ny0) as usize * cols..][..cols];
        for (c, &grown) in o.iter().enumerate() {
            let old = area.bits[y as usize * cols + c];
            let mut add = grown & !old;
            if let Some(s) = selection {
                let mut bits = add;
                while bits != 0 {
                    let b = bits.trailing_zeros();
                    bits &= bits - 1;
                    if !s.contains(c as u32 * 64 + b, y) { add &= !(1 << b); }
                }
            }
            area.bits[y as usize * cols + c] = old | add;
        }
    }
    let (x0, _, x1, _) = area.bbox;
    area.bbox = (x0.saturating_sub(1), ny0, (x1 + 1).min(w), ny1);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring drawn on "ink", filled from inside on "color": the inside is
    /// found from the merged image, and stops at the ring.
    #[test]
    fn fill_stops_at_lines_of_another_layer() {
        let mut s = LayerStack::new_document(64, 64);
        let ink = s.active_id();
        let mut px = [255u8, 255, 255, 0].repeat(64 * 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let (dx, dy) = (x as f32 - 32.0, y as f32 - 32.0);
                let d = (dx * dx + dy * dy).sqrt();
                if (18.0..21.0).contains(&d) {
                    px[((y * 64 + x) * 4) as usize..][..4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
        assert!(s.set_layer_rect(ink, (0, 0, 64, 64), &px));
        let color = s.add_layer("color");
        let p = FillParams { expand: 0, ..Default::default() };
        let a = flood(&s, color, 32, 32, p, None).unwrap();
        assert!(a.contains(32, 32) && a.contains(32, 16));
        assert!(!a.contains(32, 12) && !a.contains(2, 2), "went past the ring");
        // On the color layer alone (empty), the whole canvas is one area.
        let all = flood(&s, color, 32, 32, FillParams { refer: Refer::Layer, ..p }, None).unwrap();
        assert!(all.contains(2, 2));
        // Grown by one pixel: under the ring's inner edge.
        let grown = flood(&s, color, 32, 32, FillParams { expand: 1, ..p }, None).unwrap();
        let edge = (32..64).find(|&y| !a.contains(32, y)).unwrap();
        assert!(grown.contains(32, edge));
        // Inside a selection only.
        let sel = Selection::rect(64, 64, (0, 0, 32, 64)).unwrap();
        let half = flood(&s, color, 20, 32, p, Some(&sel)).unwrap();
        assert!(half.contains(20, 32) && !half.contains(40, 32));
        assert!(flood(&s, color, 40, 32, p, Some(&sel)).is_none());
    }

    /// Same areas as a plain pixel-by-pixel flood, on random images whose
    /// size is no multiple of 64, with and without a selection.
    #[test]
    fn matches_a_plain_flood() {
        let mut seed = 12345u64;
        let mut rnd = move |n: u32| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % n as u64) as u32 };
        for round in 0..40 {
            let (w, h) = (70 + rnd(130), 65 + rnd(100));
            let mut s = LayerStack::new_document(w, h);
            let id = s.active_id();
            let mut px = vec![0u8; (w * h * 4) as usize];
            for _ in 0..rnd(40) {
                let (cx, cy, r) = (rnd(w), rnd(h), 2 + rnd(25));
                let v = if rnd(2) == 0 { 0 } else { 200 };
                for y in cy.saturating_sub(r)..(cy + r).min(h) {
                    for x in cx.saturating_sub(r)..(cx + r).min(w) {
                        px[((y * w + x) * 4) as usize..][..4].copy_from_slice(&[v, v, v, 255]);
                    }
                }
            }
            assert!(s.set_layer_rect(id, (0, 0, w, h), &px));
            let sel = (round % 2 == 1).then(|| Selection::polygon(w, h, &[(5.0, 3.0), (w as f32 - 2.0, 20.0), (w as f32 / 2.0, h as f32)], false).unwrap());
            let (x, y) = (rnd(w), rnd(h));
            let p = FillParams { tolerance: 10, refer: Refer::Layer, skip_drafts: true, expand: 0 };
            let got = flood(&s, id, x, y, p, sel.as_ref());
            let allowed = |x: u32, y: u32| sel.as_ref().is_none_or(|s| s.contains(x, y));
            if !allowed(x, y) { assert!(got.is_none()); continue; }
            let got = got.unwrap();
            let at = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
            let c0 = at(x, y);
            let mut want = vec![false; (w * h) as usize];
            let mut st = vec![(x, y)];
            while let Some((x, y)) = st.pop() {
                let i = (y * w + x) as usize;
                if want[i] || !allowed(x, y) || !similar(at(x, y), c0, 10) { continue; }
                want[i] = true;
                if x > 0 { st.push((x - 1, y)); }
                if x + 1 < w { st.push((x + 1, y)); }
                if y > 0 { st.push((x, y - 1)); }
                if y + 1 < h { st.push((x, y + 1)); }
            }
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(got.contains(x, y), want[(y * w + x) as usize], "round {round} at {x},{y}");
                }
            }
        }
    }

    /// Time of a fill over a whole A4 300 dpi page.
    #[test]
    #[ignore]
    fn bench_fill_a4() {
        let s = LayerStack::new_document(2480, 3508);
        let id = s.active_id();
        for refer in [Refer::All, Refer::Layer] {
            let t = std::time::Instant::now();
            let a = flood(&s, id, 10, 10, FillParams { refer, expand: std::env::var("E").map_or(1, |v| v.parse().unwrap()), ..Default::default() }, None).unwrap();
            assert!(a.contains(2479, 3507));
            println!("{refer:?}: {:?}", t.elapsed());
        }
    }
}

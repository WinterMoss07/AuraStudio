//! Merges for other uses: a small preview, the split around a node (a
//! transform's live preview), the fill's view of the tree, and whether
//! the GPU could merge the tree itself.

use aurastudio_core::{Rect, Rgba8};
use aurastudio_layers::{FillSource, FolderBlend, LayerStack, Node, Pick};

use crate::merge::{merge, needs_general, Backdrop, Opts};

/// A small merged image of the frame (file previews, thumbnails), at most
/// `max` px a side: the frame merged in bands, each output pixel the
/// average of its source pixels.
pub fn preview(stack: &LayerStack, max: u32) -> (u32, u32, Vec<Rgba8>) {
    aurastudio_core::zone!("merge preview");
    let (w, h) = (stack.width, stack.height);
    let k = (max as f64 / w.max(h) as f64).min(1.0);
    let (tw, th) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
    let mut sum = vec![[0u32; 5]; (tw * th) as usize];
    let band = 256i32;
    for y0 in (0..h as i32).step_by(band as usize) {
        let r = Rect::new(0, y0, w as i32, (y0 + band).min(h as i32));
        let px = merge(stack, r, Opts { pick: Pick::EXPORT, ..Default::default() });
        for (i, p) in px.iter().enumerate() {
            let (x, y) = ((i % w as usize) as u64, (y0 as usize + i / w as usize) as u64);
            let (ox, oy) = ((x * tw as u64 / w as u64) as usize, (y * th as u64 / h as u64) as usize);
            let s = &mut sum[oy * tw as usize + ox];
            for c in 0..4 { s[c] += p[c] as u32; }
            s[4] += 1;
        }
    }
    let out = sum.iter().map(|s| { let n = s[4].max(1); [0, 1, 2, 3].map(|c| ((s[c] + n / 2) / n) as u8) }).collect();
    (tw, th, out)
}

/// The frame merged up to node `id` (it included), and on transparency
/// what is over it (`None`: nothing shows over it). A transform's live
/// preview puts what moves between the two (exact with normal blending
/// above).
/// `area`: the part merged (rows of it).
pub fn merge_split(stack: &LayerStack, id: u64, area: Rect) -> Option<(Vec<Rgba8>, Option<Vec<Rgba8>>)> {
    aurastudio_core::zone!("merge split");
    stack.node(id)?;
    // Drawn before / after it, in drawing order.
    let mut order = Vec::new();
    stack.walk(|n, _| order.push(n.id()));
    let k = order.iter().position(|&x| x == id)?;
    let inside: Vec<u64> = { let mut v = Vec::new(); if let Some(n) = stack.node(id) { n.for_each(&mut |c| v.push(c.id())); } v };
    let ancestors: Vec<u64> = { let mut v = Vec::new(); let mut p = stack.parent_of(id); while let Some(f) = p { v.push(f); p = stack.parent_of(f); } v };
    let mut after: Vec<u64> = order[k + 1..].iter().copied().filter(|x| !inside.contains(x)).collect();
    let mut before: Vec<u64> = order[..k].iter().copied().filter(|x| !ancestors.contains(x)).chain(inside.iter().copied()).collect();
    after.sort_unstable();
    before.sort_unstable();
    let frame = area;
    if frame.is_empty() { return None; }
    // (The other part drawn as if hidden: no copy of the stack.)
    let any_after = after.iter().any(|&i| stack.node(i).is_some_and(Node::visible));
    let below = merge(stack, frame, Opts { hidden: &after, ..Default::default() });
    let above = any_after.then(|| merge(stack, frame, Opts { backdrop: Backdrop::Transparent, hidden: &before, ..Default::default() }));
    Some((below, above))
}

/// What the fill tool looks at: the merge of the nodes `pick` takes, on
/// white.
pub struct Merged<'a> {
    pub stack: &'a LayerStack,
    pub pick:  Pick,
}

impl FillSource for Merged<'_> {
    fn pixels(&self, r: Rect) -> Vec<Rgba8> { merge(self.stack, r, Opts { pick: self.pick, ..Default::default() }) }
}

/// The GPU could merge the visible tree itself (normal blending, no
/// filter or clipping, folders pass-through or isolated normal at full
/// opacity: what its plain over handles).
pub fn gpu_mergeable(stack: &LayerStack) -> bool {
    fn walk(nodes: &[Node]) -> bool {
        nodes.iter().all(|n| !n.visible() || match n {
            Node::Layer(l) => l.blend == aurastudio_core::BlendMode::Normal && !l.props.clip,
            Node::Folder(f) => !f.props.clip && match f.blend {
                FolderBlend::PassThrough => true,
                FolderBlend::Isolated(m) => m == aurastudio_core::BlendMode::Normal && f.opacity >= 1.0,
            } && walk(&f.children),
            Node::Filter(_) => false,
            Node::Mask(_) => true,
        })
    }
    !needs_general(stack) && walk(&stack.root)
}

//! Content keys: a hash of what a merge of some nodes over a rect depends
//! on (the tiles' fingerprints there and every setting that changes the
//! result). Equal keys, equal pixels: caches never go stale and never need
//! dropping; an edit elsewhere leaves their other entries valid.

use aurastudio_core::{Fnv, Rect};
use aurastudio_layers::{FolderBlend, FolderKind, Node};

/// A node's settings and, over `r`, its pixels (a folder: its content).
pub(crate) fn node_key(h: &mut Fnv, n: &Node, r: Rect) {
    let p = n.props();
    h.u64(n.id()).bool(n.visible()).f32(n.opacity()).bytes(&[p.clip as u8, p.reference as u8, p.draft as u8]);
    match n {
        Node::Layer(l) => {
            h.u32(l.blend as u32);
            if l.visible { h.u64(l.surface.fingerprint(r)); let f = l.surface.fill(); h.bytes(&f); }
        }
        Node::Folder(f) => {
            match f.blend { FolderBlend::PassThrough => h.u32(u32::MAX), FolderBlend::Isolated(m) => h.u32(m as u32) };
            if let FolderKind::Sheet(k) = f.kind { h.u32(k); }
            h.u32(f.children.len() as u32);
            for c in &f.children { node_key(h, c, r); }
        }
        Node::Filter(f) => { h.str(&format!("{:?}", f.filter)); }
        Node::Mask(_) => {}
    }
}

/// The key of `nodes` over `r`, with a salt (what kind of merge).
pub(crate) fn nodes_key<'a>(nodes: impl IntoIterator<Item = &'a Node>, r: Rect, salt: u64) -> u64 {
    let mut h = Fnv::default();
    h.u64(salt).u32(r.x0 as u32).u32(r.y0 as u32).u32(r.x1 as u32).u32(r.y1 as u32);
    for n in nodes { node_key(&mut h, n, r); }
    h.finish()
}

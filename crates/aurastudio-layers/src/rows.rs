//! The layer list as the UI shows it: rows top to bottom, folded content
//! skipped, a sheet as one row.

use crate::node::Node;
use crate::stack::LayerStack;

/// One row of the layer list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id:        u64,
    pub depth:     usize,
    pub is_folder: bool,
    pub collapsed: bool,
    /// Locked through a folder around it.
    pub locked_by_parent: bool,
    /// Clipped and with a base under it (a clipped node at the bottom of
    /// its folder has none: it shows as it is).
    pub clipped:   bool,
    /// Reference / draft through a folder around it.
    pub reference_by_parent: bool,
    pub draft_by_parent: bool,
    /// The node right under it in its folder (a clipped node's chain).
    pub below:     Option<u64>,
    /// An animation folder.
    pub animation: bool,
    /// A group of animation folders.
    pub group:     bool,
    /// A sheet (the frame it is exposed from): one row, its cels not listed.
    pub sheet:     Option<u32>,
    /// A selection mask.
    pub mask:      bool,
}

impl LayerStack {
    /// Rows of the layer list, top to bottom, skipping folded content.
    pub fn rows(&self) -> Vec<Row> {
        #[derive(Clone, Copy)]
        struct Up { locked: bool, reference: bool, draft: bool }
        fn walk(nodes: &[Node], depth: usize, up: Up, out: &mut Vec<Row>) {
            for (i, n) in nodes.iter().enumerate().rev() {
                let sheet = n.sheet_start();
                let (is_folder, collapsed) = match n { Node::Folder(f) if sheet.is_none() => (true, f.collapsed), _ => (false, false) };
                let clipped = n.props().clip && nodes[..i].iter().any(|b| !b.props().clip);
                out.push(Row {
                    id: n.id(), depth, is_folder, collapsed, locked_by_parent: up.locked, clipped,
                    reference_by_parent: up.reference, draft_by_parent: up.draft,
                    below: i.checked_sub(1).map(|b| nodes[b].id()), animation: n.is_animation(),
                    group: n.is_group(), sheet, mask: matches!(n, Node::Mask(_)),
                });
                if let Node::Folder(f) = n {
                    if !f.collapsed && sheet.is_none() {
                        let p = f.props;
                        walk(&f.children, depth + 1, Up { locked: up.locked || f.locked, reference: up.reference || p.reference, draft: up.draft || p.draft }, out);
                    }
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, 0, Up { locked: false, reference: false, draft: false }, &mut v);
        v
    }

    /// The paint layers the layer list can show: all but the cels inside
    /// sheets (a sheet is one row; its cels never get one).
    pub fn listed_layer_ids(&self) -> Vec<u64> {
        fn walk(nodes: &[Node], out: &mut Vec<u64>) {
            for n in nodes {
                match n {
                    Node::Layer(l) => out.push(l.id),
                    Node::Folder(f) if f.sheet_start().is_none() => walk(&f.children, out),
                    _ => {}
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }
}

//! The layer tree without pixels, to undo the Layers docker's edits.
//! Going back to a state takes the nodes out and puts them back as it
//! says; the nodes it lacks wait in the history (pixels moved, never
//! copied). Visibility and folding are not undone (Photoshop, CSP).
//! After a restore the document lets `animation` settle the tree.

use std::collections::HashMap;

use aurastudio_core::BlendMode;

use crate::anim::FolderKind;
use crate::node::{FolderBlend, Node, Props};
use crate::stack::{first_layer_id, LayerStack};

/// A node's settings.
#[derive(Clone, Debug, PartialEq)]
enum Settings {
    Layer { name: String, opacity: f32, blend: BlendMode, locked: bool, props: Props },
    Folder { name: String, opacity: f32, blend: FolderBlend, locked: bool, props: Props, kind: FolderKind },
    Filter { name: String, opacity: f32, locked: bool, props: Props, filter: aurastudio_filters::Filter },
    Mask { name: String, locked: bool, props: Props },
}

#[derive(Clone, Debug)]
struct Shape {
    id:        u64,
    settings:  Settings,
    visible:   bool,
    collapsed: bool,
    /// Bottom to top.
    children:  Vec<Shape>,
}

impl PartialEq for Shape {
    // (Visibility and folding don't count.)
    fn eq(&self, o: &Self) -> bool { self.id == o.id && self.settings == o.settings && self.children == o.children }
}

/// The layer tree as it was at some point: settings and order, no pixels.
#[derive(Clone, Debug)]
pub struct TreeState {
    root:   Vec<Shape>,
    active: u64,
}

impl PartialEq for TreeState {
    // (Which layer is active doesn't count either.)
    fn eq(&self, o: &Self) -> bool { self.root == o.root }
}

fn shape(n: &Node) -> Shape {
    let (settings, children, collapsed) = match n {
        Node::Layer(l) => (Settings::Layer { name: l.name.clone(), opacity: l.opacity, blend: l.blend, locked: l.locked, props: l.props }, Vec::new(), false),
        Node::Folder(f) => (
            Settings::Folder { name: f.name.clone(), opacity: f.opacity, blend: f.blend, locked: f.locked, props: f.props, kind: undone_kind(&f.kind) },
            f.children.iter().map(shape).collect(),
            f.collapsed,
        ),
        Node::Filter(f) => (Settings::Filter { name: f.name.clone(), opacity: f.opacity, locked: f.locked, props: f.props, filter: f.filter.clone() }, Vec::new(), false),
        Node::Mask(m) => (Settings::Mask { name: m.name.clone(), locked: m.locked, props: m.props }, Vec::new(), false),
    };
    Shape { id: n.id(), settings, visible: n.visible(), collapsed, children }
}

/// A folder's kind as undo keeps it: which cel is chosen is not an
/// edit (like the active layer).
fn undone_kind(k: &FolderKind) -> FolderKind {
    let mut k = k.clone();
    if let FolderKind::Animation(a) = &mut k { a.cel = 0; }
    k
}

/// Take `nodes` apart into `pool` (folders emptied).
fn take_apart(nodes: Vec<Node>, pool: &mut HashMap<u64, Node>) {
    for mut n in nodes {
        if let Node::Folder(f) = &mut n {
            take_apart(std::mem::take(&mut f.children), pool);
        }
        pool.insert(n.id(), n);
    }
}

/// Build `shapes` from the nodes in `pool`. Those that were in the tree
/// (`current`) keep their visibility and folding.
fn build(shapes: &[Shape], pool: &mut HashMap<u64, Node>, current: &HashMap<u64, ()>) -> Vec<Node> {
    let mut out = Vec::with_capacity(shapes.len());
    for s in shapes {
        let Some(mut n) = pool.remove(&s.id) else { continue };
        let keep = current.contains_key(&s.id);
        match (&mut n, &s.settings) {
            (Node::Layer(l), Settings::Layer { name, opacity, blend, locked, props }) => {
                (l.name, l.opacity, l.blend, l.locked, l.props) = (name.clone(), *opacity, *blend, *locked, *props);
            }
            (Node::Folder(f), Settings::Folder { name, opacity, blend, locked, props, kind }) => {
                // (The cel chosen stays as it is now.)
                let mut kind = kind.clone();
                if let FolderKind::Animation(a) = &mut kind {
                    a.cel = f.anim().map_or(0, |now| now.cel).min(a.template.len().saturating_sub(1));
                }
                (f.name, f.opacity, f.blend, f.locked, f.props, f.kind) = (name.clone(), *opacity, *blend, *locked, *props, kind);
                if !keep { f.collapsed = s.collapsed; }
                f.children = build(&s.children, pool, current);
            }
            (Node::Filter(f), Settings::Filter { name, opacity, locked, props, filter }) => {
                (f.name, f.opacity, f.locked, f.props, f.filter) = (name.clone(), *opacity, *locked, *props, filter.clone());
            }
            (Node::Mask(m), Settings::Mask { name, locked, props }) => {
                (m.name, m.locked, m.props) = (name.clone(), *locked, *props);
            }
            // (Ids are never reused: can't happen.)
            _ => continue,
        }
        if !keep { *n.visible_mut() = s.visible; }
        out.push(n);
    }
    out
}

impl LayerStack {
    pub fn tree_state(&self) -> TreeState {
        TreeState { root: self.root.iter().map(shape).collect(), active: self.active }
    }

    /// The tree as `state` says, from its nodes and `kept`; returns the
    /// nodes left over. Refused if no paint layer would remain.
    pub fn restore_tree(&mut self, state: &TreeState, kept: Vec<Node>) -> Vec<Node> {
        let now = self.tree_state();
        let mut pool = HashMap::new();
        take_apart(std::mem::take(&mut self.root), &mut pool);
        let current: HashMap<u64, ()> = pool.keys().map(|id| (*id, ())).collect();
        take_apart(kept, &mut pool);
        let root = build(&state.root, &mut pool, &current);
        if first_layer_id(&root).is_none() {
            // (Can't happen with states the history made.)
            take_apart(root, &mut pool);
            self.root = build(&now.root, &mut pool, &current);
            return pool.into_values().collect();
        }
        self.root = root;
        if self.path(state.active).is_some() {
            self.active = state.active;
        } else if self.path(self.active).is_none() {
            self.active = first_layer_id(&self.root).unwrap_or(0);
        }
        // Leftovers: their children were taken apart, the shapes keep
        // where they go.
        pool.into_values().collect()
    }
}


//! The layer tree without its pixels, to undo what the Layers docker does
//! (new, delete, move, group, clip, marks, names, blend modes...).
//!
//! A `TreeState` holds every node's settings and where it sits, never
//! pixels. Going back to one takes the nodes out of the tree (pixels and
//! all), then puts them back as the state says; the nodes it doesn't
//! have (created since) come out, and the history keeps them for the
//! redo. A deleted node is kept the same way (`LayerStack::take_removed`).
//! So the pixels are moved, never copied.
//!
//! Visibility and folded folders are left as they are (Photoshop, CSP:
//! hiding a layer isn't an action to undo).

use std::collections::HashMap;

use aurastudio_canvas::BlendMode;

use crate::{first_layer_id, FolderBlend, LayerStack, Node, Props};

/// A node's settings.
#[derive(Clone, Debug, PartialEq)]
enum Settings {
    Layer { name: String, opacity: f32, blend: BlendMode, locked: bool, props: Props },
    Folder { name: String, opacity: f32, blend: FolderBlend, locked: bool, props: Props },
    Filter { name: String, opacity: f32, locked: bool, props: Props, filter: aurastudio_filters::Filter },
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
            Settings::Folder { name: f.name.clone(), opacity: f.opacity, blend: f.blend, locked: f.locked, props: f.props },
            f.children.iter().map(shape).collect(),
            f.collapsed,
        ),
        Node::Filter(f) => (Settings::Filter { name: f.name.clone(), opacity: f.opacity, locked: f.locked, props: f.props, filter: f.filter.clone() }, Vec::new(), false),
    };
    Shape { id: n.id(), settings, visible: n.visible(), collapsed, children }
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
            (Node::Folder(f), Settings::Folder { name, opacity, blend, locked, props }) => {
                (f.name, f.opacity, f.blend, f.locked, f.props) = (name.clone(), *opacity, *blend, *locked, *props);
                if !keep { f.collapsed = s.collapsed; }
                f.children = build(&s.children, pool, current);
            }
            (Node::Filter(f), Settings::Filter { name, opacity, locked, props, filter }) => {
                (f.name, f.opacity, f.locked, f.props, f.filter) = (name.clone(), *opacity, *locked, *props, filter.clone());
            }
            // (Ids are never reused: can't happen.)
            _ => continue,
        }
        if !keep { *n.visible_mut() = s.visible; }
        out.push(n);
    }
    out
}

impl Node {
    /// Bytes of pixels it holds (with what is inside a folder).
    pub fn bytes(&self) -> usize {
        match self {
            Node::Layer(l) => l.tiles.bytes(),
            Node::Folder(f) => f.children.iter().map(Node::bytes).sum(),
            Node::Filter(_) => 0,
        }
    }
}

impl LayerStack {
    pub fn tree_state(&self) -> TreeState {
        TreeState { root: self.root.iter().map(shape).collect(), active: self.active }
    }

    /// Put the tree back as `state` says, with the nodes of the tree and
    /// `kept` (deleted ones). Returns the nodes left over (made since),
    /// taken apart. Refused (the tree as it was, everything else given
    /// back) if the result would have no paint layer.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(s: &LayerStack) -> Vec<String> {
        s.rows().iter().map(|r| s.node(r.id).unwrap().name().to_owned()).collect()
    }

    #[test]
    fn undo_and_redo_of_tree_edits_keep_the_pixels() {
        let mut s = LayerStack::new_document(64, 64);
        let a = s.active_id();
        s.set_layer_rect(a, (0, 0, 8, 8), &[255, 0, 0, 255].repeat(64));
        let t0 = s.tree_state();
        // New layer, clipped, then the first one deleted.
        let b = s.add_layer("b");
        s.node_mut(b).unwrap().props_mut().clip = true;
        s.set_active(a);
        assert!(s.remove_active());
        let gone = s.take_removed();
        assert_eq!(gone.len(), 1);
        let t1 = s.tree_state();
        assert_ne!(t0, t1);
        let made = s.restore_tree(&t0, gone);
        assert_eq!(names(&s), ["Layer 1", "Paper"]);
        assert_eq!(s.layer_rect(a, (0, 0, 1, 1)).unwrap(), [255, 0, 0, 255], "pixels back");
        assert_eq!(made.len(), 1, "b kept for the redo");
        assert_eq!(s.active_id(), a);
        let kept = s.restore_tree(&t1, made);
        assert_eq!(names(&s), ["b", "Paper"]);
        assert!(s.node(b).unwrap().props().clip);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id(), a);
    }

    #[test]
    fn folders_come_apart_and_back() {
        let mut s = LayerStack::new_document(32, 32);
        let one = s.active_id();
        let two = s.add_layer("two");
        let t0 = s.tree_state();
        let f = s.group_nodes(&[one, two], "Folder").unwrap();
        assert_eq!(names(&s), ["Folder", "two", "Layer 1", "Paper"]);
        let t1 = s.tree_state();
        let made = s.restore_tree(&t0, Vec::new());
        assert_eq!(names(&s), ["two", "Layer 1", "Paper"]);
        assert_eq!(made.len(), 1);
        s.restore_tree(&t1, made);
        assert_eq!(names(&s), ["Folder", "two", "Layer 1", "Paper"]);
        assert_eq!(s.active_id(), f);
        // Hiding a layer is not a change to undo.
        *s.node_mut(two).unwrap().visible_mut() = false;
        assert_eq!(s.tree_state(), t1);
    }
}

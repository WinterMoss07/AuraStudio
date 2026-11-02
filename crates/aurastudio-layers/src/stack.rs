//! The tree: its nodes bottom to top, the active one, ids, lookup by id,
//! nodes put in and taken out, and the onion ghosts `animation` asks for.

use aurastudio_core::Rect;
use aurastudio_raster::Image;

use crate::node::{clear_pixel, FilterLayer, Folder, Ghost, Layer, MaskLayer, Node};

/// What a new stack starts as.
#[derive(Clone, Debug)]
pub enum Start {
    /// A white "Background" layer, active.
    Background,
    /// A new document (CSP): a locked white Paper, an empty "Layer 1"
    /// above it, active.
    Paper,
    /// A picture opened: it is the bottom layer, an empty "Layer 1" above
    /// it active.
    Image { name: String, surface: Image },
}

/// Layer tree + active node.
#[derive(Clone, Debug)]
pub struct LayerStack {
    /// Bottom to top.
    pub root:   Vec<Node>,
    pub(crate) active: u64,
    /// The frame (what the document shows by default; the canvas is
    /// unbounded).
    pub width:  u32,
    pub height: u32,
    /// Tile size of new layers (the document type's).
    pub tile:   u32,
    pub(crate) next_id: u64,
    /// Nodes deleted since the history last took them (to undo the
    /// deletion with their pixels).
    pub(crate) removed: Vec<Node>,
    /// The onion ghosts now: animation folder → ghosts, far to near.
    ghosts: Vec<(u64, Vec<Ghost>)>,
}

/// Where a node sits: the path of indices from the root.
pub(crate) type Path = Vec<usize>;

fn find_path(nodes: &[Node], id: u64, path: &mut Path) -> bool {
    for (i, n) in nodes.iter().enumerate() {
        path.push(i);
        if n.id() == id { return true; }
        if let Node::Folder(f) = n {
            if find_path(&f.children, id, path) { return true; }
        }
        path.pop();
    }
    false
}

fn node_in(nodes: &[Node], id: u64) -> Option<&Node> {
    for n in nodes {
        if n.id() == id { return Some(n); }
        if let Node::Folder(f) = n {
            if let Some(found) = node_in(&f.children, id) { return Some(found); }
        }
    }
    None
}

/// The list a path's parent points to (`None` if the path goes through
/// something that isn't a folder).
pub(crate) fn siblings_mut<'a>(root: &'a mut Vec<Node>, parent: &[usize]) -> Option<&'a mut Vec<Node>> {
    let mut v = root;
    for &i in parent {
        v = match v.get_mut(i)? { Node::Folder(f) => &mut f.children, _ => return None };
    }
    Some(v)
}

pub(crate) fn siblings<'a>(root: &'a [Node], parent: &[usize]) -> Option<&'a [Node]> {
    let mut v = root;
    for &i in parent {
        v = match v.get(i)? { Node::Folder(f) => &f.children, _ => return None };
    }
    Some(v)
}

pub(crate) fn count_layers(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| match n { Node::Layer(_) => 1, Node::Folder(f) => count_layers(&f.children), _ => 0 }).sum()
}

/// The topmost paint layer.
pub(crate) fn first_layer_id(nodes: &[Node]) -> Option<u64> {
    for n in nodes.iter().rev() {
        match n {
            Node::Layer(l) => return Some(l.id),
            Node::Folder(f) => if let Some(id) = first_layer_id(&f.children) { return Some(id); },
            _ => {}
        }
    }
    None
}

impl LayerStack {
    pub fn new(width: u32, height: u32, tile: u32, start: Start) -> Self {
        let mut s = Self { root: Vec::new(), active: 0, width, height, tile, next_id: 1, removed: Vec::new(), ghosts: Vec::new() };
        let (mut bottom, more) = match start {
            Start::Background => (Layer { name: "Background".into(), ..Layer::new(width, height, tile, true) }, false),
            Start::Paper => (Layer { name: "Paper".into(), locked: true, ..Layer::new(width, height, tile, true) }, true),
            Start::Image { name, surface } => (Layer { name, surface, ..Layer::new(width, height, tile, false) }, true),
        };
        bottom.id = s.new_id();
        s.active = bottom.id;
        s.root.push(Node::Layer(bottom));
        if more {
            let id = s.add_layer("Layer 1");
            s.set_active(id);
        }
        s
    }

    /// A stack read from a file, checked: unique ids, a paint layer, frame
    /// sizes; the active node valid, one reference per folder.
    pub fn from_parts(width: u32, height: u32, tile: u32, root: Vec<Node>, active: u64) -> Result<Self, String> {
        if width == 0 || height == 0 { return Err("empty canvas".into()); }
        let mut ids = std::collections::HashSet::new();
        let mut bad = None;
        for n in &root {
            n.for_each(&mut |n| {
                if n.id() == 0 || !ids.insert(n.id()) { bad.get_or_insert(format!("duplicate or zero layer id {}", n.id())); }
                if let Node::Layer(l) = n {
                    if (l.surface.width(), l.surface.height()) != (width, height) { bad.get_or_insert(format!("layer {} has another frame", l.id)); }
                }
            });
        }
        if let Some(e) = bad { return Err(e); }
        if count_layers(&root) == 0 { return Err("no paint layer".into()); }
        let next_id = ids.iter().max().copied().unwrap_or(0) + 1;
        let mut s = Self { root, active: 0, width, height, tile, next_id, removed: Vec::new(), ghosts: Vec::new() };
        s.active = if ids.contains(&active) { active } else { first_layer_id(&s.root).unwrap_or(0) };
        // (Files from 0.10.0 could mark several references in a folder.)
        s.one_reference_per_folder(s.active);
        Ok(s)
    }

    /// A fresh id (never reused in this stack).
    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(crate) fn path(&self, id: u64) -> Option<Path> {
        let mut p = Vec::new();
        find_path(&self.root, id, &mut p).then_some(p)
    }

    /// The folder `id` is in (None: at the top level, or no such node).
    pub fn parent_of(&self, id: u64) -> Option<u64> {
        let path = self.path(id)?;
        let (_, parent) = path.split_last()?;
        let (last, up) = parent.split_last()?;
        siblings(&self.root, up)?.get(*last).map(Node::id)
    }

    /// Its folder's id (0: the top level) and its index there, bottom to top.
    pub fn place_of(&self, id: u64) -> Option<(u64, usize)> {
        let path = self.path(id)?;
        let (&i, _) = path.split_last()?;
        Some((self.parent_of(id).unwrap_or(0), i))
    }

    pub fn active_id(&self) -> u64 { self.active }

    /// Make `id` active (a cel: its sheet, a row of the list).
    pub fn set_active(&mut self, id: u64) {
        if self.path(id).is_none() { return; }
        self.active = match self.anim_of(id) { Some((_, Some(sheet))) => sheet, _ => id };
    }

    pub fn node(&self, id: u64) -> Option<&Node> { node_in(&self.root, id) }

    pub fn node_mut(&mut self, id: u64) -> Option<&mut Node> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        siblings_mut(&mut self.root, parent)?.get_mut(*last)
    }

    /// The active node (`None` only in a stack being rebuilt).
    pub fn active_node(&self) -> Option<&Node> { self.node(self.active) }
    pub fn active_node_mut(&mut self) -> Option<&mut Node> { let id = self.active; self.node_mut(id) }

    pub fn layer(&self, id: u64) -> Option<&Layer> { self.node(id)?.as_layer() }
    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> { match self.node_mut(id)? { Node::Layer(l) => Some(l), _ => None } }

    /// True if node `id` or one of its folders is locked.
    pub fn is_locked(&self, id: u64) -> bool {
        let Some(path) = self.path(id) else { return false };
        let mut nodes = &self.root[..];
        for &i in &path {
            let n = &nodes[i];
            if n.locked() { return true; }
            if let Node::Folder(f) = n { nodes = &f.children; }
        }
        false
    }

    pub fn layer_count(&self) -> usize { count_layers(&self.root) }

    /// Every node, depth first (a folder before its children), bottom to
    /// top: `f(node, depth)`.
    pub fn walk(&self, mut f: impl FnMut(&Node, usize)) {
        fn go(nodes: &[Node], depth: usize, f: &mut impl FnMut(&Node, usize)) {
            for n in nodes {
                f(n, depth);
                if let Node::Folder(fo) = n { go(&fo.children, depth + 1, f); }
            }
        }
        go(&self.root, 0, &mut f);
    }

    /// Every node, mutable, depth first.
    pub fn walk_mut(&mut self, mut f: impl FnMut(&mut Node)) { for n in &mut self.root { n.for_each_mut(&mut f); } }

    /// Put `node` into folder `parent` (0: the top level) at `index`
    /// (bottom to top, clamped). False if `parent` is no folder.
    pub fn add_node(&mut self, node: Node, parent: u64, index: usize) -> bool {
        let list = if parent == 0 { Some(&mut self.root) } else {
            match self.node_mut(parent) { Some(Node::Folder(f)) => Some(&mut f.children), _ => None }
        };
        let Some(list) = list else { return false };
        let i = index.min(list.len());
        list.insert(i, node);
        true
    }

    /// Take node `id` out of the tree (with its content); the active node
    /// stays valid. Not kept for undo (`remove_active` does that).
    pub fn take_node(&mut self, id: u64) -> Option<Node> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        let n = siblings_mut(&mut self.root, parent)?.remove(*last);
        if self.path(self.active).is_none() { self.active = first_layer_id(&self.root).unwrap_or(0); }
        Some(n)
    }

    /// A copy of `n` with fresh ids (a folder's content too). A copy of the
    /// Paper is a plain layer.
    pub fn clone_node(&mut self, n: &Node) -> Node {
        let mut c = n.clone();
        c.for_each_mut(&mut |x| {
            let id = self.new_id();
            x.set_id(id);
            if let Node::Layer(l) = x { l.background = false; }
        });
        c
    }

    /// Housekeeping after edits: groups left empty go (a group holds
    /// animation folders only), the active node exists.
    pub fn tidy(&mut self) {
        fn prune(nodes: &mut Vec<Node>) {
            nodes.retain(|n| !(n.is_group() && matches!(n, Node::Folder(f) if f.children.is_empty())));
            for n in nodes.iter_mut() { if let Node::Folder(f) = n { prune(&mut f.children); } }
        }
        prune(&mut self.root);
        if self.path(self.active).is_none() { self.active = first_layer_id(&self.root).unwrap_or(0); }
    }

    /// The deleted nodes not taken yet (the history keeps them).
    pub fn take_removed(&mut self) -> Vec<Node> { std::mem::take(&mut self.removed) }

    /// A node taken out by another crate's edit (a sheet deleted), kept for
    /// the history like the deleted ones.
    pub fn keep_removed(&mut self, n: Node) { self.removed.push(n); }

    // --- Onion ghosts -------------------------------------------------

    /// The ghosts now (folder → ghosts, far to near).
    pub fn ghosts(&self) -> &[(u64, Vec<Ghost>)] { &self.ghosts }

    /// The ghosts of animation folder `folder`, if any.
    pub fn ghosts_of(&self, folder: u64) -> Option<&[Ghost]> {
        self.ghosts.iter().find(|x| x.0 == folder).map(|x| &x.1[..]).filter(|v| !v.is_empty())
    }

    /// New ghosts; returns where the merge changes (the sheets that
    /// appear or go), `None` if the ghosts are the same.
    pub fn set_ghosts(&mut self, ghosts: Vec<(u64, Vec<Ghost>)>) -> Option<Rect> {
        if ghosts == self.ghosts { return None; }
        let old = std::mem::replace(&mut self.ghosts, ghosts);
        let mut r = Rect::EMPTY;
        for (_, g) in old.iter().chain(self.ghosts.iter()) {
            for gh in g {
                match self.node(gh.sheet).map(Node::footprint) {
                    Some(crate::Footprint::Area(a)) => r = r.union(&a),
                    Some(crate::Footprint::All) => r = r.union(&self.frame()),
                    _ => {}
                }
            }
        }
        // (Same sheets with another tint or opacity: their area.)
        Some(r)
    }

    /// The frame as a rect.
    pub fn frame(&self) -> Rect { Rect::of_size(self.width, self.height) }

    // --- Node makers ----------------------------------------------------

    pub(crate) fn make_layer(&mut self, name: &str) -> Node {
        let mut l = Layer::new(self.width, self.height, self.tile, false);
        l.id = self.new_id();
        l.name = name.into();
        Node::Layer(l)
    }

    pub(crate) fn make_folder(&mut self, name: &str) -> Folder { Folder { id: self.new_id(), ..Folder::new(name) } }

    pub(crate) fn make_filter(&mut self, filter: aurastudio_filters::Filter) -> Node {
        Node::Filter(FilterLayer { id: self.new_id(), name: filter.kind.name().into(), visible: true, opacity: 1.0, locked: false, props: Default::default(), filter })
    }

    pub(crate) fn make_mask(&mut self, name: &str, mask: aurastudio_raster::Mask) -> Node {
        Node::Mask(MaskLayer { id: self.new_id(), name: name.into(), visible: true, locked: false, props: Default::default(), mask })
    }

    /// What a cleared layer of this stack holds.
    pub fn clear_pixel(background: bool) -> aurastudio_core::Rgba8 { clear_pixel(background) }
}

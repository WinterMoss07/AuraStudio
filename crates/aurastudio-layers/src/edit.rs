//! Structure edits: add, group, ungroup, move, duplicate, remove; the
//! references (one per folder) and where a node may land. After an edit
//! the document lets `animation` settle the tree (sheets shown by time).

use crate::anim::FolderKind;
use crate::node::{Folder, Node};
use crate::stack::{count_layers, first_layer_id, siblings, siblings_mut, LayerStack, Path};

/// Where a dragged node lands relative to the one it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place { Above, Below, Inside }

impl LayerStack {
    /// Where a new node goes: right above the active node in the same
    /// folder, or at the top inside the active folder. Never into an
    /// animation folder (only its sheets are there), nor into a group
    /// unless it is an animation folder.
    fn insert_point(&mut self, animation: bool) -> Option<(Path, usize)> {
        if let Some((folder, _)) = self.anim_of(self.active) {
            let path = self.path(folder)?;
            let (last, parent) = path.split_last()?;
            if !animation && self.in_group(folder) {
                let (g, up) = parent.split_last()?;
                return Some((up.to_vec(), g + 1));
            }
            return Some((parent.to_vec(), last + 1));
        }
        let path = self.path(self.active)?;
        let (last, parent) = path.split_last()?;
        let (last, parent) = (*last, parent.to_vec());
        match self.active_node_mut()? {
            Node::Folder(f) if f.is_group() && !animation => Some((parent, last + 1)),
            Node::Folder(f) => { f.collapsed = false; let n = f.children.len(); Some((path, n)) }
            _ => Some((parent, last + 1)),
        }
    }

    /// Insert `node` where new nodes go and make it active.
    pub fn insert(&mut self, node: Node) -> u64 {
        let id = node.id();
        let at = self.insert_point(node.is_animation());
        match at.and_then(|(parent, i)| siblings_mut(&mut self.root, &parent).map(|s| (s, i))) {
            Some((s, i)) => s.insert(i.min(s.len()), node),
            None => self.root.push(node),
        }
        self.active = id;
        id
    }

    pub fn add_layer(&mut self, name: &str) -> u64 { let n = self.make_layer(name); self.insert(n) }

    pub fn add_folder(&mut self, name: &str) -> u64 { let f = self.make_folder(name); self.insert(Node::Folder(f)) }

    /// A filter layer with `filter` where new nodes go, made active.
    pub fn add_filter_layer(&mut self, filter: aurastudio_filters::Filter) -> u64 { let n = self.make_filter(filter); self.insert(n) }

    /// A selection kept as a mask node, where new nodes go, made active.
    pub fn add_mask(&mut self, name: &str, mask: aurastudio_raster::Mask) -> u64 { let n = self.make_mask(name, mask); self.insert(n) }

    /// Put the active node into a new folder in its place (Ctrl+G), the
    /// folder active. Returns its id.
    pub fn group_active(&mut self, name: &str) -> u64 {
        // (Sheets stay in their animation folder, animation folders in their group.)
        if self.is_anim_part(self.active) || self.in_group(self.active) { return self.active; }
        let Some(path) = self.path(self.active) else { return self.active };
        let Some((&last, parent)) = path.split_last() else { return self.active };
        let mut folder = self.make_folder(name);
        let Some(s) = siblings_mut(&mut self.root, parent) else { return self.active };
        folder.children.push(s.remove(last));
        let id = folder.id;
        s.insert(last, Node::Folder(folder));
        self.active = id;
        id
    }

    /// Take the active folder's content out, in its place, and remove the
    /// folder (its opacity and blend mode are lost, as in Krita). False if
    /// the active node isn't a plain folder or a group.
    pub fn ungroup_active(&mut self) -> bool {
        let Some(path) = self.path(self.active) else { return false };
        let Some((&last, parent)) = path.split_last() else { return false };
        let Some(s) = siblings_mut(&mut self.root, parent) else { return false };
        // (An animation folder's sheets only live in it.)
        if !matches!(&s[last], Node::Folder(f) if matches!(f.kind, FolderKind::Plain | FolderKind::Group)) { return false; }
        let Node::Folder(f) = s.remove(last) else { return false };
        let top = f.children.last().map(Node::id);
        for (k, child) in f.children.into_iter().enumerate() { s.insert(last + k, child); }
        self.active = top.or_else(|| first_layer_id(&self.root)).unwrap_or(0);
        // (Its reference may meet another one there.)
        self.one_reference_per_folder(0);
        true
    }

    /// Put `ids` in a new folder (Ctrl+G on several layers), where the
    /// topmost of them was, keeping their order; the folder is active. A
    /// node inside another one of them goes with it. `None` if none of
    /// them exists.
    pub fn group_nodes(&mut self, ids: &[u64], name: &str) -> Option<u64> {
        let ids: Vec<u64> = ids.iter().copied().filter(|id| !self.is_anim_part(*id)).collect();
        let tops = self.outermost(&ids);
        let (&first, rest) = tops.split_first()?;
        self.active = first;
        let folder = self.group_active(name);
        let mut prev = first;
        for &id in rest { if self.move_node(id, prev, Place::Below) { prev = id; } }
        self.active = folder;
        Some(folder)
    }

    /// Move `ids` together next to `target` (several layers dragged),
    /// keeping their order, `dragged` active. False if they can't go there
    /// (onto or into one of them).
    pub fn move_nodes(&mut self, ids: &[u64], dragged: u64, target: u64, place: Place) -> bool {
        if ids.len() < 2 { return self.move_node(dragged, target, place); }
        if ids.iter().any(|id| self.is_anim_part(*id) || !self.can_land(*id, target, place)) { return false; }
        let tops = self.outermost(ids);
        let Some(to) = self.path(target) else { return false };
        if tops.iter().any(|&id| self.path(id).is_some_and(|p| to.starts_with(&p))) { return false; }
        let Some((&first, rest)) = tops.split_first() else { return false };
        if !self.move_node(first, target, place) { return false; }
        let mut prev = first;
        for &id in rest { if self.move_node(id, prev, Place::Below) { prev = id; } }
        self.set_active(dragged);
        true
    }

    /// `ids` that exist, top to bottom as listed, without those inside
    /// another one of them (they come along with it).
    fn outermost(&self, ids: &[u64]) -> Vec<u64> {
        fn walk(nodes: &[Node], ids: &[u64], out: &mut Vec<u64>) {
            for n in nodes.iter().rev() {
                if ids.contains(&n.id()) { out.push(n.id()); } else if let Node::Folder(f) = n { walk(&f.children, ids, out); }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, ids, &mut out);
        out
    }

    /// The reference `id` looks at (fill, "stay inside the lines"): the
    /// visible reference of its folder, else of the folder around it, up
    /// to the top level (one per folder, CSP). Never `id` itself.
    pub fn reference_for(&self, id: u64) -> Option<u64> {
        let path = self.path(id)?;
        let mut levels: Vec<&[Node]> = vec![&self.root];
        for k in 1..path.len() { levels.push(siblings(&self.root, &path[..k])?); }
        levels.iter().rev().find_map(|nodes| nodes.iter().find(|n| n.props().reference && n.visible() && n.id() != id).map(Node::id))
    }

    /// Mark `id` as its folder's reference (the other one there, if any,
    /// isn't any more), or unmark it.
    pub fn set_reference(&mut self, id: u64, on: bool) {
        // (In an animation folder the cel template decides: the Ink.)
        if self.is_anim_part(id) || self.node(id).is_some_and(Node::is_animation) { return; }
        let Some(path) = self.path(id) else { return };
        let Some((&last, parent)) = path.split_last() else { return };
        let Some(s) = siblings_mut(&mut self.root, parent) else { return };
        for (i, n) in s.iter_mut().enumerate() {
            if i == last { n.props_mut().reference = on; } else if on { n.props_mut().reference = false; }
        }
    }

    /// At most one reference per folder (after a move, an ungroup, a
    /// file): `keep` stays one, else the topmost.
    pub(crate) fn one_reference_per_folder(&mut self, keep: u64) {
        fn walk(nodes: &mut [Node], keep: u64) {
            let mine = nodes.iter().position(|n| n.id() == keep && n.props().reference).or_else(|| nodes.iter().rposition(|n| n.props().reference));
            for (i, n) in nodes.iter_mut().enumerate() {
                if Some(i) != mine { n.props_mut().reference = false; }
                if let Node::Folder(f) = n { walk(&mut f.children, keep); }
            }
        }
        walk(&mut self.root, keep);
    }

    /// The node `id` is clipped onto (its base), or `None` if it isn't
    /// clipped or has nothing under it in its folder.
    #[cfg(test)]
    pub fn clip_base(&self, id: u64) -> Option<u64> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        let nodes = siblings(&self.root, parent)?;
        if !nodes[*last].props().clip { return None; }
        nodes[..*last].iter().rev().find(|n| !n.props().clip).map(Node::id)
    }

    /// Copy of the active node (a folder with its content), placed right
    /// above it and made active.
    pub fn duplicate_active(&mut self) -> u64 {
        // (Sheets: copied and cloned in the X-sheet.)
        if self.is_anim_part(self.active) { return self.active; }
        let Some(path) = self.path(self.active) else { return self.active };
        let Some((&last, parent)) = path.split_last() else { return self.active };
        let Some(original) = siblings(&self.root, parent).and_then(|s| s.get(last)).cloned() else { return self.active };
        let mut copy = self.clone_node(&original);
        *copy.name_mut() = format!("{} copy", original.name());
        // A copy is no reference (one per folder) nor a draft: it's a new
        // layer to work on (CSP).
        copy.props_mut().reference = false;
        copy.props_mut().draft = false;
        let id = copy.id();
        if let Some(s) = siblings_mut(&mut self.root, parent) { s.insert(last + 1, copy); }
        self.active = id;
        id
    }

    /// Why the active node can't be deleted: locked (itself, a folder
    /// around, or something inside a folder), a cel, or the last paint
    /// layer.
    pub fn removal_refused(&self) -> Option<&'static str> {
        // (The locks of an animation folder's cels are phase locks of the
        // cel template, not a protection of the sheet.)
        fn any_locked(n: &Node) -> bool {
            n.locked() || matches!(n, Node::Folder(f) if f.sheet_start().is_none() && f.children.iter().any(any_locked))
        }
        let node = self.active_node()?;
        if self.is_locked(self.active) || any_locked(node) { return Some("Locked: unlock it first"); }
        if matches!(node, Node::Layer(_)) && self.is_anim_part(self.active) { return Some("Cels follow the cel template"); }
        let removed = match node { Node::Layer(_) => 1, Node::Folder(f) => count_layers(&f.children), _ => 0 };
        (self.layer_count() == removed).then_some("Can't delete the last layer")
    }

    /// Remove the active node (a folder with its content) unless
    /// `removal_refused`; it waits in `take_removed` for the history.
    pub fn remove_active(&mut self) -> bool {
        if self.removal_refused().is_some() { return false; }
        let Some(path) = self.path(self.active) else { return false };
        let Some((&last, parent)) = path.split_last() else { return false };
        let Some(s) = siblings_mut(&mut self.root, parent) else { return false };
        let gone = s.remove(last);
        // Next active: the node that was below, else above, else the parent.
        let next = if last > 0 { Some(s[last - 1].id()) } else { s.first().map(Node::id) };
        self.active = match next {
            Some(id) => id,
            None if !parent.is_empty() => {
                let (p, up) = parent.split_last().unwrap_or((&0, &[]));
                siblings(&self.root, up).and_then(|s| s.get(*p)).map_or(0, Node::id)
            }
            None => first_layer_id(&self.root).unwrap_or(0),
        };
        self.removed.push(gone);
        true
    }

    /// Move the active node one step up or down. Moving onto a plain
    /// folder goes inside it; past the top / bottom of a folder goes out.
    pub fn move_active(&mut self, up: bool) {
        if self.is_anim_part(self.active) { return; }
        self.move_one_step(up);
        // (Into or out of a folder: it may meet another reference there.)
        self.one_reference_per_folder(self.active);
    }

    fn move_one_step(&mut self, up: bool) -> Option<()> {
        let path = self.path(self.active)?;
        let (&last, parent) = path.split_last()?;
        let s = siblings_mut(&mut self.root, parent)?;
        let neighbor = if up { (last + 1 < s.len()).then_some(last + 1) } else { last.checked_sub(1) };
        match neighbor {
            Some(n) if matches!(&s[n], Node::Folder(f) if f.kind == FolderKind::Plain) => {
                // Into the folder: at its bottom going up, its top going down.
                let node = s.remove(last);
                let n = if n > last { n - 1 } else { n };
                if let Node::Folder(f) = &mut s[n] {
                    f.collapsed = false;
                    if up { f.children.insert(0, node) } else { f.children.push(node) }
                }
            }
            Some(n) => s.swap(last, n),
            None if !parent.is_empty() => {
                // Out of the folder, right above / below it.
                let node = s.remove(last);
                let (&fi, grand) = parent.split_last()?;
                let outer = siblings_mut(&mut self.root, grand)?;
                outer.insert(if up { fi + 1 } else { fi }, node);
            }
            None => {}
        }
        Some(())
    }

    /// Move node `id` next to `target` (drag and drop in the layer list),
    /// and make it active. False if it can't go there.
    pub fn move_node(&mut self, id: u64, target: u64, place: Place) -> bool {
        if !self.can_drop(id, target, place) { return false; }
        let (Some(from), Some(to)) = (self.path(id), self.path(target)) else { return false };
        // A folder can't go inside itself.
        if to.starts_with(&from) { return false; }
        let Some(node) = self.take_node(id) else { return false };
        let Some(to) = self.path(target) else { return false };
        let Some((&tl, tp)) = to.split_last() else { return false };
        let Some(s) = siblings_mut(&mut self.root, tp) else { return false };
        match (place, &mut s[tl]) {
            (Place::Inside, Node::Folder(f)) => { f.collapsed = false; f.children.push(node); }
            (Place::Inside | Place::Above, _) => s.insert(tl + 1, node),
            (Place::Below, _) => s.insert(tl, node),
        }
        self.active = id;
        // A reference moved into a folder that has one: it stays the one.
        self.one_reference_per_folder(id);
        true
    }

    /// `id` may be dropped `place` of `target`.
    pub fn can_drop(&self, id: u64, target: u64, place: Place) -> bool {
        id != target && !self.is_anim_part(id) && self.can_land(id, target, place)
    }

    /// Not among sheets, not into an animation folder; a group takes only
    /// animation folders, and never goes into another group.
    fn can_land(&self, id: u64, target: u64, place: Place) -> bool {
        if self.is_anim_part(target) { return false; }
        if place == Place::Inside && self.node(target).is_some_and(Node::is_animation) { return false; }
        let into_group = match (place, self.node(target)) {
            (Place::Inside, Some(Node::Folder(f))) => f.is_group(),
            _ => self.in_group(target),
        };
        let node = self.node(id);
        let is_group = node.is_some_and(Node::is_group);
        !(into_group && (is_group || !node.is_some_and(Node::is_animation)))
    }

    /// A new folder built from `f` (an animation folder made by
    /// `animation`), inserted where new nodes go.
    pub fn insert_folder(&mut self, mut f: Folder) -> u64 {
        f.id = self.new_id();
        self.insert(Node::Folder(f))
    }
}

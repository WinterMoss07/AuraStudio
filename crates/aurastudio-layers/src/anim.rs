//! Animation folders. One folder per element (a character, smoke...),
//! one child per drawn frame ("Frame N", its place in time), each frame
//! holding the cels of the folder's cel template (genga, douga, ink,
//! colo...) as paint layers. Only one frame of a folder shows at a time:
//! the last one drawn at or before the current frame (a hold), carried by
//! the frames' visibility so that every merge stays as it is.
//!
//! The template is the same for every frame of a folder: locks, cel
//! choice and (later) the onion skin go by a cel's rank, so a douga is
//! never shown or painted among gengas.
//!
//! The hierarchy has one level of each, never nested: Group (animation
//! folders put together, for the timeline) → Folder → Frame → Cel.

use crate::{first_layer_id, Folder, FolderBlend, Layer, LayerStack, Node, Props};

/// What a folder is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FolderKind {
    #[default]
    Plain,
    /// An animation folder: its frames follow `Anim`'s template.
    Animation(Anim),
    /// One frame of an animation folder (its number, from 1): its
    /// children are the cels, bottom to top.
    Frame(u32),
    /// Animation folders put together (one entry in the timeline). Holds
    /// only animation folders, and is never inside another group.
    Group,
}

/// An animation folder's cel template and the cel painted in.
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    /// Top to bottom, as listed.
    pub template: Vec<Cel>,
    /// Rank (in `template`) of the cel strokes go to, in every frame.
    pub cel:      usize,
}

/// One cel of a template: its name, and what a new frame starts with.
#[derive(Clone, Debug, PartialEq)]
pub struct Cel {
    pub name:    String,
    /// The coloring cel (palette colors, later).
    pub colo:    bool,
    /// The reference of its frame (what the Colo's fill reads): Ink.
    pub reference: bool,
    pub visible: bool,
    pub locked:  bool,
    /// Shown by the onion skin (later).
    pub onion:   bool,
}

impl Cel {
    pub fn new(name: &str, colo: bool) -> Self {
        Self { name: name.into(), colo, reference: false, visible: true, locked: false, onion: true }
    }
}

impl Default for Anim {
    /// Listed top to bottom, as the Layers docker does: each new cel
    /// stacks above the last. Colo, ink, douga, genga; strokes go to the
    /// genga first, and the ink is the reference of the colo.
    fn default() -> Self {
        let template = vec![
            Cel::new("Colo", true),
            Cel { reference: true, ..Cel::new("Ink", false) },
            Cel::new("Douga", false),
            Cel::new("Genga", false),
        ];
        Self { template, cel: 3 }
    }
}

/// Where a stroke (or a fill) goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// This paint layer.
    Layer(u64),
    /// A frame to make first: this animation folder has none at `frame`.
    NewFrame { folder: u64, frame: u32 },
}

/// What a frame moved or copied onto a drawn one does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Onto { Replace, Swap, Refuse }

/// Timing edits of `shift_frames`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift { Insert, Remove }


/// One entry of the timeline / X-sheet: an animation folder or a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Folder(u64),
    Group(u64),
}

impl Column {
    pub fn id(self) -> u64 {
        match self { Column::Folder(i) | Column::Group(i) => i }
    }
}

impl LayerStack {
    /// Put animation folders `ids` in a new group, where the topmost of
    /// them was, keeping their order (the group is folded and active).
    /// Only animation folders go in; ones already in a group come out of
    /// it. None if there is none.
    pub fn group_animation_folders(&mut self, ids: &[u64], name: &str) -> Option<u64> {
        // Listed top to bottom, those that are animation folders, once.
        let mut folders: Vec<u64> = Vec::new();
        fn walk(nodes: &[Node], ids: &[u64], out: &mut Vec<u64>) {
            for n in nodes.iter().rev() {
                if n.is_animation() && ids.contains(&n.id()) { out.push(n.id()); }
                else if let Node::Folder(f) = n { if f.frame().is_none() { walk(&f.children, ids, out); } }
            }
        }
        walk(&self.root, ids, &mut folders);
        let &first = folders.first()?;
        // The group goes right above the topmost (above its group, if it
        // is in one: groups never nest).
        let anchor = self.group_of(first).unwrap_or(first);
        let path = self.path(anchor)?;
        let (last, parent) = path.split_last().unwrap();
        let id = self.new_id();
        let group = Folder {
            id, name: name.into(), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: true, children: Vec::new(), props: Props::default(), kind: FolderKind::Group,
        };
        crate::siblings_mut(&mut self.root, parent).insert(last + 1, Node::Folder(group));
        let mut taken: Vec<Node> = Vec::new();
        for &f in &folders {
            let p = self.path(f)?;
            let (l, par) = p.split_last().unwrap();
            taken.push(crate::siblings_mut(&mut self.root, par).remove(*l));
        }
        // Bottom to top.
        taken.reverse();
        if let Some(Node::Folder(g)) = self.node_mut(id) { g.children = taken; }
        self.active = id;
        self.settle();
        Some(id)
    }

    /// What the timeline lists: inside group `group`, its folders; at the
    /// top (None), the groups and the animation folders that are not in
    /// one. Bottom to top, as the stack.
    pub fn columns(&self, group: Option<u64>) -> Vec<Column> {
        fn walk(nodes: &[Node], out: &mut Vec<Column>) {
            for n in nodes {
                let Node::Folder(f) = n else { continue };
                match f.kind {
                    FolderKind::Group => out.push(Column::Group(f.id)),
                    FolderKind::Animation(_) => out.push(Column::Folder(f.id)),
                    FolderKind::Plain => walk(&f.children, out),
                    FolderKind::Frame(_) => {}
                }
            }
        }
        let mut v = Vec::new();
        match group {
            None => walk(&self.root, &mut v),
            Some(g) => if let Some(Node::Folder(f)) = self.node(g) {
                if f.kind == FolderKind::Group { for n in &f.children { if n.is_animation() { v.push(Column::Folder(n.id())); } } }
            },
        }
        v
    }

    /// The animation folders in group `id`, bottom to top.
    pub fn group_members(&self, id: u64) -> Vec<u64> {
        self.columns(Some(id)).into_iter().map(Column::id).collect()
    }

    /// The group an animation folder is in.
    pub fn group_of(&self, id: u64) -> Option<u64> {
        let path = self.path(id)?;
        let mut nodes = &self.root;
        let mut group = None;
        for (k, &i) in path.iter().enumerate() {
            if k + 1 == path.len() { break; }
            let Node::Folder(f) = &nodes[i] else { return None };
            if f.kind == FolderKind::Group { group = Some(f.id); }
            nodes = &f.children;
        }
        group
    }

    /// A document that starts as an animation: the paper (locked), then
    /// an animation folder, nothing else.
    pub fn start_animation(&mut self) {
        // Everything but the paper goes.
        let paper = self.root.drain(..).next();
        self.root.extend(paper);
        let first = self.root.first().map(Node::id).unwrap_or(0);
        self.active = first;
        self.add_animation_folder("Animation 1");
    }

    /// Where a moved frame set can go: `frames` of `folder` shifted by
    /// `delta`, kept in order; false if any would leave 1..=MAX_FRAME or
    /// land on a drawn frame that is not itself moving (`onto` decides).
    pub fn move_frames(&mut self, folder: u64, frames: &[u32], delta: i32, copy: bool, onto: Onto) -> bool {
        if delta == 0 || frames.is_empty() { return false; }
        let mut from: Vec<u32> = frames.iter().copied().filter(|n| self.frame_at(folder, *n).is_some()).collect();
        from.sort_unstable();
        from.dedup();
        if from.is_empty() { return false; }
        let to = |n: u32| n as i64 + delta as i64;
        if from.iter().any(|n| !(1..=MAX_FRAME as i64).contains(&to(*n))) { return false; }
        // Landing on a drawn frame that stays where it is.
        let clash = from.iter().any(|n| { let t = to(*n) as u32; self.frame_at(folder, t).is_some() && (copy || !from.contains(&t)) });
        if clash && onto == Onto::Refuse { return false; }
        // Moved far end first so no frame lands on one still to move.
        let order: Vec<u32> = if delta > 0 { from.iter().rev().copied().collect() } else { from.clone() };
        let mut any = false;
        for n in order {
            let t = to(n) as u32;
            any |= if copy { self.copy_frame(folder, n, t, Onto::Replace) } else { self.move_frame(folder, n, t, Onto::Replace) };
        }
        any
    }
}

/// A frame node without the ids of its children changed (to clone it
/// while the stack is borrowed).
fn clone_shallow(n: &Node) -> Node {
    match n {
        Node::Folder(f) => Node::Folder(Folder {
            id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity, blend: f.blend, locked: f.locked,
            collapsed: f.collapsed, props: f.props, kind: f.kind.clone(),
            children: f.children.iter().map(clone_shallow).collect(),
        }),
        Node::Layer(l) => Node::Layer(Layer {
            id: l.id, name: l.name.clone(), visible: l.visible, opacity: l.opacity, blend: l.blend, locked: l.locked,
            tiles: l.tiles.clone(), background: l.background, props: l.props,
        }),
        Node::Filter(f) => Node::Filter(crate::FilterLayer {
            id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity, locked: f.locked, props: f.props, filter: f.filter.clone(),
        }),
    }
}

/// Most frames a folder may reach (a 24 fps hour).
pub const MAX_FRAME: u32 = 86_400;

impl Folder {
    pub fn anim(&self) -> Option<&Anim> {
        match &self.kind { FolderKind::Animation(a) => Some(a), _ => None }
    }

    pub fn frame(&self) -> Option<u32> {
        match self.kind { FolderKind::Frame(n) => Some(n), _ => None }
    }
}

impl Node {
    /// It is an animation folder.
    pub fn is_animation(&self) -> bool { matches!(self, Node::Folder(f) if f.anim().is_some()) }

    /// Its frame number, if it is a frame.
    pub fn frame(&self) -> Option<u32> {
        match self { Node::Folder(f) => f.frame(), _ => None }
    }
}

/// Child index (bottom to top) of a template's rank `r` (top to bottom).
fn child_of(len: usize, r: usize) -> usize { len - 1 - r }

impl LayerStack {
    /// The current frame (from 1).
    pub fn time(&self) -> u32 { self.time }

    /// Go to frame `n`: every animation folder shows its frame there. A
    /// frame or cel chosen moves to its folder's frame shown there.
    pub fn set_time(&mut self, n: u32) {
        self.time = n.clamp(1, MAX_FRAME);
        if let Some((folder, Some(_))) = self.anim_of(self.active) {
            self.active = self.exposed(folder).unwrap_or(folder);
        }
        self.settle();
    }

    /// The animation folder `id` is in or is, and the frame it is or is
    /// in (None: it is the folder itself).
    pub fn anim_of(&self, id: u64) -> Option<(u64, Option<u64>)> {
        let path = self.path(id)?;
        let mut nodes = &self.root;
        let mut found = None;
        for &i in &path {
            let n = &nodes[i];
            if n.is_animation() { found = Some((n.id(), None)); }
            if let (Some((f, None)), Some(_)) = (found, n.frame()) { found = Some((f, Some(n.id()))); }
            match n { Node::Folder(f) => nodes = &f.children, _ => break }
        }
        found
    }

    /// The frames of animation folder `folder`: (number, id), in order.
    pub fn frames(&self, folder: u64) -> Vec<(u32, u64)> {
        let Some(Node::Folder(f)) = self.node(folder) else { return Vec::new() };
        let mut v: Vec<(u32, u64)> = f.children.iter().filter_map(|c| Some((c.frame()?, c.id()))).collect();
        v.sort_unstable();
        v
    }

    /// The frame of `folder` drawn at number `n`.
    pub fn frame_at(&self, folder: u64, n: u32) -> Option<u64> {
        self.frames(folder).into_iter().find(|f| f.0 == n).map(|f| f.1)
    }

    /// The frame `folder` shows now: the last one at or before the
    /// current frame (None before its first one).
    pub fn exposed(&self, folder: u64) -> Option<u64> {
        self.frames(folder).into_iter().rev().find(|f| f.0 <= self.time).map(|f| f.1)
    }

    /// The template of the animation folder `id` is in (or is).
    pub fn anim(&self, id: u64) -> Option<&Anim> {
        let (folder, _) = self.anim_of(id)?;
        match self.node(folder)? { Node::Folder(f) => f.anim(), _ => None }
    }

    fn anim_mut(&mut self, folder: u64) -> Option<&mut Anim> {
        match self.node_mut(folder)? {
            Node::Folder(Folder { kind: FolderKind::Animation(a), .. }) => Some(a),
            _ => None,
        }
    }

    /// After any change of the tree or of the time: each animation folder
    /// shows only its frame of now; the active node is a row of the layer
    /// list (a frame, not one of its cels).
    pub(crate) fn settle(&mut self) {
        fn walk(nodes: &mut [Node], time: u32) {
            for n in nodes {
                let Node::Folder(f) = n else { continue };
                if f.anim().is_some() {
                    let shown = f.children.iter().filter_map(|c| c.frame()).filter(|k| *k <= time).max();
                    for c in &mut f.children {
                        if let Some(k) = c.frame() { *c.visible_mut() = Some(k) == shown; }
                    }
                } else {
                    walk(&mut f.children, time);
                }
            }
        }
        walk(&mut self.root, self.time);
        self.prune_empty_groups();
        if let Some((_, Some(frame))) = self.anim_of(self.active) { self.active = frame; }
    }

    /// A group left without animation folders goes away (the active node
    /// moves to the first layer if it was that group).
    fn prune_empty_groups(&mut self) {
        fn walk(nodes: &mut Vec<Node>) {
            for n in nodes.iter_mut() {
                if let Node::Folder(f) = n { if f.frame().is_none() { walk(&mut f.children); } }
            }
            nodes.retain(|n| !matches!(n, Node::Folder(f) if f.kind == FolderKind::Group && f.children.is_empty()));
        }
        walk(&mut self.root);
        if self.path(self.active).is_none() { self.active = first_layer_id(&self.root).unwrap_or(0); }
    }

    /// Where a stroke on the active node goes, or None if it can't be
    /// painted (a folder, a filter layer, locked). In an animation folder:
    /// its frame drawn at the current frame, else a new one there (also
    /// over a held frame), on the folder's cel.
    pub fn paint_target(&self) -> Option<Target> {
        let a = self.active;
        match self.anim_of(a) {
            None => (matches!(self.node(a)?, Node::Layer(_)) && !self.is_locked(a)).then_some(Target::Layer(a)),
            Some((folder, _)) => {
                let anim = self.anim(folder)?;
                let rank = anim.cel.min(anim.template.len().checked_sub(1)?);
                match self.frame_at(folder, self.time) {
                    Some(frame) => {
                        let Some(Node::Folder(f)) = self.node(frame) else { return None };
                        let cel = f.children.get(child_of(f.children.len(), rank))?.id();
                        (!self.is_locked(cel)).then_some(Target::Layer(cel))
                    }
                    None => (!self.is_locked(folder) && !anim.template[rank].locked)
                        .then_some(Target::NewFrame { folder, frame: self.time }),
                }
            }
        }
    }

    /// The paint layer edits (clear, filters, transform...) go to: the
    /// active layer, or the cel of the frame drawn now. None: nothing
    /// to edit there (a frame not drawn yet, a folder, locked).
    pub fn paint_id(&self) -> Option<u64> {
        match self.paint_target()? { Target::Layer(id) => Some(id), Target::NewFrame { .. } => None }
    }

    /// The paint layer the active row stands for, to read from (copy, the
    /// wand): a layer, a frame's chosen cel, or for an animation folder
    /// the one of the frame it shows. Locks and time don't matter.
    pub fn layer_of_active(&self) -> Option<u64> {
        let a = self.active;
        let frame = match self.node(a)? {
            Node::Layer(_) => return Some(a),
            n if n.is_animation() => self.exposed(a)?,
            n if n.frame().is_some() => a,
            _ => return None,
        };
        let rank = self.anim(frame)?.cel;
        let Some(Node::Folder(f)) = self.node(frame) else { return None };
        f.children.get(child_of(f.children.len(), rank.min(f.children.len().checked_sub(1)?))).map(Node::id)
    }

    /// A new animation folder with the default template, where a new
    /// node goes (never inside another animation folder); made active.
    pub fn add_animation_folder(&mut self, name: &str) -> u64 {
        let id = self.new_id();
        let folder = Folder {
            id, name: name.into(), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: true, children: Vec::new(), props: Props::default(),
            kind: FolderKind::Animation(Anim::default()),
        };
        self.insert(Node::Folder(folder))
    }

    /// A new animation folder in group `group` (the view of that group is
    /// open), else outside any group, whatever the active node is in.
    pub fn add_animation_folder_in(&mut self, name: &str, group: Option<u64>) -> u64 {
        let id = self.add_animation_folder(name);
        match (group, self.group_of(id)) {
            (Some(g), now) if now != Some(g) => { self.move_node(id, g, crate::Place::Inside); }
            (None, Some(g)) => { self.move_node(id, g, crate::Place::Above); }
            _ => {}
        }
        self.set_active(id);
        id
    }

    /// Make frame `n` of animation folder `folder` with its template's
    /// cels; it becomes active. Returns the cel strokes go to.
    /// None if the folder isn't one, or `n` is taken or out of range.
    pub fn make_frame(&mut self, folder: u64, n: u32) -> Option<u64> {
        if !(1..=MAX_FRAME).contains(&n) || self.frame_at(folder, n).is_some() { return None; }
        let anim = self.anim(folder)?.clone();
        if self.anim_of(folder)? != (folder, None) || anim.template.is_empty() { return None; }
        let frame = self.new_id();
        let mut cels = Vec::with_capacity(anim.template.len());
        for s in anim.template.iter().rev() {
            let mut l = Layer::new(self.width, self.height, false);
            l.id = self.new_id();
            l.name = s.name.clone();
            l.visible = s.visible;
            l.locked = s.locked;
            l.props.reference = s.reference;
            cels.push(Node::Layer(l));
        }
        let target = cels[child_of(cels.len(), anim.cel.min(cels.len() - 1))].id();
        let node = Node::Folder(Folder {
            id: frame, name: format!("Frame {n}"), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: true, children: cels, props: Props::default(), kind: FolderKind::Frame(n),
        });
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return None };
        // Kept from the last frame (bottom) to the first (top of the list).
        let at = f.children.iter().position(|c| c.frame().is_some_and(|k| k < n)).unwrap_or(f.children.len());
        f.children.insert(at, node);
        self.active = frame;
        self.settle();
        Some(target)
    }

    /// Strokes of animation folder `folder` go to cel `rank`.
    pub fn set_cel(&mut self, folder: u64, rank: usize) {
        if let Some(a) = self.anim_mut(folder) {
            if rank < a.template.len() { a.cel = rank; }
        }
    }

    /// Every frame of `folder`, mutably (its cels bottom to top).
    fn each_frame(&mut self, folder: u64, mut f: impl FnMut(&mut Vec<Node>)) {
        let Some(Node::Folder(fo)) = self.node_mut(folder) else { return };
        for c in &mut fo.children {
            if let Node::Folder(fr) = c { if fr.frame().is_some() { f(&mut fr.children); } }
        }
    }

    /// Add cel `cel` to the template at rank `at` (top to bottom), and
    /// to every frame.
    pub fn add_cel(&mut self, folder: u64, at: usize, cel: Cel) {
        let (w, h) = (self.width, self.height);
        let Some(a) = self.anim_mut(folder) else { return };
        let at = at.min(a.template.len());
        let len = a.template.len();
        a.template.insert(at, cel.clone());
        if a.cel >= at && len > 0 { a.cel += 1; }
        let mut ids = Vec::new();
        self.each_frame(folder, |_| ids.push(()));
        let new_ids: Vec<u64> = ids.iter().map(|_| self.new_id()).collect();
        let mut k = 0;
        self.each_frame(folder, |children| {
            let mut l = Layer::new(w, h, false);
            l.id = new_ids[k];
            k += 1;
            (l.name, l.visible, l.locked) = (cel.name.clone(), cel.visible, cel.locked);
            l.props.reference = cel.reference;
            children.insert(len - at, Node::Layer(l));
        });
        if cel.reference { self.set_cel_reference(folder, at, true); }
        self.settle();
    }

    /// Make cel rank `at` the reference of every frame (the one that was
    /// isn't any more), or none. Only the Colo reads it: the fill and the
    /// gap close look at the reference's lines.
    pub fn set_cel_reference(&mut self, folder: u64, at: usize, on: bool) {
        let Some(a) = self.anim_mut(folder) else { return };
        let len = a.template.len();
        if at >= len { return; }
        for (r, c) in a.template.iter_mut().enumerate() {
            if r == at { c.reference = on; } else if on { c.reference = false; }
        }
        let flags: Vec<bool> = a.template.iter().map(|c| c.reference).collect();
        self.each_frame(folder, |children| {
            if children.len() != len { return; }
            for (r, f) in flags.iter().enumerate() {
                children[child_of(len, r)].props_mut().reference = *f;
            }
        });
    }

    /// What deleting cel rank `at` would take away: the frames it is in,
    /// and how many of them have something drawn on it.
    pub fn cel_content(&self, folder: u64, at: usize) -> (usize, usize) {
        let Some(Node::Folder(f)) = self.node(folder) else { return (0, 0) };
        let len = match f.anim() { Some(a) => a.template.len(), None => return (0, 0) };
        let (mut frames, mut drawn) = (0, 0);
        for c in &f.children {
            let Node::Folder(fr) = c else { continue };
            if fr.frame().is_none() || fr.children.len() != len { continue; }
            frames += 1;
            if let Node::Layer(l) = &fr.children[child_of(len, at.min(len - 1))] {
                if l.tiles.visible_bounds().is_some() { drawn += 1; }
            }
        }
        (frames, drawn)
    }

    /// Take cel rank `at` out of the template and of every frame (the
    /// history keeps their pixels). Not the last one.
    pub fn remove_cel(&mut self, folder: u64, at: usize) -> bool {
        let Some(a) = self.anim_mut(folder) else { return false };
        let len = a.template.len();
        if len < 2 || at >= len { return false; }
        a.template.remove(at);
        if a.cel > at || a.cel >= len - 1 { a.cel = a.cel.saturating_sub(1); }
        let mut gone = Vec::new();
        self.each_frame(folder, |children| {
            if children.len() == len { gone.push(children.remove(child_of(len, at))); }
        });
        self.removed.extend(gone);
        self.settle();
        true
    }

    /// Rename cel rank `at`, in the template and every frame.
    pub fn rename_cel(&mut self, folder: u64, at: usize, name: &str) {
        let Some(a) = self.anim_mut(folder) else { return };
        let len = a.template.len();
        let Some(s) = a.template.get_mut(at) else { return };
        s.name = name.into();
        self.each_frame(folder, |children| {
            if let Some(c) = children.get_mut(child_of(len, at)) { *c.name_mut() = name.into(); }
        });
    }

    /// Move cel rank `from` to rank `to`, in the template and every
    /// frame (the chosen cel stays the same one).
    pub fn move_cel(&mut self, folder: u64, from: usize, to: usize) {
        let Some(a) = self.anim_mut(folder) else { return };
        let len = a.template.len();
        if from >= len || to >= len || from == to { return; }
        let chosen = a.template[a.cel].name.clone();
        let s = a.template.remove(from);
        a.template.insert(to, s);
        a.cel = a.template.iter().position(|s| s.name == chosen).unwrap_or(0);
        self.each_frame(folder, |children| {
            if children.len() != len { return; }
            let c = children.remove(child_of(len, from));
            children.insert(child_of(len, to), c);
        });
        self.settle();
    }

    /// Give every frame of `frame`'s folder (and the template, for the
    /// frames to come) the visibility and locks of `frame`'s cels.
    pub fn apply_to_folder(&mut self, frame: u64) {
        let Some((folder, Some(_))) = self.anim_of(frame) else { return };
        let Some(Node::Folder(f)) = self.node(frame) else { return };
        let states: Vec<(bool, bool)> = f.children.iter().map(|c| (c.visible(), c.locked())).collect();
        let len = states.len();
        let Some(a) = self.anim_mut(folder) else { return };
        if a.template.len() != len { return; }
        for (r, s) in a.template.iter_mut().enumerate() {
            (s.visible, s.locked) = states[child_of(len, r)];
        }
        self.each_frame(folder, |children| {
            if children.len() != len { return; }
            for (c, (v, l)) in children.iter_mut().zip(&states) {
                *c.visible_mut() = *v;
                *c.locked_mut() = *l;
            }
        });
    }

    /// The animation folders, bottom to top (the X-sheet's columns).
    pub fn animation_folders(&self) -> Vec<u64> {
        fn walk(nodes: &[Node], out: &mut Vec<u64>) {
            for n in nodes {
                if n.is_animation() { out.push(n.id()); } else if let Node::Folder(f) = n { if f.frame().is_none() { walk(&f.children, out); } }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }

    /// The last frame drawn in any animation folder (1 if none): where the
    /// scene ends.
    pub fn last_frame(&self) -> u32 {
        self.animation_folders().iter().filter_map(|f| self.frames(*f).last().map(|x| x.0)).max().unwrap_or(1)
    }

    /// Renumber frame `id` to `n` (its name follows), keeping the folder's
    /// frames in order.
    fn renumber(&mut self, folder: u64, moves: &[(u64, u32)]) {
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return };
        for c in &mut f.children {
            let Node::Folder(fr) = c else { continue };
            if let Some(&(_, n)) = moves.iter().find(|m| m.0 == fr.id) {
                fr.kind = FolderKind::Frame(n);
                fr.name = format!("Frame {n}");
            }
        }
        // Last frame at the bottom, first at the top of the list.
        f.children.sort_by_key(|c| std::cmp::Reverse(c.frame().unwrap_or(0)));
    }

    /// Move the frame drawn at `from` to `to` (the X-sheet's drag). A
    /// frame already at `to`: `Replace` takes it away (kept for undo),
    /// `Swap` puts it at `from`; `Refuse` changes nothing. True if moved.
    pub fn move_frame(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool {
        if from == to || !(1..=MAX_FRAME).contains(&to) { return false; }
        let Some(src) = self.frame_at(folder, from) else { return false };
        match (self.frame_at(folder, to), onto) {
            (Some(_), Onto::Refuse) => return false,
            (Some(dst), Onto::Replace) => { self.take_frame(dst); self.renumber(folder, &[(src, to)]); }
            (Some(dst), Onto::Swap) => self.renumber(folder, &[(src, to), (dst, from)]),
            (None, _) => self.renumber(folder, &[(src, to)]),
        }
        self.active = src;
        self.settle();
        true
    }

    /// A copy of the frame drawn at `from` (its cels and pixels), at
    /// `to` (Ctrl + drag); `onto` as for `move_frame`, Swap meaning
    /// Replace. True if copied.
    pub fn copy_frame(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool {
        if from == to || !(1..=MAX_FRAME).contains(&to) { return false; }
        let Some(src) = self.frame_at(folder, from) else { return false };
        if let Some(dst) = self.frame_at(folder, to) {
            if onto == Onto::Refuse { return false; }
            self.take_frame(dst);
        }
        let Some(original) = self.node(src) else { return false };
        let original = clone_shallow(original);
        let mut copy = self.clone_node(&original);
        // One reference per folder: the copy isn't one (as Duplicate).
        copy.props_mut().reference = false;
        let id = copy.id();
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return false };
        f.children.push(copy);
        self.renumber(folder, &[(id, to)]);
        self.active = id;
        self.settle();
        true
    }

    /// Frame `id` may go: a paint layer stays in the document.
    fn can_take(&self, id: u64) -> bool {
        let Some(Node::Folder(f)) = self.node(id) else { return false };
        self.layer_count() > crate::count_layers(&f.children)
    }

    /// Take frame `id` out (kept for undo); active, it gives way to its
    /// folder.
    fn take_frame(&mut self, id: u64) {
        let folder = self.anim_of(id).map(|x| x.0);
        let Some(path) = self.path(id) else { return };
        let (last, parent) = path.split_last().unwrap();
        let gone = crate::siblings_mut(&mut self.root, parent).remove(*last);
        self.removed.push(gone);
        if self.path(self.active).is_none() { if let Some(f) = folder { self.active = f; } }
    }

    /// Delete the frame drawn at `n` (the one before then holds on).
    pub fn delete_frame(&mut self, folder: u64, n: u32) -> bool {
        let Some(id) = self.frame_at(folder, n) else { return false };
        if self.is_locked(id) || !self.can_take(id) { return false; }
        self.take_frame(id);
        if self.anim_of(self.active).is_none_or(|(f, _)| f == folder) { self.active = self.exposed(folder).unwrap_or(folder); }
        self.settle();
        true
    }

    /// Timing: `Insert` pushes the frames from `at` one later (the frame
    /// before holds one more); `Remove` takes the frame drawn at `at` away
    /// (if any) and brings the following ones one earlier. Frames are
    /// renamed. False if nothing changed.
    pub fn shift_frames(&mut self, folder: u64, at: u32, edit: Shift) -> bool {
        let frames = self.frames(folder);
        let moves: Vec<(u64, u32)> = match edit {
            Shift::Insert => {
                if frames.last().is_some_and(|f| f.0 >= MAX_FRAME) { return false; }
                frames.iter().filter(|f| f.0 >= at).map(|f| (f.1, f.0 + 1)).collect()
            }
            Shift::Remove => {
                if let Some(id) = self.frame_at(folder, at) {
                    if self.is_locked(id) || !self.can_take(id) { return false; }
                    self.take_frame(id);
                }
                frames.iter().filter(|f| f.0 > at).map(|f| (f.1, f.0 - 1)).collect()
            }
        };
        let changed = !moves.is_empty() || edit == Shift::Remove && frames.iter().any(|f| f.0 == at);
        self.renumber(folder, &moves);
        if self.anim_of(self.active).is_none() || self.node(self.active).is_none() { self.active = folder; }
        self.settle();
        changed
    }

    /// `id` is a frame or a cel: it stays where it is (the X-sheet
    /// moves frames; the template decides the cels).
    pub fn is_anim_part(&self, id: u64) -> bool {
        matches!(self.anim_of(id), Some((_, Some(_))))
    }

    /// Check the animation folders: only frames in them, numbers unique,
    /// every frame with its template's cels (in order, by name), the
    /// frame of now shown. Err: what is wrong (tests, fuzz).
    pub fn check_animation(&self) -> Result<(), String> {
        fn walk(s: &LayerStack, nodes: &[Node], in_anim: bool, in_group: bool) -> Result<(), String> {
            for n in nodes {
                if in_group && !n.is_animation() { return Err(format!("node {} in a group is no animation folder", n.id())); }
                if n.frame().is_some() != in_anim { return Err(format!("node {} {} an animation folder", n.id(), if in_anim { "in" } else { "a frame outside" })); }
                let Node::Folder(f) = n else { continue };
                if let Some(a) = f.anim() {
                    if a.template.is_empty() || a.cel >= a.template.len() { return Err(format!("folder {}: template", f.id)); }
                    let mut seen = std::collections::HashSet::new();
                    for c in &f.children {
                        let Node::Folder(fr) = c else { return Err(format!("folder {}: a layer among frames", f.id)) };
                        let k = fr.frame().ok_or("a folder among frames")?;
                        if !seen.insert(k) { return Err(format!("folder {}: frame {k} twice", f.id)); }
                        let names: Vec<&str> = fr.children.iter().rev().map(|c| c.name()).collect();
                        let want: Vec<&str> = a.template.iter().map(|t| t.name.as_str()).collect();
                        if names != want || !fr.children.iter().all(|c| matches!(c, Node::Layer(_))) {
                            return Err(format!("frame {k}: cels {names:?}, template {want:?}"));
                        }
                        let marks: Vec<bool> = fr.children.iter().rev().map(|c| c.props().reference).collect();
                        let want_marks: Vec<bool> = a.template.iter().map(|t| t.reference).collect();
                        if marks != want_marks { return Err(format!("frame {k}: reference marks {marks:?}, template {want_marks:?}")); }
                        let shown = s.frames(f.id).into_iter().rev().find(|x| x.0 <= s.time).map(|x| x.1);
                        if c.visible() != (shown == Some(c.id())) { return Err(format!("frame {k}: shown {} at {}", c.visible(), s.time)); }
                    }
                } else if f.frame().is_none() {
                    if f.kind == FolderKind::Group {
                        if in_group { return Err(format!("group {} in a group", f.id)); }
                        if f.children.is_empty() { return Err(format!("group {} is empty", f.id)); }
                    }
                    walk(s, &f.children, false, f.kind == FolderKind::Group)?;
                }
            }
            Ok(())
        }
        walk(self, &self.root, false, false)?;
        if self.anim_of(self.active).is_some_and(|(_, fr)| fr.is_some_and(|fr| fr != self.active)) {
            return Err("a cel is active".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Place;

    fn names(s: &LayerStack, frame: u64) -> Vec<String> {
        let Some(Node::Folder(f)) = s.node(frame) else { panic!() };
        f.children.iter().rev().map(|c| c.name().to_owned()).collect()
    }

    /// A folder made, frames drawn at 1 and 5: each with the template's
    /// cels; 1 held until 5; nothing before; the active stays a row.
    #[test]
    fn frames_follow_the_template_and_hold() {
        let mut s = LayerStack::new_document(64, 64);
        let a = s.add_animation_folder("Perso A");
        assert_eq!(s.paint_target(), Some(Target::NewFrame { folder: a, frame: 1 }));
        assert_eq!(s.paint_id(), None);
        let cel = s.make_frame(a, 1).unwrap();
        let f1 = s.active_id();
        assert_eq!(s.node(f1).unwrap().name(), "Frame 1");
        assert_eq!(names(&s, f1), ["Colo", "Ink", "Douga", "Genga"]);
        assert_eq!(s.node(cel).unwrap().name(), "Genga");
        assert_eq!(s.paint_target(), Some(Target::Layer(cel)));
        // A cel made active: its frame is.
        s.set_active(cel);
        assert_eq!(s.active_id(), f1);
        s.set_time(5);
        assert_eq!(s.exposed(a), Some(f1), "held");
        assert_eq!(s.paint_target(), Some(Target::NewFrame { folder: a, frame: 5 }), "a new frame over a hold");
        s.set_cel(a, 2);
        let douga = s.make_frame(a, 5).unwrap();
        assert_eq!(s.node(douga).unwrap().name(), "Douga");
        let f5 = s.active_id();
        assert!(s.node(f5).unwrap().visible() && !s.node(f1).unwrap().visible());
        for t in [1, 4] { s.set_time(t); assert_eq!(s.exposed(a), Some(f1)); }
        s.set_time(9);
        assert_eq!(s.exposed(a), Some(f5));
        assert_eq!(s.frames(a), vec![(1, f1), (5, f5)]);
        // In the list: the folder, then Frame 1, Frame 5; no cels.
        if let Some(Node::Folder(f)) = s.node_mut(a) { f.collapsed = false; }
        let rows = s.rows();
        let at = rows.iter().position(|r| r.id == a).unwrap();
        assert!(rows[at].animation);
        assert_eq!((rows[at + 1].frame, rows[at + 2].frame), (Some(1), Some(5)));
        assert!(rows.iter().all(|r| r.id != douga));
        s.check_animation().unwrap();
        // Taken twice: no.
        assert!(s.make_frame(a, 5).is_none());
    }

    /// Nothing else goes into an animation folder; frames and cels
    /// can't be moved, grouped, duplicated (cels deleted) from the
    /// layer list.
    #[test]
    fn frames_stay_in_their_folder() {
        let mut s = LayerStack::new_document(32, 32);
        let ink = s.active_id();
        let a = s.add_animation_folder("A");
        s.make_frame(a, 1).unwrap();
        let f1 = s.active_id();
        // A new layer with a frame active: above the folder, outside.
        let l = s.add_layer("L");
        assert!(s.anim_of(l).is_none());
        assert!(!s.move_node(ink, a, Place::Inside));
        assert!(!s.move_node(ink, f1, Place::Above));
        assert!(!s.move_node(f1, ink, Place::Above));
        assert!(!s.move_nodes(&[ink, l], ink, f1, Place::Below));
        assert!(s.move_node(ink, a, Place::Above), "next to it is fine");
        s.set_active(f1);
        assert_eq!(s.group_active("G"), f1);
        assert_eq!(s.duplicate_active(), f1);
        s.move_active(true);
        assert_eq!(s.frames(a), vec![(1, f1)]);
        s.set_active(a);
        assert!(!s.ungroup_active());
        s.check_animation().unwrap();
        // The frame may be deleted; then the folder is empty.
        s.set_active(f1);
        assert!(s.remove_active());
        assert!(s.frames(a).is_empty());
        s.check_animation().unwrap();
    }

    /// Template edits go to every frame; a locked cel isn't painted,
    /// nor made by a new frame; apply to folder copies one frame's states.
    #[test]
    fn template_edits_reach_every_frame() {
        let mut s = LayerStack::new_document(32, 32);
        let a = s.add_animation_folder("A");
        let f: Vec<u64> = (1..=3).map(|n| { s.set_time(n * 2); s.make_frame(a, n * 2).unwrap(); s.active_id() }).collect();
        s.set_cel(a, 0);
        s.add_cel(a, 1, Cel::new("Layout", false));
        s.rename_cel(a, 0, "Key");
        s.move_cel(a, 4, 0);
        for &fr in &f { assert_eq!(names(&s, fr), ["Genga", "Key", "Layout", "Ink", "Douga"]); }
        assert_eq!(s.anim(a).unwrap().template[s.anim(a).unwrap().cel].name, "Key", "the chosen one follows");
        assert!(s.remove_cel(a, 2));
        for &fr in &f { assert_eq!(names(&s, fr), ["Genga", "Key", "Ink", "Douga"]); }
        assert_eq!(s.take_removed().len(), 3, "kept for undo");
        s.check_animation().unwrap();
        // Lock "Key" in frame 2, apply to the folder.
        let key = |s: &LayerStack, fr: u64| { let Some(Node::Folder(x)) = s.node(fr) else { panic!() }; x.children[2].id() };
        let k = key(&s, f[0]);
        *s.node_mut(k).unwrap().locked_mut() = true;
        s.apply_to_folder(f[0]);
        for &fr in &f { assert!(s.node(key(&s, fr)).unwrap().locked()); }
        s.set_time(6);
        s.set_active(f[2]);
        s.set_cel(a, 1);
        assert_eq!(s.paint_target(), None, "locked");
        s.set_time(7);
        assert_eq!(s.paint_target(), None, "a new frame would start locked");
        s.set_cel(a, 2);
        assert!(matches!(s.paint_target(), Some(Target::NewFrame { frame: 7, .. })));
        // The last cel can't go.
        for _ in 0..3 { assert!(s.remove_cel(a, 0)); }
        assert!(!s.remove_cel(a, 0));
        s.check_animation().unwrap();
    }

    /// Undo of the layer tree brings frames back with their cels and
    /// shows the right one; the cel chosen is not undone.
    #[test]
    fn tree_undo_keeps_frames_whole() {
        let mut s = LayerStack::new_document(32, 32);
        let a = s.add_animation_folder("A");
        let before = s.tree_state();
        s.make_frame(a, 1).unwrap();
        s.set_cel(a, 2);
        let after = s.tree_state();
        assert_ne!(before, after);
        let kept = s.restore_tree(&before, Vec::new());
        assert!(s.frames(a).is_empty());
        s.restore_tree(&after, kept);
        assert_eq!(s.frames(a).len(), 1);
        assert_eq!(s.anim(a).unwrap().cel, 2);
        s.check_animation().unwrap();
        // Choosing a cel is no edit.
        let t = s.tree_state();
        s.set_cel(a, 0);
        assert_eq!(s.tree_state(), t);
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    /// Folder with frames drawn at `ns`, each marked by a pixel of value n.
    fn folder_with(ns: &[u32]) -> (LayerStack, u64) {
        let mut s = LayerStack::new_document(8, 8);
        let a = s.add_animation_folder("A");
        for &n in ns {
            let cel = s.make_frame(a, n).unwrap();
            s.set_layer_rect(cel, (0, 0, 1, 1), &[n as u8, 0, 0, 255]);
        }
        (s, a)
    }

    /// The frame numbers, and the mark each frame carries.
    fn marks(s: &LayerStack, a: u64) -> Vec<(u32, u8)> {
        s.frames(a).into_iter().map(|(n, id)| {
            let Some(Node::Folder(f)) = s.node(id) else { panic!() };
            assert_eq!(f.name, format!("Frame {n}"));
            let mark = f.children.iter().find_map(|c| match c { Node::Layer(l) => Some(l.tiles.pixel(0, 0)[0]).filter(|v| *v != 255), _ => None }).unwrap_or(0);
            (n, mark)
        }).collect()
    }

    #[test]
    fn moving_a_frame() {
        let (mut s, a) = folder_with(&[1, 3, 5]);
        assert!(s.move_frame(a, 3, 4, Onto::Refuse), "onto an empty frame: no question");
        assert_eq!(marks(&s, a), vec![(1, 1), (4, 3), (5, 5)]);
        assert!(!s.move_frame(a, 4, 5, Onto::Refuse));
        assert!(s.move_frame(a, 4, 5, Onto::Swap));
        assert_eq!(marks(&s, a), vec![(1, 1), (4, 5), (5, 3)]);
        assert!(s.move_frame(a, 1, 4, Onto::Replace));
        assert_eq!(marks(&s, a), vec![(4, 1), (5, 3)]);
        assert_eq!(s.take_removed().len(), 1, "the replaced one kept for undo");
        s.check_animation().unwrap();
    }

    #[test]
    fn copying_a_frame() {
        let (mut s, a) = folder_with(&[1, 2]);
        assert!(s.copy_frame(a, 1, 6, Onto::Refuse));
        assert_eq!(marks(&s, a), vec![(1, 1), (2, 2), (6, 1)]);
        assert!(!s.copy_frame(a, 2, 6, Onto::Refuse));
        assert!(s.copy_frame(a, 2, 6, Onto::Replace));
        assert_eq!(marks(&s, a), vec![(1, 1), (2, 2), (6, 2)]);
        // A copy: painting it leaves the original as it was.
        let six = s.frame_at(a, 6).unwrap();
        let Some(Node::Folder(f)) = s.node(six) else { panic!() };
        let cel = f.children.last().unwrap().id();
        s.set_layer_rect(cel, (0, 0, 1, 1), &[99, 0, 0, 255]);
        assert_eq!(marks(&s, a)[1], (2, 2));
        // A reference frame's copy isn't a second reference.
        let one = s.frame_at(a, 1).unwrap();
        s.node_mut(one).unwrap().props_mut().reference = true;
        assert!(s.copy_frame(a, 1, 8, Onto::Refuse));
        let Some(Node::Folder(f)) = s.node(a) else { panic!() };
        assert_eq!(f.children.iter().filter(|n| n.props().reference).count(), 1);
        s.check_animation().unwrap();
    }

    /// Frames 1 to 10 drawn, insert at 2: the new gap is 2 (the frame 1
    /// holds there), the old 2 to 10 are 3 to 11; remove at 2 brings them
    /// back; remove where a frame is drawn deletes it.
    #[test]
    fn inserting_and_removing_frames() {
        let ns: Vec<u32> = (1..=10).collect();
        let (mut s, a) = folder_with(&ns);
        assert!(s.shift_frames(a, 2, Shift::Insert));
        let m = marks(&s, a);
        assert_eq!(m[0], (1, 1));
        assert_eq!(m[1..], (3..=11).map(|n| (n, (n - 1) as u8)).collect::<Vec<_>>()[..]);
        s.set_time(2);
        assert_eq!(s.exposed(a), s.frame_at(a, 1), "the gap holds frame 1");
        assert!(s.shift_frames(a, 2, Shift::Remove));
        assert_eq!(marks(&s, a), (1..=10).map(|n| (n, n as u8)).collect::<Vec<_>>());
        assert!(s.shift_frames(a, 5, Shift::Remove));
        assert_eq!(marks(&s, a).len(), 9);
        assert_eq!(marks(&s, a)[4], (5, 6));
        assert!(!s.shift_frames(a, 50, Shift::Insert), "nothing after it");
        s.check_animation().unwrap();
        assert_eq!(s.last_frame(), 9);
        assert!(s.delete_frame(a, 9));
        assert_eq!(s.last_frame(), 8);
        s.check_animation().unwrap();
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::Place;

    /// Paper, then animation folders A, B, C (bottom to top).
    fn doc() -> (LayerStack, u64, u64, u64) {
        let mut s = LayerStack::new_document(16, 16);
        let a = s.add_animation_folder("A");
        let b = s.add_animation_folder("B");
        let c = s.add_animation_folder("C");
        (s, a, b, c)
    }

    /// A new folder goes in the group only while the group is open; out of
    /// it (or from the Layers docker) it is made outside, wherever the
    /// active folder is.
    #[test]
    fn new_folders_leave_a_group_unless_it_is_open() {
        let (mut s, a, b, _c) = doc();
        let g = s.group_animation_folders(&[a, b], "Hero").unwrap();
        s.set_active(a);
        let out = s.add_animation_folder_in("Out", None);
        assert_eq!(s.group_of(out), None);
        assert!(s.columns(None).contains(&Column::Folder(out)));
        s.set_active(a);
        let inside = s.add_animation_folder_in("In", Some(g));
        assert_eq!(s.group_of(inside), Some(g));
        // Active in the open group, but the group isn't (view closed).
        s.set_active(inside);
        let again = s.add_animation_folder_in("Again", None);
        assert_eq!(s.group_of(again), None);
        assert_eq!(s.group_members(g).len(), 3);
        s.check_animation().unwrap();
    }

    /// Animation folders go in a group where the topmost was: one entry in
    /// the timeline, the folders inside when it is opened.
    #[test]
    fn a_group_holds_animation_folders() {
        let (mut s, a, b, c) = doc();
        let g = s.group_animation_folders(&[a, c], "Hero").unwrap();
        assert_eq!(s.columns(None), vec![Column::Folder(b), Column::Group(g)]);
        assert_eq!(s.columns(Some(g)), vec![Column::Folder(a), Column::Folder(c)]);
        assert_eq!(s.group_of(a), Some(g));
        assert_eq!(s.group_of(b), None);
        assert!(matches!(s.node(g), Some(Node::Folder(f)) if f.collapsed && f.kind == FolderKind::Group));
        // Everything still counts as animation folders (playback, last frame).
        assert_eq!(s.animation_folders(), vec![b, a, c]);
        // Something that is not an animation folder isn't taken.
        assert!(s.group_animation_folders(&[s.rows().last().unwrap().id], "x").is_none());
        s.check_animation().unwrap();
    }

    /// One level only: a group never goes in a group, and only animation
    /// folders go in one, whatever the way (drag, new layer, group).
    #[test]
    fn groups_do_not_nest() {
        let (mut s, a, b, c) = doc();
        let g = s.group_animation_folders(&[a, b], "G").unwrap();
        let h = s.group_animation_folders(&[c], "H").unwrap();
        assert!(!s.move_node(h, g, Place::Inside), "a group into a group");
        assert!(!s.move_node(h, a, Place::Above), "next to a folder of a group");
        let paper = s.rows().last().unwrap().id;
        assert!(!s.move_node(paper, g, Place::Inside), "a layer into a group");
        assert!(!s.move_node(paper, a, Place::Below), "a layer among a group's folders");
        // Grouping again takes the folder out of its group.
        let k = s.group_animation_folders(&[a], "K").unwrap();
        assert_eq!(s.group_of(a), Some(k));
        assert_eq!(s.columns(Some(g)), vec![Column::Folder(b)]);
        // A new layer made with a folder of a group active goes out of it.
        s.set_active(b);
        let l = s.add_layer("L");
        assert!(s.group_of(l).is_none() && !s.in_group(l));
        // A new animation folder with one active goes into its group.
        s.set_active(b);
        let n = s.add_animation_folder("N");
        assert_eq!(s.group_of(n), Some(g));
        s.check_animation().unwrap();
    }

    /// Drag in and out: a folder dropped on a group is put in it, taken
    /// out it lands beside; a group left empty goes away.
    #[test]
    fn folders_go_in_and_out_of_a_group() {
        let (mut s, a, b, c) = doc();
        let g = s.group_animation_folders(&[a], "G").unwrap();
        assert!(s.move_node(b, g, Place::Inside));
        assert_eq!(s.columns(Some(g)), vec![Column::Folder(a), Column::Folder(b)]);
        assert!(s.move_node(a, c, Place::Above));
        assert_eq!(s.group_of(a), None);
        assert!(s.move_node(b, c, Place::Above));
        assert!(s.node(g).is_none(), "the empty group is gone");
        s.check_animation().unwrap();
        // Ungroup puts the folders back where the group was.
        let g = s.group_animation_folders(&[a, b], "G").unwrap();
        s.set_active(g);
        assert!(s.ungroup_active());
        assert_eq!(s.group_of(a), None);
        assert!(s.node(g).is_none());
        s.check_animation().unwrap();
    }

    /// The cel locks of a frame are phase locks (Apply to folder), not a
    /// protection of the frame: a frame or a folder is deleted anyway.
    #[test]
    fn locked_cels_do_not_keep_a_frame_or_a_folder() {
        let (mut s, a, ..) = doc();
        for n in 1..=3 { s.set_time(n); s.make_frame(a, n).unwrap(); }
        let f1 = s.frame_at(a, 1).unwrap();
        // Genga unlocked, every other cel locked, in every frame.
        let Some(Node::Folder(fr)) = s.node(f1) else { panic!() };
        let ids: Vec<u64> = fr.children.iter().map(Node::id).collect();
        for (k, id) in ids.iter().enumerate() { *s.node_mut(*id).unwrap().locked_mut() = k != 0; }
        s.apply_to_folder(f1);
        s.set_active(f1);
        assert_eq!(s.removal_refused(), None);
        assert!(s.remove_active());
        assert_eq!(s.frames(a).len(), 2);
        // The folder with its locked cels too.
        s.set_active(a);
        assert_eq!(s.removal_refused(), None);
        assert!(s.remove_active());
        assert!(s.node(a).is_none());
        // But the folder's own lock still protects it.
        let b = s.add_animation_folder("B");
        *s.node_mut(b).unwrap().locked_mut() = true;
        s.set_active(b);
        assert!(s.removal_refused().is_some());
        s.check_animation().unwrap();
    }

    /// Deleting a cel takes it from every frame: the docker asks first
    /// when some of them have a drawing.
    #[test]
    fn a_cel_counts_what_deleting_it_would_take() {
        let (mut s, a, ..) = doc();
        for n in 1..=3 { s.set_time(n); s.make_frame(a, n).unwrap(); }
        assert_eq!(s.cel_content(a, 1), (3, 0), "nothing drawn: no question");
        let ink_of = |s: &LayerStack, n: u32| { let Some(Node::Folder(f)) = s.node(s.frame_at(a, n).unwrap()) else { panic!() }; f.children[2].id() };
        s.set_layer_rect(ink_of(&s, 1), (0, 0, 1, 1), &[9, 0, 0, 255]);
        s.set_layer_rect(ink_of(&s, 3), (0, 0, 1, 1), &[9, 0, 0, 255]);
        assert_eq!(s.cel_content(a, 1), (3, 2));
        assert_eq!(s.cel_content(a, 0), (3, 0));
    }

    /// The reference of a frame is its Ink cel, set by the template for
    /// every frame, and moved or cleared there, never by hand.
    #[test]
    fn the_ink_cel_is_the_reference_of_every_frame() {
        let (mut s, a, ..) = doc();
        let marks = |s: &LayerStack, n: u32| -> Vec<(String, bool)> {
            let Some(Node::Folder(f)) = s.node(s.frame_at(a, n).unwrap()) else { panic!() };
            f.children.iter().rev().map(|c| (c.name().to_owned(), c.props().reference)).collect()
        };
        for n in [1, 4] { s.set_time(n); s.make_frame(a, n).unwrap(); }
        for n in [1, 4] { assert_eq!(marks(&s, n)[1], ("Ink".to_owned(), true)); assert_eq!(marks(&s, n).iter().filter(|m| m.1).count(), 1); }
        // The Colo's fill looks at the Ink of its frame.
        let Some(Node::Folder(f)) = s.node(s.frame_at(a, 4).unwrap()) else { panic!() };
        let (colo, ink) = (f.children[3].id(), f.children[2].id());
        assert_eq!(s.reference_for(colo), Some(ink));
        // Another cel as the reference: the mark moves, in every frame.
        s.set_cel_reference(a, 2, true);
        for n in [1, 4] { assert_eq!(marks(&s, n)[2], ("Douga".to_owned(), true)); assert_eq!(marks(&s, n).iter().filter(|m| m.1).count(), 1); }
        s.set_cel_reference(a, 2, false);
        for n in [1, 4] { assert!(marks(&s, n).iter().all(|m| !m.1)); }
        // A new frame and a new cel follow the template.
        s.set_cel_reference(a, 1, true);
        s.set_time(8);
        s.make_frame(a, 8).unwrap();
        assert_eq!(marks(&s, 8)[1], ("Ink".to_owned(), true));
        s.add_cel(a, 0, Cel { reference: true, ..Cel::new("Lines", false) });
        for n in [1, 4, 8] { assert_eq!(marks(&s, n)[0], ("Lines".to_owned(), true)); assert_eq!(marks(&s, n).iter().filter(|m| m.1).count(), 1); }
        s.check_animation().unwrap();
    }

    /// Several frames move together, in order, as one set.
    #[test]
    fn several_frames_move_together() {
        let (mut s, a, ..) = doc();
        for n in [1, 2, 3, 6] { s.set_time(n); s.make_frame(a, n).unwrap(); }
        let ns = |s: &LayerStack| s.frames(a).iter().map(|f| f.0).collect::<Vec<_>>();
        assert!(s.move_frames(a, &[1, 2, 3], 1, false, Onto::Refuse), "frame 4 is free: the set shifts");
        assert_eq!(ns(&s), vec![2, 3, 4, 6]);
        assert!(!s.move_frames(a, &[4], 2, false, Onto::Refuse), "onto frame 6");
        assert!(!s.move_frames(a, &[2, 3], -2, false, Onto::Refuse), "before frame 1");
        assert!(s.move_frames(a, &[3, 4], 2, false, Onto::Replace), "replaces 6");
        assert_eq!(ns(&s), vec![2, 5, 6]);
        assert!(s.move_frames(a, &[2, 5], 10, true, Onto::Refuse), "a copy");
        assert_eq!(ns(&s), vec![2, 5, 6, 12, 15]);
        s.check_animation().unwrap();
    }

    /// An Animation document starts with its paper (locked) and an
    /// animation folder, no layer.
    #[test]
    fn an_animation_document_starts_with_a_folder() {
        let mut s = LayerStack::new_document(16, 16);
        s.start_animation();
        let rows = s.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].animation);
        assert_eq!(s.node(rows[1].id).unwrap().name(), "Paper");
        assert!(s.is_locked(rows[1].id));
        assert!(matches!(s.node(rows[0].id), Some(Node::Folder(f)) if f.collapsed));
        assert_eq!(s.active_id(), rows[0].id);
        assert_eq!(s.paint_target(), Some(Target::NewFrame { folder: rows[0].id, frame: 1 }));
        s.check_animation().unwrap();
    }
}

//! Animation folders. One folder per element (a character, smoke...),
//! one child per drawn frame ("Frame N", its place in time), each frame
//! holding the sub-cels of the folder's cel template (genga, douga, ink,
//! colo...) as paint layers. Only one frame of a folder shows at a time:
//! the last one drawn at or before the current frame (a hold), carried by
//! the frames' visibility so that every merge stays as it is.
//!
//! The template is the same for every frame of a folder: locks, sub-cel
//! choice and (later) the onion skin go by a sub-cel's rank, so a douga is
//! never shown or painted among gengas.

use crate::{Folder, FolderBlend, Layer, LayerStack, Node, Props};

/// What a folder is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FolderKind {
    #[default]
    Plain,
    /// An animation folder: its frames follow `Anim`'s template.
    Animation(Anim),
    /// One frame of an animation folder (its number, from 1): its
    /// children are the sub-cels, bottom to top.
    Frame(u32),
}

/// An animation folder's cel template and the sub-cel painted in.
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    /// Top to bottom, as listed.
    pub template: Vec<SubCel>,
    /// Rank (in `template`) of the sub-cel strokes go to, in every frame.
    pub sub:      usize,
}

/// One sub-cel of a template: its name, and what a new frame starts with.
#[derive(Clone, Debug, PartialEq)]
pub struct SubCel {
    pub name:    String,
    /// The coloring sub-cel (palette colors, later).
    pub colo:    bool,
    pub visible: bool,
    pub locked:  bool,
    /// Shown by the onion skin (later).
    pub onion:   bool,
}

impl SubCel {
    pub fn new(name: &str, colo: bool) -> Self {
        Self { name: name.into(), colo, visible: true, locked: false, onion: true }
    }
}

impl Default for Anim {
    /// Genga, douga, ink, colo; strokes go to the genga first.
    fn default() -> Self {
        let template = ["Genga", "Douga", "Ink"].iter().map(|n| SubCel::new(n, false)).chain([SubCel::new("Colo", true)]).collect();
        Self { template, sub: 0 }
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
    /// frame or sub-cel chosen moves to its folder's frame shown there.
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
    /// list (a frame, not one of its sub-cels).
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
        if let Some((_, Some(frame))) = self.anim_of(self.active) { self.active = frame; }
    }

    /// Where a stroke on the active node goes, or None if it can't be
    /// painted (a folder, a filter layer, locked). In an animation folder:
    /// its frame drawn at the current frame, else a new one there (also
    /// over a held frame), on the folder's sub-cel.
    pub fn paint_target(&self) -> Option<Target> {
        let a = self.active;
        match self.anim_of(a) {
            None => (matches!(self.node(a)?, Node::Layer(_)) && !self.is_locked(a)).then_some(Target::Layer(a)),
            Some((folder, _)) => {
                let anim = self.anim(folder)?;
                let rank = anim.sub.min(anim.template.len().checked_sub(1)?);
                match self.frame_at(folder, self.time) {
                    Some(frame) => {
                        let Some(Node::Folder(f)) = self.node(frame) else { return None };
                        let sub = f.children.get(child_of(f.children.len(), rank))?.id();
                        (!self.is_locked(sub)).then_some(Target::Layer(sub))
                    }
                    None => (!self.is_locked(folder) && !anim.template[rank].locked)
                        .then_some(Target::NewFrame { folder, frame: self.time }),
                }
            }
        }
    }

    /// The paint layer edits (clear, filters, transform...) go to: the
    /// active layer, or the sub-cel of the frame drawn now. None: nothing
    /// to edit there (a frame not drawn yet, a folder, locked).
    pub fn paint_id(&self) -> Option<u64> {
        match self.paint_target()? { Target::Layer(id) => Some(id), Target::NewFrame { .. } => None }
    }

    /// The paint layer the active row stands for, to read from (copy, the
    /// wand): a layer, a frame's chosen sub-cel, or for an animation folder
    /// the one of the frame it shows. Locks and time don't matter.
    pub fn layer_of_active(&self) -> Option<u64> {
        let a = self.active;
        let frame = match self.node(a)? {
            Node::Layer(_) => return Some(a),
            n if n.is_animation() => self.exposed(a)?,
            n if n.frame().is_some() => a,
            _ => return None,
        };
        let rank = self.anim(frame)?.sub;
        let Some(Node::Folder(f)) = self.node(frame) else { return None };
        f.children.get(child_of(f.children.len(), rank.min(f.children.len().checked_sub(1)?))).map(Node::id)
    }

    /// A new animation folder with the default template, where a new
    /// node goes (never inside another animation folder); made active.
    pub fn add_animation_folder(&mut self, name: &str) -> u64 {
        let id = self.new_id();
        let folder = Folder {
            id, name: name.into(), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: false, children: Vec::new(), props: Props::default(),
            kind: FolderKind::Animation(Anim::default()),
        };
        self.insert(Node::Folder(folder))
    }

    /// Make frame `n` of animation folder `folder` with its template's
    /// sub-cels; it becomes active. Returns the sub-cel strokes go to.
    /// None if the folder isn't one, or `n` is taken or out of range.
    pub fn make_frame(&mut self, folder: u64, n: u32) -> Option<u64> {
        if !(1..=MAX_FRAME).contains(&n) || self.frame_at(folder, n).is_some() { return None; }
        let anim = self.anim(folder)?.clone();
        if self.anim_of(folder)? != (folder, None) || anim.template.is_empty() { return None; }
        let frame = self.new_id();
        let mut subs = Vec::with_capacity(anim.template.len());
        for s in anim.template.iter().rev() {
            let mut l = Layer::new(self.width, self.height, false);
            l.id = self.new_id();
            l.name = s.name.clone();
            l.visible = s.visible;
            l.locked = s.locked;
            subs.push(Node::Layer(l));
        }
        let target = subs[child_of(subs.len(), anim.sub.min(subs.len() - 1))].id();
        let node = Node::Folder(Folder {
            id: frame, name: format!("Frame {n}"), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: true, children: subs, props: Props::default(), kind: FolderKind::Frame(n),
        });
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return None };
        // Kept from the last frame (bottom) to the first (top of the list).
        let at = f.children.iter().position(|c| c.frame().is_some_and(|k| k < n)).unwrap_or(f.children.len());
        f.children.insert(at, node);
        f.collapsed = false;
        self.active = frame;
        self.settle();
        Some(target)
    }

    /// Strokes of animation folder `folder` go to sub-cel `rank`.
    pub fn set_sub(&mut self, folder: u64, rank: usize) {
        if let Some(a) = self.anim_mut(folder) {
            if rank < a.template.len() { a.sub = rank; }
        }
    }

    /// Every frame of `folder`, mutably (its sub-cels bottom to top).
    fn each_frame(&mut self, folder: u64, mut f: impl FnMut(&mut Vec<Node>)) {
        let Some(Node::Folder(fo)) = self.node_mut(folder) else { return };
        for c in &mut fo.children {
            if let Node::Folder(fr) = c { if fr.frame().is_some() { f(&mut fr.children); } }
        }
    }

    /// Add sub-cel `sub` to the template at rank `at` (top to bottom), and
    /// to every frame.
    pub fn add_sub(&mut self, folder: u64, at: usize, sub: SubCel) {
        let (w, h) = (self.width, self.height);
        let Some(a) = self.anim_mut(folder) else { return };
        let at = at.min(a.template.len());
        let len = a.template.len();
        a.template.insert(at, sub.clone());
        if a.sub >= at && len > 0 { a.sub += 1; }
        let mut ids = Vec::new();
        self.each_frame(folder, |_| ids.push(()));
        let new_ids: Vec<u64> = ids.iter().map(|_| self.new_id()).collect();
        let mut k = 0;
        self.each_frame(folder, |children| {
            let mut l = Layer::new(w, h, false);
            l.id = new_ids[k];
            k += 1;
            (l.name, l.visible, l.locked) = (sub.name.clone(), sub.visible, sub.locked);
            children.insert(len - at, Node::Layer(l));
        });
        self.settle();
    }

    /// Take sub-cel rank `at` out of the template and of every frame (the
    /// history keeps their pixels). Not the last one.
    pub fn remove_sub(&mut self, folder: u64, at: usize) -> bool {
        let Some(a) = self.anim_mut(folder) else { return false };
        let len = a.template.len();
        if len < 2 || at >= len { return false; }
        a.template.remove(at);
        if a.sub > at || a.sub >= len - 1 { a.sub = a.sub.saturating_sub(1); }
        let mut gone = Vec::new();
        self.each_frame(folder, |children| {
            if children.len() == len { gone.push(children.remove(child_of(len, at))); }
        });
        self.removed.extend(gone);
        self.settle();
        true
    }

    /// Rename sub-cel rank `at`, in the template and every frame.
    pub fn rename_sub(&mut self, folder: u64, at: usize, name: &str) {
        let Some(a) = self.anim_mut(folder) else { return };
        let len = a.template.len();
        let Some(s) = a.template.get_mut(at) else { return };
        s.name = name.into();
        self.each_frame(folder, |children| {
            if let Some(c) = children.get_mut(child_of(len, at)) { *c.name_mut() = name.into(); }
        });
    }

    /// Move sub-cel rank `from` to rank `to`, in the template and every
    /// frame (the chosen sub-cel stays the same one).
    pub fn move_sub(&mut self, folder: u64, from: usize, to: usize) {
        let Some(a) = self.anim_mut(folder) else { return };
        let len = a.template.len();
        if from >= len || to >= len || from == to { return; }
        let chosen = a.template[a.sub].name.clone();
        let s = a.template.remove(from);
        a.template.insert(to, s);
        a.sub = a.template.iter().position(|s| s.name == chosen).unwrap_or(0);
        self.each_frame(folder, |children| {
            if children.len() != len { return; }
            let c = children.remove(child_of(len, from));
            children.insert(child_of(len, to), c);
        });
        self.settle();
    }

    /// Give every frame of `frame`'s folder (and the template, for the
    /// frames to come) the visibility and locks of `frame`'s sub-cels.
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

    /// A copy of the frame drawn at `from` (its sub-cels and pixels), at
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
        let copy = self.clone_node(&original);
        let id = copy.id();
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return false };
        f.children.push(copy);
        self.renumber(folder, &[(id, to)]);
        self.active = id;
        self.settle();
        true
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
        if self.is_locked(id) { return false; }
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
                    if self.is_locked(id) { return false; }
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

    /// `id` is a frame or a sub-cel: it stays where it is (the X-sheet
    /// moves frames; the template decides the sub-cels).
    pub fn is_anim_part(&self, id: u64) -> bool {
        matches!(self.anim_of(id), Some((_, Some(_))))
    }

    /// Check the animation folders: only frames in them, numbers unique,
    /// every frame with its template's sub-cels (in order, by name), the
    /// frame of now shown. Err: what is wrong (tests, fuzz).
    pub fn check_animation(&self) -> Result<(), String> {
        fn walk(s: &LayerStack, nodes: &[Node], in_anim: bool) -> Result<(), String> {
            for n in nodes {
                if n.frame().is_some() != in_anim { return Err(format!("node {} {} an animation folder", n.id(), if in_anim { "in" } else { "a frame outside" })); }
                let Node::Folder(f) = n else { continue };
                if let Some(a) = f.anim() {
                    if a.template.is_empty() || a.sub >= a.template.len() { return Err(format!("folder {}: template", f.id)); }
                    let mut seen = std::collections::HashSet::new();
                    for c in &f.children {
                        let Node::Folder(fr) = c else { return Err(format!("folder {}: a layer among frames", f.id)) };
                        let k = fr.frame().ok_or("a folder among frames")?;
                        if !seen.insert(k) { return Err(format!("folder {}: frame {k} twice", f.id)); }
                        let names: Vec<&str> = fr.children.iter().rev().map(|c| c.name()).collect();
                        let want: Vec<&str> = a.template.iter().map(|t| t.name.as_str()).collect();
                        if names != want || !fr.children.iter().all(|c| matches!(c, Node::Layer(_))) {
                            return Err(format!("frame {k}: sub-cels {names:?}, template {want:?}"));
                        }
                        let shown = s.frames(f.id).into_iter().rev().find(|x| x.0 <= s.time).map(|x| x.1);
                        if c.visible() != (shown == Some(c.id())) { return Err(format!("frame {k}: shown {} at {}", c.visible(), s.time)); }
                    }
                } else if f.frame().is_none() {
                    walk(s, &f.children, false)?;
                }
            }
            Ok(())
        }
        walk(self, &self.root, false)?;
        if self.anim_of(self.active).is_some_and(|(_, fr)| fr.is_some_and(|fr| fr != self.active)) {
            return Err("a sub-cel is active".into());
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
    /// sub-cels; 1 held until 5; nothing before; the active stays a row.
    #[test]
    fn frames_follow_the_template_and_hold() {
        let mut s = LayerStack::new_document(64, 64);
        let a = s.add_animation_folder("Perso A");
        assert_eq!(s.paint_target(), Some(Target::NewFrame { folder: a, frame: 1 }));
        assert_eq!(s.paint_id(), None);
        let sub = s.make_frame(a, 1).unwrap();
        let f1 = s.active_id();
        assert_eq!(s.node(f1).unwrap().name(), "Frame 1");
        assert_eq!(names(&s, f1), ["Genga", "Douga", "Ink", "Colo"]);
        assert_eq!(s.node(sub).unwrap().name(), "Genga");
        assert_eq!(s.paint_target(), Some(Target::Layer(sub)));
        // A sub-cel made active: its frame is.
        s.set_active(sub);
        assert_eq!(s.active_id(), f1);
        s.set_time(5);
        assert_eq!(s.exposed(a), Some(f1), "held");
        assert_eq!(s.paint_target(), Some(Target::NewFrame { folder: a, frame: 5 }), "a new frame over a hold");
        s.set_sub(a, 1);
        let douga = s.make_frame(a, 5).unwrap();
        assert_eq!(s.node(douga).unwrap().name(), "Douga");
        let f5 = s.active_id();
        assert!(s.node(f5).unwrap().visible() && !s.node(f1).unwrap().visible());
        for t in [1, 4] { s.set_time(t); assert_eq!(s.exposed(a), Some(f1)); }
        s.set_time(9);
        assert_eq!(s.exposed(a), Some(f5));
        assert_eq!(s.frames(a), vec![(1, f1), (5, f5)]);
        // In the list: the folder, then Frame 1, Frame 5; no sub-cels.
        let rows = s.rows();
        let at = rows.iter().position(|r| r.id == a).unwrap();
        assert!(rows[at].animation);
        assert_eq!((rows[at + 1].frame, rows[at + 2].frame), (Some(1), Some(5)));
        assert!(rows.iter().all(|r| r.id != douga));
        s.check_animation().unwrap();
        // Taken twice: no.
        assert!(s.make_frame(a, 5).is_none());
    }

    /// Nothing else goes into an animation folder; frames and sub-cels
    /// can't be moved, grouped, duplicated (sub-cels deleted) from the
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

    /// Template edits go to every frame; a locked sub-cel isn't painted,
    /// nor made by a new frame; apply to folder copies one frame's states.
    #[test]
    fn template_edits_reach_every_frame() {
        let mut s = LayerStack::new_document(32, 32);
        let a = s.add_animation_folder("A");
        let f: Vec<u64> = (1..=3).map(|n| { s.set_time(n * 2); s.make_frame(a, n * 2).unwrap(); s.active_id() }).collect();
        s.add_sub(a, 1, SubCel::new("Layout", false));
        s.rename_sub(a, 0, "Key");
        s.move_sub(a, 4, 0);
        for &fr in &f { assert_eq!(names(&s, fr), ["Colo", "Key", "Layout", "Douga", "Ink"]); }
        assert_eq!(s.anim(a).unwrap().template[s.anim(a).unwrap().sub].name, "Key", "the chosen one follows");
        assert!(s.remove_sub(a, 2));
        for &fr in &f { assert_eq!(names(&s, fr), ["Colo", "Key", "Douga", "Ink"]); }
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
        s.set_sub(a, 1);
        assert_eq!(s.paint_target(), None, "locked");
        s.set_time(7);
        assert_eq!(s.paint_target(), None, "a new frame would start locked");
        s.set_sub(a, 2);
        assert!(matches!(s.paint_target(), Some(Target::NewFrame { frame: 7, .. })));
        // The last sub-cel can't go.
        for _ in 0..3 { assert!(s.remove_sub(a, 0)); }
        assert!(!s.remove_sub(a, 0));
        s.check_animation().unwrap();
    }

    /// Undo of the layer tree brings frames back with their sub-cels and
    /// shows the right one; the sub-cel chosen is not undone.
    #[test]
    fn tree_undo_keeps_frames_whole() {
        let mut s = LayerStack::new_document(32, 32);
        let a = s.add_animation_folder("A");
        let before = s.tree_state();
        s.make_frame(a, 1).unwrap();
        s.set_sub(a, 2);
        let after = s.tree_state();
        assert_ne!(before, after);
        let kept = s.restore_tree(&before, Vec::new());
        assert!(s.frames(a).is_empty());
        s.restore_tree(&after, kept);
        assert_eq!(s.frames(a).len(), 1);
        assert_eq!(s.anim(a).unwrap().sub, 2);
        s.check_animation().unwrap();
        // Choosing a sub-cel is no edit.
        let t = s.tree_state();
        s.set_sub(a, 0);
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
            let sub = s.make_frame(a, n).unwrap();
            s.set_layer_rect(sub, (0, 0, 1, 1), &[n as u8, 0, 0, 255]);
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
        let sub = f.children.last().unwrap().id();
        s.set_layer_rect(sub, (0, 0, 1, 1), &[99, 0, 0, 255]);
        assert_eq!(marks(&s, a)[1], (2, 2));
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

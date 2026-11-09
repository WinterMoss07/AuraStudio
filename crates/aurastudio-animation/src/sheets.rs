//! Reading and editing animation folders: sheets in time order, cel
//! templates, groups, timing. Edits change the tree only; the document
//! settles it with its `Timeline` afterwards (visibility by time, names,
//! the active row).

use aurastudio_core::Rect;
use aurastudio_layers::{Anim, Cel, Column, Folder, FolderKind, Layer, LayerStack, Node, Onto, Place, Shift, MAX_FRAME};

/// Child index (bottom to top) of a template's rank `r` (top to bottom).
pub(crate) fn child_of(len: usize, r: usize) -> usize { len - 1 - r }

fn count_layers(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| match n { Node::Layer(_) => 1, Node::Folder(f) => count_layers(&f.children), _ => 0 }).sum()
}

/// Reading the animation folders of a tree.
pub trait Sheets {
    /// The sheets of animation folder `folder`: (frame exposed from, id),
    /// in time order.
    fn sheets(&self, folder: u64) -> Vec<(u32, u64)>;
    /// The sheet of `folder` exposed from frame `n`.
    fn sheet_at(&self, folder: u64, n: u32) -> Option<u64>;
    /// The sheet `folder` shows at frame `t`: the last one exposed at or
    /// before it (None before its first one).
    fn shown_at(&self, folder: u64, t: u32) -> Option<u64>;
    /// The template of the animation folder `id` is in (or is).
    fn anim(&self, id: u64) -> Option<&Anim>;
    /// The animation folders, bottom to top (the X-sheet's columns).
    fn animation_folders(&self) -> Vec<u64>;
    /// What the timeline lists: inside group `group`, its folders; at the
    /// top (None), the groups and the animation folders not in one.
    fn columns(&self, group: Option<u64>) -> Vec<Column>;
    fn group_members(&self, id: u64) -> Vec<u64>;
    /// The group an animation folder is in.
    fn group_of(&self, id: u64) -> Option<u64>;
    /// The last frame a sheet is exposed from, in any animation folder (1
    /// if none): where the scene ends.
    fn last_frame(&self) -> u32;
    /// What deleting cel rank `at` would take away: the sheets it is in,
    /// and how many of them have something drawn on it.
    fn cel_content(&self, folder: u64, at: usize) -> (usize, usize);
    /// Where `id` (a layer, or a folder's layers) can show pixels.
    fn extent(&self, id: u64) -> Option<Rect>;
    /// Check the animation folders against frame `t`: only sheets in
    /// them, one per frame, named in time order, every sheet with its
    /// template's cels, the sheet of `t` shown. Err: what is wrong.
    fn check_animation(&self, t: u32) -> Result<(), String>;
}

impl Sheets for LayerStack {
    fn sheets(&self, folder: u64) -> Vec<(u32, u64)> {
        let Some(Node::Folder(f)) = self.node(folder) else { return Vec::new() };
        let mut v: Vec<(u32, u64)> = f.children.iter().filter_map(|c| Some((c.sheet_start()?, c.id()))).collect();
        v.sort_unstable();
        v
    }

    fn sheet_at(&self, folder: u64, n: u32) -> Option<u64> { self.sheets(folder).into_iter().find(|f| f.0 == n).map(|f| f.1) }

    fn shown_at(&self, folder: u64, t: u32) -> Option<u64> {
        let s = self.sheets(folder);
        let i = s.partition_point(|x| x.0 <= t);
        i.checked_sub(1).map(|i| s[i].1)
    }

    fn anim(&self, id: u64) -> Option<&Anim> {
        let (folder, _) = self.anim_of(id)?;
        self.node(folder)?.as_folder()?.anim()
    }

    fn animation_folders(&self) -> Vec<u64> {
        fn walk(nodes: &[Node], out: &mut Vec<u64>) {
            for n in nodes {
                if n.is_animation() { out.push(n.id()); } else if let Node::Folder(f) = n { if f.sheet_start().is_none() { walk(&f.children, out); } }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }

    fn columns(&self, group: Option<u64>) -> Vec<Column> {
        fn walk(nodes: &[Node], out: &mut Vec<Column>) {
            for n in nodes {
                let Node::Folder(f) = n else { continue };
                match f.kind {
                    FolderKind::Group => out.push(Column::Group(f.id)),
                    FolderKind::Animation(_) => out.push(Column::Folder(f.id)),
                    FolderKind::Plain => walk(&f.children, out),
                    FolderKind::Sheet(_) => {}
                }
            }
        }
        let mut v = Vec::new();
        match group {
            None => walk(&self.root, &mut v),
            Some(g) => if let Some(Node::Folder(f)) = self.node(g) {
                if f.is_group() { for n in &f.children { if n.is_animation() { v.push(Column::Folder(n.id())); } } }
            },
        }
        v
    }

    fn group_members(&self, id: u64) -> Vec<u64> { self.columns(Some(id)).into_iter().map(Column::id).collect() }

    fn group_of(&self, id: u64) -> Option<u64> {
        let mut p = self.parent_of(id);
        while let Some(f) = p {
            if self.node(f).is_some_and(Node::is_group) { return Some(f); }
            p = self.parent_of(f);
        }
        None
    }

    fn last_frame(&self) -> u32 {
        self.animation_folders().iter().filter_map(|f| self.sheets(*f).last().map(|x| x.0)).max().unwrap_or(1)
    }

    fn cel_content(&self, folder: u64, at: usize) -> (usize, usize) {
        let Some(Node::Folder(f)) = self.node(folder) else { return (0, 0) };
        let len = match f.anim() { Some(a) => a.template.len(), None => return (0, 0) };
        let (mut frames, mut drawn) = (0, 0);
        for c in &f.children {
            let Node::Folder(fr) = c else { continue };
            if fr.sheet_start().is_none() || fr.children.len() != len { continue; }
            frames += 1;
            if let Node::Layer(l) = &fr.children[child_of(len, at.min(len - 1))] {
                if l.surface.visible_bounds().is_some() { drawn += 1; }
            }
        }
        (frames, drawn)
    }

    fn extent(&self, id: u64) -> Option<Rect> {
        match self.node(id)?.footprint() {
            aurastudio_layers::Footprint::Nothing => None,
            aurastudio_layers::Footprint::Area(r) => Some(r),
            aurastudio_layers::Footprint::All => Some(crate::EVERYWHERE),
        }
    }

    fn check_animation(&self, t: u32) -> Result<(), String> {
        fn walk(s: &LayerStack, nodes: &[Node], in_anim: bool, in_group: bool, t: u32) -> Result<(), String> {
            for n in nodes {
                if in_group && !n.is_animation() { return Err(format!("node {} in a group is no animation folder", n.id())); }
                if n.sheet_start().is_some() != in_anim { return Err(format!("node {} {} an animation folder", n.id(), if in_anim { "in" } else { "a sheet outside" })); }
                let Node::Folder(f) = n else { continue };
                if let Some(a) = f.anim() {
                    if a.template.is_empty() || a.cel >= a.template.len() { return Err(format!("folder {}: template", f.id)); }
                    let sheets = s.sheets(f.id);
                    let mut seen = std::collections::HashSet::new();
                    for c in &f.children {
                        let Node::Folder(fr) = c else { return Err(format!("folder {}: a layer among sheets", f.id)) };
                        let k = fr.sheet_start().ok_or("a folder among sheets")?;
                        if !seen.insert(k) { return Err(format!("folder {}: two sheets at frame {k}", f.id)); }
                        let rank = sheets.iter().position(|x| x.1 == fr.id).map_or(0, |i| i + 1);
                        if fr.name != format!("Sheet {rank}") { return Err(format!("sheet at {k} named {:?}, rank {rank}", fr.name)); }
                        let names: Vec<&str> = fr.children.iter().rev().map(Node::name).collect();
                        let want: Vec<&str> = a.template.iter().map(|t| t.name.as_str()).collect();
                        if names != want || !fr.children.iter().all(|c| matches!(c, Node::Layer(_))) {
                            return Err(format!("sheet at {k}: cels {names:?}, template {want:?}"));
                        }
                        let marks: Vec<bool> = fr.children.iter().rev().map(|c| c.props().reference).collect();
                        let want_marks: Vec<bool> = a.template.iter().map(|t| t.reference).collect();
                        if marks != want_marks { return Err(format!("sheet at {k}: reference marks {marks:?}, template {want_marks:?}")); }
                        let shown = sheets.iter().rev().find(|x| x.0 <= t).map(|x| x.1);
                        if c.visible() != (shown == Some(c.id())) { return Err(format!("sheet at {k}: shown {} at {t}", c.visible())); }
                    }
                } else if f.sheet_start().is_none() {
                    if f.is_group() {
                        if in_group { return Err(format!("group {} in a group", f.id)); }
                        if f.children.is_empty() { return Err(format!("group {} is empty", f.id)); }
                    }
                    walk(s, &f.children, false, f.is_group(), t)?;
                }
            }
            Ok(())
        }
        walk(self, &self.root, false, false, t)?;
        if self.anim_of(self.active_id()).is_some_and(|(_, fr)| fr.is_some_and(|fr| fr != self.active_id())) { return Err("a cel is active".into()); }
        Ok(())
    }
}

/// Editing the animation folders of a tree.
pub trait SheetEdit {
    /// A new animation folder with the default template, where a new node
    /// goes (never inside another animation folder); made active.
    fn add_animation_folder(&mut self, name: &str) -> u64;
    /// The same in group `group` (its view is open), else outside any
    /// group, whatever the active node is in.
    fn add_animation_folder_in(&mut self, name: &str, group: Option<u64>) -> u64;
    /// A document that starts as an animation: the paper, then an
    /// animation folder, nothing else.
    fn start_animation(&mut self);
    /// Put animation folders `ids` in a new group where the topmost was,
    /// keeping their order (folded, active). `None` if there is none.
    fn group_animation_folders(&mut self, ids: &[u64], name: &str) -> Option<u64>;
    /// A sheet of `folder` exposed from frame `n` with the template's cels;
    /// it becomes active. Returns the cel strokes go to.
    fn make_sheet(&mut self, folder: u64, n: u32) -> Option<u64>;
    /// Strokes of `folder` go to cel `rank`.
    fn set_cel(&mut self, folder: u64, rank: usize);
    /// Add cel `cel` to the template at rank `at` (top to bottom), and to
    /// every sheet.
    fn add_cel(&mut self, folder: u64, at: usize, cel: Cel);
    /// Eye and lock of template cel `at` (what sheets start with). True if
    /// one changed.
    fn set_template_state(&mut self, folder: u64, at: usize, visible: Option<bool>, locked: Option<bool>) -> bool;
    /// Make cel rank `at` the reference of every sheet (the Colo's fill
    /// reads it), or none.
    fn set_cel_reference(&mut self, folder: u64, at: usize, on: bool);
    /// Take cel rank `at` out of the template and every sheet (the history
    /// keeps their pixels). Not the last one.
    fn remove_cel(&mut self, folder: u64, at: usize) -> bool;
    fn rename_cel(&mut self, folder: u64, at: usize, name: &str);
    /// Move cel rank `from` to rank `to` everywhere (the chosen cel stays).
    fn move_cel(&mut self, folder: u64, from: usize, to: usize);
    /// Give every sheet of `sheet`'s folder (and the template) the
    /// visibility and locks of `sheet`'s cels.
    fn apply_to_folder(&mut self, sheet: u64);
    /// Move the sheet exposed from `from` to `to`; a sheet already there:
    /// `Replace` takes it away (kept for undo), `Swap` puts it at `from`,
    /// `Refuse` changes nothing. True if moved.
    fn move_sheet(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool;
    /// A copy of the sheet at `from` (cels and pixels) at `to`.
    fn copy_sheet(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool;
    /// Move (or copy) the sheets at `frames` by `delta`, kept in order;
    /// false if any would leave the frames, or land where `onto` refuses.
    fn move_sheets(&mut self, folder: u64, frames: &[u32], delta: i32, copy: bool, onto: Onto) -> bool;
    /// Delete the sheet exposed from `n` (the one before then holds on).
    fn delete_sheet(&mut self, folder: u64, n: u32) -> bool;
    /// `Insert` pushes the sheets from `at` one frame later; `Remove` takes
    /// the sheet at `at` away and brings the following ones one earlier.
    fn shift_frames(&mut self, folder: u64, at: u32, edit: Shift) -> bool;
    /// The sheets of every animation folder stacked as they come (the
    /// first at the bottom): a file from before, a sheet put back.
    fn stack_sheets(&mut self);
}

fn anim_mut(s: &mut LayerStack, folder: u64) -> Option<&mut Anim> {
    match s.node_mut(folder)? { Node::Folder(f) => f.anim_mut(), _ => None }
}

/// Every sheet of `folder`, mutably (its cels bottom to top).
fn each_sheet(s: &mut LayerStack, folder: u64, mut f: impl FnMut(&mut Vec<Node>)) {
    let Some(Node::Folder(fo)) = s.node_mut(folder) else { return };
    for c in &mut fo.children { if let Node::Folder(fr) = c { if fr.sheet_start().is_some() { f(&mut fr.children); } } }
}

/// Expose sheets from other frames (`moves`: id, frame), keeping the
/// folder's sheets in time order (their names follow at the settle).
fn renumber(s: &mut LayerStack, folder: u64, moves: &[(u64, u32)]) {
    let Some(Node::Folder(f)) = s.node_mut(folder) else { return };
    for c in &mut f.children {
        let Node::Folder(fr) = c else { continue };
        if let Some(&(_, n)) = moves.iter().find(|m| m.0 == fr.id) { fr.kind = FolderKind::Sheet(n); }
    }
    stack_in_order(&mut f.children);
}

/// Sheets stacked as they come: the first at the bottom, each next one
/// over it (on top of the list). Other children keep their place order.
pub(crate) fn stack_in_order(children: &mut [Node]) {
    children.sort_by_key(|c| c.sheet_start().unwrap_or(0));
}

/// Sheet `id` may go: a paint layer stays in the document.
fn can_take(s: &LayerStack, id: u64) -> bool {
    let Some(Node::Folder(f)) = s.node(id) else { return false };
    s.layer_count() > count_layers(&f.children)
}

/// Take sheet `id` out (kept for undo); active, it gives way to its folder.
fn take_sheet(s: &mut LayerStack, id: u64) {
    let folder = s.anim_of(id).map(|x| x.0);
    let was_active = s.active_id() == id;
    if let Some(n) = s.take_node(id) { s.keep_removed(n); }
    if was_active { if let Some(f) = folder { s.set_active(f); } }
}

impl SheetEdit for LayerStack {
    fn stack_sheets(&mut self) {
        for folder in self.animation_folders() {
            if let Some(Node::Folder(f)) = self.node_mut(folder) { stack_in_order(&mut f.children); }
        }
    }

    fn add_animation_folder(&mut self, name: &str) -> u64 {
        self.insert_folder(Folder { kind: FolderKind::Animation(Anim::default()), ..Folder::new(name) })
    }

    fn add_animation_folder_in(&mut self, name: &str, group: Option<u64>) -> u64 {
        let id = self.add_animation_folder(name);
        match (group, self.group_of(id)) {
            (Some(g), now) if now != Some(g) => { self.move_node(id, g, Place::Inside); }
            (None, Some(g)) => { self.move_node(id, g, Place::Above); }
            _ => {}
        }
        self.set_active(id);
        id
    }

    fn start_animation(&mut self) {
        let paper = self.root.drain(..).next();
        self.root.extend(paper);
        if let Some(id) = self.root.first().map(Node::id) { self.set_active(id); }
        self.add_animation_folder("Animation 1");
    }

    fn group_animation_folders(&mut self, ids: &[u64], name: &str) -> Option<u64> {
        // Listed top to bottom, those that are animation folders, once.
        fn walk(nodes: &[Node], ids: &[u64], out: &mut Vec<u64>) {
            for n in nodes.iter().rev() {
                if n.is_animation() && ids.contains(&n.id()) { out.push(n.id()); } else if let Node::Folder(f) = n { if f.sheet_start().is_none() { walk(&f.children, ids, out); } }
            }
        }
        let mut folders = Vec::new();
        walk(&self.root, ids, &mut folders);
        let &first = folders.first()?;
        // Right above the topmost (above its group: groups never nest).
        let anchor = self.group_of(first).unwrap_or(first);
        let (parent, index) = self.place_of(anchor)?;
        let id = self.new_id();
        let group = Folder { id, kind: FolderKind::Group, ..Folder::new(name) };
        self.add_node(Node::Folder(group), parent, index + 1);
        let mut taken: Vec<Node> = folders.iter().filter_map(|&f| self.take_node(f)).collect();
        taken.reverse();
        if let Some(Node::Folder(g)) = self.node_mut(id) { g.children = taken; }
        self.set_active(id);
        self.tidy();
        Some(id)
    }

    fn make_sheet(&mut self, folder: u64, n: u32) -> Option<u64> {
        if !(1..=MAX_FRAME).contains(&n) || self.sheet_at(folder, n).is_some() { return None; }
        if self.anim_of(folder)? != (folder, None) { return None; }
        let anim = self.anim(folder)?.clone();
        if anim.template.is_empty() { return None; }
        let (w, h, tile) = (self.width, self.height, self.tile);
        let mut cels = Vec::with_capacity(anim.template.len());
        for s in anim.template.iter().rev() {
            let mut l = Layer::new(w, h, tile, false);
            l.id = self.new_id();
            (l.name, l.visible, l.locked) = (s.name.clone(), s.visible, s.locked);
            l.props.reference = s.reference;
            cels.push(Node::Layer(l));
        }
        let target = cels[child_of(cels.len(), anim.cel.min(cels.len() - 1))].id();
        let sheet = self.new_id();
        let node = Node::Folder(Folder { id: sheet, name: String::new(), children: cels, kind: FolderKind::Sheet(n), ..Folder::new("") });
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return None };
        // Over the sheets before it, under those after (the first at the bottom).
        let at = f.children.iter().position(|c| c.sheet_start().is_some_and(|k| k > n)).unwrap_or(f.children.len());
        f.children.insert(at, node);
        self.set_active(sheet);
        Some(target)
    }

    fn set_cel(&mut self, folder: u64, rank: usize) {
        if let Some(a) = anim_mut(self, folder) { if rank < a.template.len() { a.cel = rank; } }
    }

    fn add_cel(&mut self, folder: u64, at: usize, cel: Cel) {
        let (w, h, tile) = (self.width, self.height, self.tile);
        let Some(a) = anim_mut(self, folder) else { return };
        let at = at.min(a.template.len());
        let len = a.template.len();
        a.template.insert(at, cel.clone());
        if a.cel >= at && len > 0 { a.cel += 1; }
        let mut n = 0;
        each_sheet(self, folder, |_| n += 1);
        let ids: Vec<u64> = (0..n).map(|_| self.new_id()).collect();
        let mut k = 0;
        each_sheet(self, folder, |children| {
            let mut l = Layer::new(w, h, tile, false);
            l.id = ids[k];
            k += 1;
            (l.name, l.visible, l.locked) = (cel.name.clone(), cel.visible, cel.locked);
            l.props.reference = cel.reference;
            children.insert(len - at, Node::Layer(l));
        });
        if cel.reference { self.set_cel_reference(folder, at, true); }
    }

    fn set_template_state(&mut self, folder: u64, at: usize, visible: Option<bool>, locked: Option<bool>) -> bool {
        let Some(t) = anim_mut(self, folder).and_then(|a| a.template.get_mut(at)) else { return false };
        let before = (t.visible, t.locked);
        if let Some(v) = visible { t.visible = v; }
        if let Some(l) = locked { t.locked = l; }
        before != (t.visible, t.locked)
    }

    fn set_cel_reference(&mut self, folder: u64, at: usize, on: bool) {
        let Some(a) = anim_mut(self, folder) else { return };
        let len = a.template.len();
        if at >= len { return; }
        for (r, c) in a.template.iter_mut().enumerate() { if r == at { c.reference = on; } else if on { c.reference = false; } }
        let flags: Vec<bool> = a.template.iter().map(|c| c.reference).collect();
        each_sheet(self, folder, |children| {
            if children.len() != len { return; }
            for (r, f) in flags.iter().enumerate() { children[child_of(len, r)].props_mut().reference = *f; }
        });
    }

    fn remove_cel(&mut self, folder: u64, at: usize) -> bool {
        let Some(a) = anim_mut(self, folder) else { return false };
        let len = a.template.len();
        if len < 2 || at >= len { return false; }
        a.template.remove(at);
        if a.cel > at || a.cel >= len - 1 { a.cel = a.cel.saturating_sub(1); }
        let mut gone = Vec::new();
        each_sheet(self, folder, |children| { if children.len() == len { gone.push(children.remove(child_of(len, at))); } });
        for g in gone { self.keep_removed(g); }
        true
    }

    fn rename_cel(&mut self, folder: u64, at: usize, name: &str) {
        let Some(a) = anim_mut(self, folder) else { return };
        let len = a.template.len();
        let Some(s) = a.template.get_mut(at) else { return };
        s.name = name.into();
        each_sheet(self, folder, |children| { if let Some(c) = children.get_mut(child_of(len, at)) { *c.name_mut() = name.into(); } });
    }

    fn move_cel(&mut self, folder: u64, from: usize, to: usize) {
        let Some(a) = anim_mut(self, folder) else { return };
        let len = a.template.len();
        if from >= len || to >= len || from == to { return; }
        let chosen = a.template[a.cel].name.clone();
        let s = a.template.remove(from);
        a.template.insert(to, s);
        a.cel = a.template.iter().position(|s| s.name == chosen).unwrap_or(0);
        each_sheet(self, folder, |children| {
            if children.len() != len { return; }
            let c = children.remove(child_of(len, from));
            children.insert(child_of(len, to), c);
        });
    }

    fn apply_to_folder(&mut self, sheet: u64) {
        let Some((folder, Some(_))) = self.anim_of(sheet) else { return };
        let Some(Node::Folder(f)) = self.node(sheet) else { return };
        let states: Vec<(bool, bool)> = f.children.iter().map(|c| (c.visible(), c.locked())).collect();
        let len = states.len();
        let Some(a) = anim_mut(self, folder) else { return };
        if a.template.len() != len { return; }
        for (r, s) in a.template.iter_mut().enumerate() { (s.visible, s.locked) = states[child_of(len, r)]; }
        each_sheet(self, folder, |children| {
            if children.len() != len { return; }
            for (c, (v, l)) in children.iter_mut().zip(&states) { *c.visible_mut() = *v; *c.locked_mut() = *l; }
        });
    }

    fn move_sheet(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool {
        if from == to || !(1..=MAX_FRAME).contains(&to) { return false; }
        let Some(src) = self.sheet_at(folder, from) else { return false };
        match (self.sheet_at(folder, to), onto) {
            (Some(_), Onto::Refuse) => return false,
            (Some(dst), Onto::Replace) => { take_sheet(self, dst); renumber(self, folder, &[(src, to)]); }
            (Some(dst), Onto::Swap) => renumber(self, folder, &[(src, to), (dst, from)]),
            (None, _) => renumber(self, folder, &[(src, to)]),
        }
        self.set_active(src);
        true
    }

    fn copy_sheet(&mut self, folder: u64, from: u32, to: u32, onto: Onto) -> bool {
        if from == to || !(1..=MAX_FRAME).contains(&to) { return false; }
        let Some(src) = self.sheet_at(folder, from) else { return false };
        if let Some(dst) = self.sheet_at(folder, to) {
            if onto == Onto::Refuse { return false; }
            take_sheet(self, dst);
        }
        let Some(original) = self.node(src).cloned() else { return false };
        let mut copy = self.clone_node(&original);
        // One reference per folder: the copy isn't one (as Duplicate).
        copy.props_mut().reference = false;
        let id = copy.id();
        let Some(Node::Folder(f)) = self.node_mut(folder) else { return false };
        f.children.push(copy);
        renumber(self, folder, &[(id, to)]);
        self.set_active(id);
        true
    }

    fn move_sheets(&mut self, folder: u64, frames: &[u32], delta: i32, copy: bool, onto: Onto) -> bool {
        if delta == 0 || frames.is_empty() { return false; }
        let mut from: Vec<u32> = frames.iter().copied().filter(|n| self.sheet_at(folder, *n).is_some()).collect();
        from.sort_unstable();
        from.dedup();
        if from.is_empty() { return false; }
        let to = |n: u32| n as i64 + delta as i64;
        if from.iter().any(|n| !(1..=MAX_FRAME as i64).contains(&to(*n))) { return false; }
        let clash = from.iter().any(|n| { let t = to(*n) as u32; self.sheet_at(folder, t).is_some() && (copy || !from.contains(&t)) });
        if clash && onto == Onto::Refuse { return false; }
        if clash && onto == Onto::Swap && !copy {
            // The sheets landed on take the frames left free, in order.
            let moved: Vec<(u64, u32)> = from.iter().filter_map(|&n| Some((self.sheet_at(folder, n)?, to(n) as u32))).collect();
            let dest: Vec<u32> = from.iter().map(|&n| to(n) as u32).filter(|t| !from.contains(t)).collect();
            let mut hit: Vec<u32> = dest.iter().copied().filter(|t| self.sheet_at(folder, *t).is_some()).collect();
            hit.sort_unstable();
            let mut free: Vec<u32> = from.iter().copied().filter(|n| !dest.contains(n)).collect();
            free.sort_unstable();
            let mut moves = moved;
            moves.extend(hit.iter().zip(&free).filter_map(|(&t, &f)| Some((self.sheet_at(folder, t)?, f))));
            renumber(self, folder, &moves);
            if let Some(&(id, _)) = moves.first() { self.set_active(id); }
            return true;
        }
        // Far end first: no sheet lands on one still to move.
        let order: Vec<u32> = if delta > 0 { from.iter().rev().copied().collect() } else { from.clone() };
        let mut any = false;
        for n in order {
            let t = to(n) as u32;
            any |= if copy { self.copy_sheet(folder, n, t, Onto::Replace) } else { self.move_sheet(folder, n, t, Onto::Replace) };
        }
        any
    }

    fn delete_sheet(&mut self, folder: u64, n: u32) -> bool {
        let Some(id) = self.sheet_at(folder, n) else { return false };
        if self.is_locked(id) || !can_take(self, id) { return false; }
        take_sheet(self, id);
        if self.anim_of(self.active_id()).is_none_or(|(f, _)| f == folder) { self.set_active(folder); }
        true
    }

    fn shift_frames(&mut self, folder: u64, at: u32, edit: Shift) -> bool {
        let frames = self.sheets(folder);
        let moves: Vec<(u64, u32)> = match edit {
            Shift::Insert => {
                if frames.last().is_some_and(|f| f.0 >= MAX_FRAME) { return false; }
                frames.iter().filter(|f| f.0 >= at).map(|f| (f.1, f.0 + 1)).collect()
            }
            Shift::Remove => {
                if let Some(id) = self.sheet_at(folder, at) {
                    if self.is_locked(id) || !can_take(self, id) { return false; }
                    take_sheet(self, id);
                }
                frames.iter().filter(|f| f.0 > at).map(|f| (f.1, f.0 - 1)).collect()
            }
        };
        let changed = !moves.is_empty() || edit == Shift::Remove && frames.iter().any(|f| f.0 == at);
        renumber(self, folder, &moves);
        if self.anim_of(self.active_id()).is_none() { self.set_active(folder); }
        changed
    }
}

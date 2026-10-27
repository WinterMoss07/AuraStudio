//! What the interface may do to the layers, as values: each edit is one
//! named operation of the layer stack, so a panel can't leave the tree in
//! a state the stack would not make itself. The interface holds an
//! `Editor`: it reads the stack freely and changes it only with `apply`.

use std::ops::Deref;

use aurastudio_core::BlendMode;
use aurastudio_filters::Filter;
use aurastudio_layers::onion::Onion;
use aurastudio_layers::{Cel, FolderBlend, LayerStack, Node, Onto, Place, Shift};

/// One change of the layer stack.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    // Nodes.
    AddLayer(String),
    AddFolder(String),
    /// In group `group` (None: outside any group).
    AddAnimationFolder { name: String, group: Option<u64> },
    AddFilterLayer(Filter),
    SetActive(u64),
    /// The active node (refused when `removal_refused` says why).
    RemoveActive,
    /// The active node, above itself.
    DuplicateActive,
    /// `ids` in a new folder (one id: the active node).
    Group { ids: Vec<u64>, name: String },
    /// The active folder's content out of it.
    Ungroup,
    GroupAnimationFolders { ids: Vec<u64>, name: String },
    /// `ids` (with `dragged` among them) next to or into `target`.
    Move { ids: Vec<u64>, dragged: u64, target: u64, place: Place },
    // A node's settings.
    Rename(u64, String),
    SetVisible(u64, bool),
    SetLocked(u64, bool),
    SetCollapsed(u64, bool),
    SetOpacity(u64, f32),
    SetBlend(u64, BlendMode),
    SetFolderBlend(u64, FolderBlend),
    SetLabel(u64, u8),
    SetClip(u64, bool),
    SetDraft(u64, bool),
    /// One per folder: the others of its folder lose it.
    SetReference(u64, bool),
    SetFilter(u64, Filter),
    // Time and sheets.
    SetTime(u32),
    SetOnion(Onion),
    ShiftFrames { folder: u64, at: u32, shift: Shift },
    DeleteSheet { folder: u64, frame: u32 },
    MoveSheets { folder: u64, frames: Vec<u32>, delta: i32, copy: bool, onto: Onto },
    // The cel template of an animation folder.
    AddCel { folder: u64, at: usize, cel: Cel },
    RemoveCel { folder: u64, at: usize },
    RenameCel { folder: u64, at: usize, name: String },
    MoveCel { folder: u64, from: usize, to: usize },
    SetCel { folder: u64, rank: usize },
    SetCelReference { folder: u64, at: usize, on: bool },
    /// Eye and lock of a template cel (what sheets start with).
    SetTemplateState { folder: u64, at: usize, visible: Option<bool>, locked: Option<bool> },
    /// The eyes and locks of sheet `sheet` to every sheet of its folder.
    ApplyToFolder(u64),
}

/// What an edit did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Done {
    /// Something changed.
    pub changed: bool,
    /// The node it made, if any.
    pub made:    Option<u64>,
}

impl Done {
    fn changed(c: bool) -> Self { Self { changed: c, made: None } }
    fn made(id: Option<u64>) -> Self { Self { changed: id.is_some(), made: id } }
}

impl Edit {
    /// Do it on `s`.
    pub fn apply(self, s: &mut LayerStack) -> Done {
        fn node(s: &mut LayerStack, id: u64, f: impl FnOnce(&mut Node) -> bool) -> Done {
            Done::changed(s.node_mut(id).is_some_and(f))
        }
        fn set<T: PartialEq>(slot: &mut T, v: T) -> bool {
            if *slot == v { return false; }
            *slot = v;
            true
        }
        match self {
            Edit::AddLayer(name) => Done::made(Some(s.add_layer(&name))),
            Edit::AddFolder(name) => Done::made(Some(s.add_folder(&name))),
            Edit::AddAnimationFolder { name, group } => Done::made(Some(s.add_animation_folder_in(&name, group))),
            Edit::AddFilterLayer(f) => Done::made(Some(s.add_filter_layer(f))),
            Edit::SetActive(id) => {
                let before = s.active_id();
                s.set_active(id);
                Done::changed(s.active_id() != before)
            }
            Edit::RemoveActive => Done::changed(s.removal_refused().is_none() && s.remove_active()),
            Edit::DuplicateActive => Done::made(Some(s.duplicate_active())),
            Edit::Group { ids, name } => {
                if ids.len() > 1 { Done::made(s.group_nodes(&ids, &name)) } else { Done::made(Some(s.group_active(&name))) }
            }
            Edit::Ungroup => Done::changed(s.ungroup_active()),
            Edit::GroupAnimationFolders { ids, name } => Done::made(s.group_animation_folders(&ids, &name)),
            Edit::Move { ids, dragged, target, place } => Done::changed(s.move_nodes(&ids, dragged, target, place)),
            Edit::Rename(id, name) => node(s, id, |n| set(n.name_mut(), name)),
            Edit::SetVisible(id, v) => node(s, id, |n| set(n.visible_mut(), v)),
            Edit::SetLocked(id, v) => node(s, id, |n| set(n.locked_mut(), v)),
            Edit::SetCollapsed(id, v) => node(s, id, |n| match n { Node::Folder(f) => set(&mut f.collapsed, v), _ => false }),
            Edit::SetOpacity(id, v) => node(s, id, |n| set(n.opacity_mut(), v.clamp(0.0, 1.0))),
            Edit::SetBlend(id, b) => node(s, id, |n| match n { Node::Layer(l) => set(&mut l.blend, b), _ => false }),
            Edit::SetFolderBlend(id, b) => node(s, id, |n| match n { Node::Folder(f) => set(&mut f.blend, b), _ => false }),
            Edit::SetLabel(id, l) => node(s, id, |n| set(&mut n.props_mut().label, l)),
            Edit::SetClip(id, v) => node(s, id, |n| set(&mut n.props_mut().clip, v)),
            Edit::SetDraft(id, v) => node(s, id, |n| set(&mut n.props_mut().draft, v)),
            Edit::SetReference(id, on) => {
                let was = s.node(id).map(|n| n.props().reference);
                s.set_reference(id, on);
                Done::changed(was.is_some_and(|w| w != on))
            }
            Edit::SetFilter(id, f) => node(s, id, |n| match n { Node::Filter(x) => set(&mut x.filter, f), _ => false }),
            // (Changed: the merge may show something else.)
            Edit::SetTime(t) => Done::changed(s.set_time(t)),
            Edit::SetOnion(o) => {
                if *s.onion() == o { return Done::default(); }
                s.set_onion(o);
                Done::changed(true)
            }
            Edit::ShiftFrames { folder, at, shift } => Done::changed(s.shift_frames(folder, at, shift)),
            Edit::DeleteSheet { folder, frame } => Done::changed(s.delete_sheet(folder, frame)),
            Edit::MoveSheets { folder, frames, delta, copy, onto } => Done::changed(s.move_sheets(folder, &frames, delta, copy, onto)),
            Edit::AddCel { folder, at, cel } => { s.add_cel(folder, at, cel); Done::changed(true) }
            Edit::RemoveCel { folder, at } => Done::changed(s.remove_cel(folder, at)),
            Edit::RenameCel { folder, at, name } => { s.rename_cel(folder, at, &name); Done::changed(true) }
            Edit::MoveCel { folder, from, to } => { s.move_cel(folder, from, to); Done::changed(from != to) }
            Edit::SetCel { folder, rank } => { s.set_cel(folder, rank); Done::changed(true) }
            Edit::SetCelReference { folder, at, on } => { s.set_cel_reference(folder, at, on); Done::changed(true) }
            Edit::SetTemplateState { folder, at, visible, locked } => Done::changed(s.set_template_state(folder, at, visible, locked)),
            Edit::ApplyToFolder(sheet) => { s.apply_to_folder(sheet); Done::changed(true) }
        }
    }
}

/// The layer stack as the interface holds it: read as a `LayerStack`,
/// changed only by `apply`. The edits made are counted (for tests, and a
/// log to come).
pub struct Editor<'a> {
    stack: &'a mut LayerStack,
    applied: usize,
}

impl<'a> Editor<'a> {
    pub fn new(stack: &'a mut LayerStack) -> Self { Self { stack, applied: 0 } }

    pub fn apply(&mut self, e: Edit) -> Done {
        self.applied += 1;
        e.apply(self.stack)
    }

    /// How many edits were applied.
    pub fn applied(&self) -> usize { self.applied }
}

impl Deref for Editor<'_> {
    type Target = LayerStack;
    fn deref(&self) -> &LayerStack { self.stack }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> LayerStack {
        let mut s = LayerStack::new(64, 64);
        s.add_layer("a");
        s
    }

    #[test]
    fn node_settings_report_a_change_only_when_there_is_one() {
        let mut s = stack();
        let id = s.active_id();
        assert!(Edit::SetOpacity(id, 0.5).apply(&mut s).changed);
        assert!(!Edit::SetOpacity(id, 0.5).apply(&mut s).changed);
        assert!(Edit::Rename(id, "ink".into()).apply(&mut s).changed);
        assert_eq!(s.node(id).unwrap().name(), "ink");
        assert!(Edit::SetBlend(id, BlendMode::Multiply).apply(&mut s).changed);
        assert!(!Edit::SetFolderBlend(id, FolderBlend::PassThrough).apply(&mut s).changed, "a layer has no folder blend");
        assert!(!Edit::SetOpacity(9999, 0.1).apply(&mut s).changed, "no such node");
    }

    #[test]
    fn made_nodes_are_given_back() {
        let mut s = stack();
        let d = Edit::AddFolder("f".into()).apply(&mut s);
        assert!(d.changed && d.made.is_some_and(|id| s.node(id).is_some()));
        let id = d.made.unwrap();
        let folded = matches!(s.node(id), Some(Node::Folder(f)) if f.collapsed);
        assert!(Edit::SetCollapsed(id, !folded).apply(&mut s).changed);
        assert!(!Edit::SetCollapsed(id, !folded).apply(&mut s).changed);
    }

    #[test]
    fn the_last_paint_layer_is_not_removed() {
        let mut s = LayerStack::new(16, 16);
        let only = s.add_layer("only");
        // (The paper is locked: the one layer left can't go either.)
        Edit::SetActive(only).apply(&mut s);
        let before = s.listed_layer_ids().len();
        let d = Edit::RemoveActive.apply(&mut s);
        assert_eq!(d.changed, s.listed_layer_ids().len() < before);
    }

    #[test]
    fn the_editor_reads_and_counts() {
        let mut s = stack();
        let mut e = Editor::new(&mut s);
        let id = e.active_id();
        e.apply(Edit::SetVisible(id, false));
        assert!(!e.node(id).unwrap().visible());
        assert_eq!(e.applied(), 1);
    }
}

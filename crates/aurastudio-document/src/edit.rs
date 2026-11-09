//! Every change of a document, as a value: `Document::apply` is the only
//! door (strokes too go through it, by `Document::stroke`). Each edit is
//! one named operation, so a panel can't leave the tree in a state the
//! stack would not make itself; after it the time settles, the tree step
//! is recorded, the revision moves.

use aurastudio_animation::{Onion, OnionLook, SheetEdit};
use aurastudio_color::Profile;
use aurastudio_core::{BlendMode, Fps, Rgba8};
use aurastudio_filters::Filter;
use aurastudio_layers::{Cel, FillParams, FolderBlend, LayerStack, Node, Onto, Place, Shift};
use aurastudio_transform::{Interp, Warp};

use crate::clip::Clip;
use crate::select::SelectOp;

/// One change of a document.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    // Nodes.
    AddLayer(String),
    AddFolder(String),
    /// In group `group` (`None`: outside any group).
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
    // Selection masks.
    /// The selection kept as a mask node named so.
    SelectionToMask(String),
    /// Mask node `id` becomes the selection.
    MaskToSelection(u64),
    // Time and sheets.
    SetFrame(u32),
    SetFps(Fps),
    SetOnion(Onion),
    SetOnionLook(OnionLook),
    /// Ghosts off for a while (playback).
    PauseOnion(bool),
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
    // Pixels.
    /// The active layer (the selected pixels of it) back to nothing.
    ClearActive,
    /// The fill tool at a canvas pixel.
    Fill { x: i32, y: i32, color: Rgba8, params: FillParams },
    /// On a new layer above the active one.
    Paste(Clip),
    // Selection.
    Select(SelectOp),
    // The filter dialog: shown on the active layer until applied or
    // cancelled (`None`: the layer as it was).
    FilterPreview(Option<Filter>),
    FilterApply(Filter),
    FilterCancel,
    // The transform tool: these paint layers (the selection says what
    // moves), shown warped until applied (one step, the selection moved
    // along) or cancelled.
    TransformBegin(Vec<u64>),
    TransformPreview { warp: Warp, interp: Interp },
    TransformApply { warp: Warp, interp: Interp },
    TransformCancel,
    // The document.
    Undo,
    Redo,
    /// New frame, the content centered (nothing is cut); the history is
    /// dropped.
    Resize { width: u32, height: u32 },
    /// Room kept around the frame, percent per side (what lies further is
    /// cut; the history is dropped, as for a resize).
    SetMargin(u32),
    SetDpi(f32),
    /// Give the document another profile, pixels unchanged (undoable).
    AssignProfile(Profile),
}

impl Edit {
    /// Its name in the history.
    pub fn label(&self) -> &'static str {
        match self {
            Edit::AddLayer(_) => "New layer",
            Edit::AddFolder(_) => "New folder",
            Edit::AddAnimationFolder { .. } => "New animation folder",
            Edit::AddFilterLayer(_) => "New filter layer",
            Edit::SetActive(_) => "Select layer",
            Edit::RemoveActive => "Delete layer",
            Edit::DuplicateActive => "Duplicate layer",
            Edit::Group { .. } | Edit::GroupAnimationFolders { .. } => "Group",
            Edit::Ungroup => "Ungroup",
            Edit::Move { .. } => "Move layers",
            Edit::Rename(..) => "Rename",
            Edit::SetVisible(..) => "Visibility",
            Edit::SetLocked(..) => "Lock",
            Edit::SetCollapsed(..) => "Fold",
            Edit::SetOpacity(..) => "Opacity",
            Edit::SetBlend(..) | Edit::SetFolderBlend(..) => "Blend mode",
            Edit::SetLabel(..) => "Color label",
            Edit::SetClip(..) => "Clipping",
            Edit::SetDraft(..) => "Draft layer",
            Edit::SetReference(..) => "Reference layer",
            Edit::SetFilter(..) => "Filter settings",
            Edit::SelectionToMask(_) => "Save selection",
            Edit::MaskToSelection(_) => "Load selection",
            Edit::SetFrame(_) => "Frame",
            Edit::SetFps(_) => "Frame rate",
            Edit::SetOnion(_) | Edit::SetOnionLook(_) | Edit::PauseOnion(_) => "Onion skin",
            Edit::ShiftFrames { .. } => "Timing",
            Edit::DeleteSheet { .. } => "Delete sheet",
            Edit::MoveSheets { copy: true, .. } => "Copy sheets",
            Edit::MoveSheets { .. } => "Move sheets",
            Edit::AddCel { .. } => "New cel",
            Edit::RemoveCel { .. } => "Delete cel",
            Edit::RenameCel { .. } => "Rename cel",
            Edit::MoveCel { .. } => "Move cel",
            Edit::SetCel { .. } => "Choose cel",
            Edit::SetCelReference { .. } => "Reference cel",
            Edit::SetTemplateState { .. } => "Cel template",
            Edit::ApplyToFolder(_) => "Apply to folder",
            Edit::ClearActive => "Clear layer",
            Edit::Fill { .. } => "Fill",
            Edit::Paste(_) => "Paste",
            Edit::Select(op) => op.label(),
            Edit::FilterPreview(_) | Edit::FilterCancel => "Filter",
            Edit::FilterApply(f) => f.kind.name(),
            Edit::TransformBegin(_) | Edit::TransformPreview { .. } | Edit::TransformCancel | Edit::TransformApply { .. } => "Transform",
            Edit::Undo => "Undo",
            Edit::Redo => "Redo",
            Edit::Resize { .. } => "Canvas size",
            Edit::SetMargin(_) => "Canvas margin",
            Edit::SetDpi(_) => "Resolution",
            Edit::AssignProfile(_) => "Assign profile",
        }
    }

    /// It changes pixels or the history (a stroke under way ends first).
    pub(crate) fn changes_pixels(&self) -> bool {
        matches!(self, Edit::ClearActive | Edit::Fill { .. } | Edit::Paste(_) | Edit::FilterPreview(_) | Edit::FilterApply(_) | Edit::FilterCancel
            | Edit::TransformBegin(_) | Edit::TransformPreview { .. } | Edit::TransformApply { .. } | Edit::TransformCancel
            | Edit::Undo | Edit::Redo | Edit::Resize { .. } | Edit::SetMargin(_) | Edit::RemoveActive | Edit::DuplicateActive | Edit::SetFilter(..)
            | Edit::DeleteSheet { .. } | Edit::MoveSheets { .. } | Edit::ShiftFrames { .. } | Edit::RemoveCel { .. } | Edit::MoveCel { .. } | Edit::AddCel { .. }
            | Edit::SelectionToMask(_) | Edit::Select(_) | Edit::MaskToSelection(_))
    }

    /// A setting of one node a later change of joins as one undo step (an
    /// opacity dragged, a name typed): that node.
    pub(crate) fn joins(&self) -> Option<u64> {
        match self {
            Edit::Rename(id, _) | Edit::SetOpacity(id, _) | Edit::SetBlend(id, _) | Edit::SetFolderBlend(id, _) | Edit::SetLabel(id, _) | Edit::SetFilter(id, _) => Some(*id),
            Edit::RenameCel { folder, .. } => Some(*folder),
            _ => None,
        }
    }

    /// The nodes whose footprint it may change (to bound the region).
    pub(crate) fn nodes(&self, s: &LayerStack) -> Vec<u64> {
        match self {
            Edit::RemoveActive | Edit::DuplicateActive | Edit::Ungroup => vec![s.active_id()],
            Edit::Group { ids, .. } | Edit::GroupAnimationFolders { ids, .. } | Edit::Move { ids, .. } => {
                if ids.is_empty() { vec![s.active_id()] } else { ids.clone() }
            }
            Edit::Rename(id, _) | Edit::SetVisible(id, _) | Edit::SetLocked(id, _) | Edit::SetCollapsed(id, _) | Edit::SetOpacity(id, _)
            | Edit::SetBlend(id, _) | Edit::SetFolderBlend(id, _) | Edit::SetLabel(id, _) | Edit::SetClip(id, _) | Edit::SetDraft(id, _)
            | Edit::SetReference(id, _) | Edit::SetFilter(id, _) | Edit::ApplyToFolder(id) => vec![*id],
            Edit::ShiftFrames { folder, .. } | Edit::DeleteSheet { folder, .. } | Edit::MoveSheets { folder, .. } | Edit::AddCel { folder, .. }
            | Edit::RemoveCel { folder, .. } | Edit::RenameCel { folder, .. } | Edit::MoveCel { folder, .. } | Edit::SetCel { folder, .. }
            | Edit::SetCelReference { folder, .. } | Edit::SetTemplateState { folder, .. } => vec![*folder],
            _ => Vec::new(),
        }
    }
}

/// What a tree edit did: changed, and the node it made.
pub(crate) fn apply_tree(e: Edit, s: &mut LayerStack) -> (bool, Option<u64>) {
    fn node(s: &mut LayerStack, id: u64, f: impl FnOnce(&mut Node) -> bool) -> (bool, Option<u64>) { (s.node_mut(id).is_some_and(f), None) }
    fn set<T: PartialEq>(slot: &mut T, v: T) -> bool {
        if *slot == v { return false; }
        *slot = v;
        true
    }
    let made = |id: Option<u64>| (id.is_some(), id);
    match e {
        Edit::AddLayer(name) => made(Some(s.add_layer(&name))),
        Edit::AddFolder(name) => made(Some(s.add_folder(&name))),
        Edit::AddAnimationFolder { name, group } => made(Some(s.add_animation_folder_in(&name, group))),
        Edit::AddFilterLayer(f) => made(Some(s.add_filter_layer(f))),
        Edit::SetActive(id) => {
            let before = s.active_id();
            s.set_active(id);
            (s.active_id() != before, None)
        }
        Edit::RemoveActive => (s.removal_refused().is_none() && s.remove_active(), None),
        Edit::DuplicateActive => made(Some(s.duplicate_active())),
        Edit::Group { ids, name } => made(if ids.len() > 1 { s.group_nodes(&ids, &name) } else { Some(s.group_active(&name)) }),
        Edit::Ungroup => (s.ungroup_active(), None),
        Edit::GroupAnimationFolders { ids, name } => made(s.group_animation_folders(&ids, &name)),
        Edit::Move { ids, dragged, target, place } => (s.move_nodes(&ids, dragged, target, place), None),
        Edit::Rename(id, name) => node(s, id, |n| set(n.name_mut(), name)),
        Edit::SetVisible(id, v) => node(s, id, |n| set(n.visible_mut(), v)),
        Edit::SetLocked(id, v) => node(s, id, |n| set(n.locked_mut(), v)),
        Edit::SetCollapsed(id, v) => node(s, id, |n| match n { Node::Folder(f) => set(&mut f.collapsed, v), _ => false }),
        Edit::SetOpacity(id, v) => node(s, id, |n| {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { return false };
            if n.opacity() == v || matches!(n, Node::Mask(_)) { return false; }
            n.set_opacity(v);
            true
        }),
        Edit::SetBlend(id, b) => node(s, id, |n| match n { Node::Layer(l) => set(&mut l.blend, b), _ => false }),
        Edit::SetFolderBlend(id, b) => node(s, id, |n| match n { Node::Folder(f) => set(&mut f.blend, b), _ => false }),
        Edit::SetLabel(id, l) => node(s, id, |n| set(&mut n.props_mut().label, l.min(aurastudio_layers::LABELS.len() as u8 - 1))),
        Edit::SetClip(id, v) => node(s, id, |n| set(&mut n.props_mut().clip, v)),
        Edit::SetDraft(id, v) => node(s, id, |n| set(&mut n.props_mut().draft, v)),
        Edit::SetReference(id, on) => {
            let was = s.node(id).map(|n| n.props().reference);
            s.set_reference(id, on);
            (was.is_some_and(|w| w != on), None)
        }
        Edit::SetFilter(id, f) => node(s, id, |n| match n { Node::Filter(x) => set(&mut x.filter, f), _ => false }),
        Edit::ShiftFrames { folder, at, shift } => (s.shift_frames(folder, at, shift), None),
        Edit::DeleteSheet { folder, frame } => (s.delete_sheet(folder, frame), None),
        Edit::MoveSheets { folder, frames, delta, copy, onto } => (s.move_sheets(folder, &frames, delta, copy, onto), None),
        Edit::AddCel { folder, at, cel } => { s.add_cel(folder, at, cel); (true, None) }
        Edit::RemoveCel { folder, at } => (s.remove_cel(folder, at), None),
        Edit::RenameCel { folder, at, name } => { s.rename_cel(folder, at, &name); (true, None) }
        Edit::MoveCel { folder, from, to } => { s.move_cel(folder, from, to); (from != to, None) }
        Edit::SetCel { folder, rank } => { s.set_cel(folder, rank); (true, None) }
        Edit::SetCelReference { folder, at, on } => { s.set_cel_reference(folder, at, on); (true, None) }
        Edit::SetTemplateState { folder, at, visible, locked } => (s.set_template_state(folder, at, visible, locked), None),
        Edit::ApplyToFolder(sheet) => { s.apply_to_folder(sheet); (true, None) }
        _ => (false, None),
    }
}

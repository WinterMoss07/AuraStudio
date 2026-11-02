//! Animation folders. One folder per element (a character, smoke...),
//! one child per sheet (a drawing: "Sheet 1", "Sheet 2"... in time
//! order), each sheet exposed from a frame (its place in time) and
//! holding the cels of the folder's cel template (genga, douga, ink,
//! colo...) as paint layers. Only one sheet of a folder shows at a time:
//! the last one exposed at or before the current frame (a hold), carried
//! by the sheets' visibility so that every merge stays as it is.
//!
//! Frames are the time (the timeline's columns, the X-sheet's rows);
//! sheets are the drawings exposed on them.
//!
//! The template is the same for every sheet of a folder: locks, cel
//! choice and the onion skin go by a cel's rank, so a douga is never
//! shown or painted among gengas.
//!
//! The hierarchy has one level of each, never nested: Group (animation
//! folders put together, for the timeline) → Folder → Sheet → Cel.
//!
//! Here: the data (it is part of the tree) and the structure queries the
//! tree's own rules need. What time does with it (the frame shown, sheet
//! and cel operations, the onion skin) is `animation`'s.

use crate::{Folder, LayerStack, Node};

/// What a folder is.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FolderKind {
    #[default]
    Plain,
    /// An animation folder: its sheets follow `Anim`'s template.
    Animation(Anim),
    /// One sheet of an animation folder, exposed from that frame (from
    /// 1): its children are the cels, bottom to top.
    Sheet(u32),
    /// Animation folders put together (one entry in the timeline). Holds
    /// only animation folders, and is never inside another group.
    Group,
}

/// An animation folder's cel template and the cel painted in.
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    /// Top to bottom, as listed.
    pub template: Vec<Cel>,
    /// Rank (in `template`) of the cel strokes go to, in every sheet.
    pub cel:      usize,
}

/// One cel of a template: its name, and what a new sheet starts with.
#[derive(Clone, Debug, PartialEq)]
pub struct Cel {
    pub name:    String,
    /// The coloring cel (palette colors, later).
    pub colo:    bool,
    /// The reference of its sheet (what the Colo's fill reads): Ink.
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
    /// A sheet to make first: this animation folder has none exposed
    /// from `frame`.
    NewSheet { folder: u64, frame: u32 },
}

/// What a sheet moved or copied onto another one does.
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

/// Most frames a folder may reach (a 24 fps hour).
pub const MAX_FRAME: u32 = 86_400;

impl Folder {
    pub fn anim(&self) -> Option<&Anim> {
        match &self.kind { FolderKind::Animation(a) => Some(a), _ => None }
    }

    pub fn anim_mut(&mut self) -> Option<&mut Anim> {
        match &mut self.kind { FolderKind::Animation(a) => Some(a), _ => None }
    }

    pub fn sheet_start(&self) -> Option<u32> {
        match self.kind { FolderKind::Sheet(n) => Some(n), _ => None }
    }

    pub fn is_group(&self) -> bool { self.kind == FolderKind::Group }
}

impl Node {
    /// It is an animation folder.
    pub fn is_animation(&self) -> bool { matches!(self, Node::Folder(f) if f.anim().is_some()) }

    /// The frame it is exposed from, if it is a sheet.
    pub fn sheet_start(&self) -> Option<u32> {
        match self { Node::Folder(f) => f.sheet_start(), _ => None }
    }

    pub fn is_group(&self) -> bool { matches!(self, Node::Folder(f) if f.is_group()) }
}

impl LayerStack {
    /// The animation folder `id` is in or is, and the sheet it is or is
    /// in (None: it is the folder itself).
    pub fn anim_of(&self, id: u64) -> Option<(u64, Option<u64>)> {
        let path = self.path(id)?;
        let mut nodes = &self.root;
        let mut found = None;
        for &i in &path {
            let n = &nodes[i];
            if n.is_animation() { found = Some((n.id(), None)); }
            if let (Some((f, None)), Some(_)) = (found, n.sheet_start()) { found = Some((f, Some(n.id()))); }
            match n { Node::Folder(f) => nodes = &f.children, _ => break }
        }
        found
    }

    /// `id` is a sheet or a cel: it stays where it is (the X-sheet moves
    /// sheets; the template decides the cels).
    pub fn is_anim_part(&self, id: u64) -> bool { matches!(self.anim_of(id), Some((_, Some(_)))) }

    /// `id` sits directly in a group.
    pub fn in_group(&self, id: u64) -> bool {
        let Some(path) = self.path(id) else { return false };
        let mut nodes = &self.root;
        let mut parent_is_group = false;
        for &i in &path[..path.len().saturating_sub(1)] {
            match &nodes[i] {
                Node::Folder(f) => { parent_is_group = f.is_group(); nodes = &f.children; }
                _ => return false,
            }
        }
        parent_is_group
    }
}

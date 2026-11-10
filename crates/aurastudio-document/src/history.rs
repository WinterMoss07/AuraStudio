//! Undo / redo of one document within a byte budget. A pixel step is the
//! patch of the tiles it changed (swapped with the layer: no copy); a tree
//! step keeps the tree as it was and the nodes only one side has; a
//! selection step the selection to go back to. Steps older than the last
//! one are packed (LZ4).

use std::collections::VecDeque;

use aurastudio_color::Profile;
use aurastudio_core::{HeapSize, Rgba8};
use aurastudio_layers::{LayerStack, Node, TreeState};
use aurastudio_raster::Patch;
use aurastudio_selection::Selection;

use crate::region::{Done, Region};

/// Default memory budget for one document's history (bytes).
pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;
/// Upper bound on undo steps, whatever their size.
const MAX_STEPS: usize = 200;

pub(crate) enum Step {
    /// `fill`: the layer's fill to go back to, when it changed too (a
    /// filter on the Paper).
    Pixels { layer: u64, patch: Patch<Rgba8>, fill: Option<Rgba8> },
    /// `region`: where the merged image changed when it was done (the
    /// footprints of the nodes it touched, before and after): the same
    /// both ways, since the steps after it are undone first.
    Tree { state: TreeState, kept: Vec<Node>, region: Region },
    /// The selection to go back to.
    Select(Option<Selection>),
    /// The document's profile to go back to.
    Profile(Profile),
    /// A sheet made for a paint, at `place` (its folder, its index):
    /// undone, it is taken out (`node`); redone, put back. `active`: the
    /// node to make active then.
    Sheet { id: u64, place: (u64, usize), node: Option<Node>, active: u64 },
    /// Done as one (a transform: its layers and the selection; a sheet
    /// made and its first stroke); `undone`: which way the next swap goes.
    Group { steps: Vec<Step>, undone: bool },
    /// A new canvas size or margin: the whole stack and the selection to go
    /// back to (their tiles shared with the stack's where unchanged).
    Frame { stack: Box<LayerStack>, selection: Option<Selection> },
}

/// What the history needs of the document to swap a step in.
pub(crate) struct Target<'a> {
    pub stack:     &'a mut LayerStack,
    pub selection: &'a mut Option<Selection>,
    pub profile:   &'a mut Profile,
}

impl Step {
    fn bytes(&self) -> usize {
        match self {
            Step::Pixels { patch, .. } => patch.heap_bytes(),
            Step::Tree { kept, .. } => kept.iter().map(HeapSize::heap_bytes).sum(),
            Step::Select(s) => s.as_ref().map_or(0, HeapSize::heap_bytes),
            Step::Profile(p) => p.icc().len(),
            Step::Sheet { node, .. } => node.as_ref().map_or(0, HeapSize::heap_bytes),
            Step::Group { steps, .. } => steps.iter().map(Step::bytes).sum(),
            // (Counted whole, though most tiles are the stack's too.)
            Step::Frame { stack, selection } => stack.root.iter().map(HeapSize::heap_bytes).sum::<usize>() + selection.as_ref().map_or(0, HeapSize::heap_bytes),
        }
    }

    fn pack(&mut self) {
        match self {
            Step::Pixels { patch, .. } => patch.pack(),
            Step::Group { steps, .. } => steps.iter_mut().for_each(Step::pack),
            _ => {}
        }
    }

    /// Undo / redo it: the step now goes the other way. `None` (and
    /// dropped) if it can't be (its layer is gone).
    fn swap_into(&mut self, t: &mut Target<'_>) -> Option<Done> {
        match self {
            Step::Pixels { layer, patch, fill } => {
                if !t.stack.swap_patch(*layer, patch) { return None; }
                let mut d = Done::rects(patch.rects());
                d.layers.push(*layer);
                if let (Some(f), Some(img)) = (fill.as_mut(), t.stack.image_mut(*layer)) {
                    let now = img.fill();
                    img.set_fill(*f);
                    *f = now;
                    d.region = crate::Region::Whole;
                }
                Some(d)
            }
            Step::Tree { state, kept, region } => {
                let now = t.stack.tree_state();
                let made = t.stack.restore_tree(state, std::mem::take(kept));
                *state = now;
                *kept = made;
                Some(Done { region: region.clone(), tree: true, ..Done::default() })
            }
            Step::Select(s) => {
                std::mem::swap(s, t.selection);
                Some(Done { selection: true, ..Done::default() })
            }
            Step::Profile(p) => {
                std::mem::swap(p, t.profile);
                Some(Done { region: crate::Region::Whole, other: true, ..Done::default() })
            }
            Step::Sheet { id, place, node, active } => {
                // (Before: taking the sheet out moves what is active.)
                let now = t.stack.active_id();
                // (What it shows: its own pixels; what the time shows
                // instead is added when the time settles.)
                let region;
                match node.take() {
                    None => {
                        *place = t.stack.place_of(*id)?;
                        region = Region::of_footprint(t.stack.footprint(*id));
                        *node = Some(t.stack.take_node(*id)?);
                    }
                    Some(n) => {
                        let folder = place.0 == 0 || matches!(t.stack.node(place.0), Some(Node::Folder(_)));
                        if !folder || t.stack.node(*id).is_some() { *node = Some(n); return None; }
                        t.stack.add_node(n, place.0, place.1);
                        region = Region::of_footprint(t.stack.footprint(*id));
                        // (Saved in another order: stacked as they come.)
                        aurastudio_animation::SheetEdit::stack_sheets(&mut *t.stack);
                    }
                }
                t.stack.set_active(*active);
                *active = now;
                Some(Done { region, tree: true, ..Done::default() })
            }
            Step::Frame { stack, selection } => {
                std::mem::swap(&mut **stack, t.stack);
                std::mem::swap(selection, t.selection);
                Some(Done { region: crate::Region::Whole, tree: true, selection: true, other: true, ..Done::default() })
            }
            Step::Group { steps, undone } => {
                // Back in the reverse order, forward in the order done.
                let n = steps.len();
                let mut done: Option<Done> = None;
                for k in 0..n {
                    let i = if *undone { k } else { n - 1 - k };
                    if let Some(d) = steps[i].swap_into(t) {
                        match &mut done { Some(x) => x.join(d), None => done = Some(d) }
                    }
                }
                *undone = !*undone;
                done
            }
        }
    }
}

/// One saved pixel step (what `file` writes), in the order it applies.
pub(crate) struct Exported<'a> {
    pub label:  &'a str,
    pub layer:  u64,
    pub patch:  &'a Patch<Rgba8>,
    /// Undone with the one before it in the list (one step).
    pub joined: bool,
    /// The sheet it made first: id, folder, index.
    pub sheet:  Option<[u64; 3]>,
}

/// A pixel step read from a file (see `Exported`).
pub(crate) struct Imported {
    pub label:  String,
    pub layer:  u64,
    pub patch:  Patch<Rgba8>,
    pub joined: bool,
    pub sheet:  Option<[u64; 3]>,
}

/// One done step as saved: its pixel parts in the order they apply, the
/// sheet it made (with the first); the tree, selection and profile parts
/// are not in files. `None`: it can't come back (a sheet undone: its
/// content isn't in the file).
fn saved<'a>(label: &'a str, s: &'a Step, redo: bool) -> Option<Vec<Exported<'a>>> {
    fn flat<'a>(s: &'a Step, out: &mut Vec<&'a Step>) {
        match s {
            Step::Group { steps, .. } => for p in steps { flat(p, out) },
            p => out.push(p),
        }
    }
    let mut parts = Vec::new();
    flat(s, &mut parts);
    let (mut out, mut sheet, mut whole) = (Vec::new(), None, true);
    for p in parts {
        match p {
            Step::Pixels { layer, patch, .. } => out.push(Exported { label, layer: *layer, patch, joined: !out.is_empty(), sheet: None }),
            Step::Sheet { id, place, node: None, .. } => sheet = Some([*id, place.0, place.1 as u64]),
            // (Undone, its content isn't in the file.)
            Step::Sheet { .. } => whole = false,
            // (The steps before it are of another frame: not in the file.)
            Step::Frame { .. } => return None,
            _ => {}
        }
    }
    if let (Some(first), Some(sh)) = (out.first_mut(), sheet) { first.sheet = Some(sh); }
    if redo {
        // Redone in the order done; saved with the next to redo last.
        out.reverse();
        for (i, e) in out.iter_mut().enumerate() { e.joined = i > 0; }
    }
    whole.then_some(out)
}

pub struct History {
    undo:   VecDeque<(String, Step)>,
    redo:   Vec<(String, Step)>,
    budget: usize,
    /// Bumped by every push, undo and redo: the same value later means
    /// nothing happened in between.
    serial: u64,
    /// The last step, if a setting of one node that the next change of the
    /// same setting joins (an opacity dragged, a name typed: one step).
    joinable: Option<(String, u64)>,
}

impl History {
    pub fn new(budget: usize) -> Self { Self { undo: VecDeque::new(), redo: Vec::new(), budget, serial: 0, joinable: None } }

    /// A tree step that joins the last one when it changed the same setting
    /// of the same node (`key`).
    /// `region`: where it changed the merged image.
    pub(crate) fn push_tree(&mut self, label: &str, key: Option<u64>, state: TreeState, kept: Vec<Node>, region: &Region) {
        let k = key.map(|k| (label.to_owned(), k));
        if k.is_some() && self.joinable == k && self.redo.is_empty() {
            if let Some((_, Step::Tree { kept: old, region: r, .. })) = self.undo.back_mut() {
                old.extend(kept);
                r.add(region.clone());
                self.serial += 1;
                return;
            }
        }
        self.push(label, Step::Tree { state, kept, region: region.clone() });
        self.joinable = k;
    }

    pub(crate) fn push(&mut self, label: &str, step: Step) {
        self.joinable = None;
        self.redo.clear();
        self.serial += 1;
        // The one before is packed (the last stays quick to undo).
        if let Some((_, last)) = self.undo.back_mut() { last.pack(); }
        self.undo.push_back((label.to_owned(), step));
        self.trim();
    }

    /// A pixel step, if the patch holds anything.
    pub(crate) fn push_pixels(&mut self, label: &str, layer: u64, patch: Patch<Rgba8>, fill: Option<Rgba8>) -> bool {
        if patch.is_empty() && fill.is_none() { return false; }
        self.push(label, Step::Pixels { layer, patch, fill });
        true
    }

    /// What the next undo / redo would undo / redo.
    pub fn undo_label(&self) -> Option<&str> { self.undo.back().map(|s| s.0.as_str()) }
    pub fn redo_label(&self) -> Option<&str> { self.redo.last().map(|s| s.0.as_str()) }

    pub(crate) fn undo(&mut self, t: &mut Target<'_>) -> Option<Done> {
        self.serial += 1;
        self.joinable = None;
        // A step that can't be applied (its layer is gone) is dropped and
        // said so: one undo never undoes two things.
        let (label, mut step) = self.undo.pop_back()?;
        match step.swap_into(t) {
            Some(d) => {
                // (The next to redo stays as it is, the others packed.)
                if let Some((_, last)) = self.redo.last_mut() { last.pack(); }
                self.redo.push((label, step));
                Some(d)
            }
            None => Some(Done::refused("That step could not be undone (its layer is gone)")),
        }
    }

    pub(crate) fn redo(&mut self, t: &mut Target<'_>) -> Option<Done> {
        self.serial += 1;
        self.joinable = None;
        let (label, mut step) = self.redo.pop()?;
        match step.swap_into(t) {
            Some(d) => {
                if let Some((_, last)) = self.undo.back_mut() { last.pack(); }
                self.undo.push_back((label, step));
                self.trim();
                Some(d)
            }
            None => Some(Done::refused("That step could not be redone (its layer is gone)")),
        }
    }

    /// The newest undo steps (`max_undo` pixel parts at most) and the redo
    /// steps, as files keep them: pixel parts (a group's joined), the
    /// sheet made with the first. The redo steps stop at one that can't
    /// come back (the later ones need it).
    pub(crate) fn export(&self, max_undo: usize) -> (Vec<Exported<'_>>, Vec<Exported<'_>>) {
        // Only those since the last new frame (older ones fit the old one).
        let since = self.undo.iter().rposition(|(_, s)| matches!(s, Step::Frame { .. })).map_or(0, |i| i + 1);
        let mut groups: Vec<Vec<Exported<'_>>> = self.undo.iter().skip(since).filter_map(|(l, s)| saved(l, s, false)).collect();
        let mut n: usize = groups.iter().map(Vec::len).sum();
        let mut skip = 0;
        while n > max_undo && skip < groups.len() { n -= groups[skip].len(); skip += 1; }
        let undo = groups.drain(skip..).flatten().collect();
        // (The next to redo is last.)
        let mut redo: Vec<Vec<Exported<'_>>> = Vec::new();
        for (l, s) in self.redo.iter().rev() {
            match saved(l, s, true) { Some(g) => redo.push(g), None => break }
        }
        let redo = redo.into_iter().rev().flatten().collect();
        (undo, redo)
    }

    /// A history read from a file: pixel steps, oldest first; redo steps
    /// with the next one to redo last. Joined parts are one step again.
    pub(crate) fn import(budget: usize, undo: Vec<Imported>, redo: Vec<Imported>) -> Self {
        fn steps(list: Vec<Imported>, redo: bool) -> Vec<(String, Step)> {
            let mut out: Vec<(String, Vec<Step>)> = Vec::new();
            for i in list {
                let mut parts = Vec::new();
                if let Some([id, folder, index]) = i.sheet { parts.push(Step::Sheet { id, place: (folder, index as usize), node: None, active: folder }); }
                parts.push(Step::Pixels { layer: i.layer, patch: i.patch, fill: None });
                match out.last_mut() {
                    Some(last) if i.joined => last.1.extend(parts),
                    _ => out.push((i.label, parts)),
                }
            }
            out.into_iter().map(|(label, mut parts)| {
                if parts.len() == 1 { return (label, parts.remove(0)); }
                // (Saved in reverse for redo.)
                if redo { parts.reverse(); }
                (label, Step::Group { steps: parts, undone: redo })
            }).collect()
        }
        let mut h = Self { undo: steps(undo, false).into(), redo: steps(redo, true), budget, serial: 0, joinable: None };
        h.trim();
        h
    }

    pub fn serial(&self) -> u64 { self.serial }

    /// The last two steps done become one, named `label` (a sheet made
    /// and its first stroke).
    pub(crate) fn join_last(&mut self, label: &str) {
        if self.undo.len() < 2 { return; }
        let (Some((_, b)), Some((_, a))) = (self.undo.pop_back(), self.undo.pop_back()) else { return };
        self.serial += 1;
        self.undo.push_back((label.to_owned(), Step::Group { steps: vec![a, b], undone: false }));
    }

    /// Forget the step just undone (it is not to be redone).
    pub(crate) fn forget_redo(&mut self) {
        self.serial += 1;
        self.redo.pop();
    }

    pub fn clear(&mut self) {
        self.serial += 1;
        self.joinable = None;
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize { self.undo.len() }
    pub fn redo_len(&self) -> usize { self.redo.len() }

    /// Bytes held by the history.
    pub fn bytes(&self) -> usize { self.undo.iter().chain(self.redo.iter()).map(|s| s.1.bytes()).sum() }

    /// Drop the oldest steps until within budget (the newest is always
    /// kept, even alone over budget).
    fn trim(&mut self) {
        let mut bytes = self.bytes();
        while self.undo.len() > 1 && (bytes > self.budget || self.undo.len() > MAX_STEPS) {
            if let Some((_, old)) = self.undo.pop_front() { bytes -= old.bytes(); }
        }
    }
}

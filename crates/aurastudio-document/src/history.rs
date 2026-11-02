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

use crate::region::Done;

/// Default memory budget for one document's history (bytes).
pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;
/// Upper bound on undo steps, whatever their size.
const MAX_STEPS: usize = 200;

pub(crate) enum Step {
    /// `fill`: the layer's fill to go back to, when it changed too (a
    /// filter on the Paper).
    Pixels { layer: u64, patch: Patch<Rgba8>, fill: Option<Rgba8> },
    Tree { state: TreeState, kept: Vec<Node> },
    /// The selection to go back to.
    Select(Option<Selection>),
    /// The document's profile to go back to.
    Profile(Profile),
    /// Done as one (a transform: its layers and the selection; a sheet
    /// made and its first stroke); `undone`: which way the next swap goes.
    Group { steps: Vec<Step>, undone: bool },
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
            Step::Group { steps, .. } => steps.iter().map(Step::bytes).sum(),
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
                if let (Some(f), Some(img)) = (fill.as_mut(), t.stack.image_mut(*layer)) {
                    let now = img.fill();
                    img.set_fill(*f);
                    *f = now;
                    d.region = crate::Region::Whole;
                }
                Some(d)
            }
            Step::Tree { state, kept } => {
                let now = t.stack.tree_state();
                let made = t.stack.restore_tree(state, std::mem::take(kept));
                *state = now;
                *kept = made;
                Some(Done { region: crate::Region::Whole, tree: true, ..Done::default() })
            }
            Step::Select(s) => {
                std::mem::swap(s, t.selection);
                Some(Done { selection: true, ..Done::default() })
            }
            Step::Profile(p) => {
                std::mem::swap(p, t.profile);
                Some(Done { region: crate::Region::Whole, other: true, ..Done::default() })
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
    pub label: &'a str,
    pub layer: u64,
    pub patch: &'a Patch<Rgba8>,
}

pub struct History {
    undo:   VecDeque<(String, Step)>,
    redo:   Vec<(String, Step)>,
    budget: usize,
    /// Bumped by every push, undo and redo: the same value later means
    /// nothing happened in between.
    serial: u64,
}

impl History {
    pub fn new(budget: usize) -> Self { Self { undo: VecDeque::new(), redo: Vec::new(), budget, serial: 0 } }

    pub(crate) fn push(&mut self, label: &str, step: Step) {
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
        // A step whose layer was deleted can't be applied: dropped, and the
        // previous one goes.
        while let Some((label, mut step)) = self.undo.pop_back() {
            if let Some(d) = step.swap_into(t) {
                self.redo.push((label, step));
                return Some(d);
            }
        }
        None
    }

    pub(crate) fn redo(&mut self, t: &mut Target<'_>) -> Option<Done> {
        self.serial += 1;
        while let Some((label, mut step)) = self.redo.pop() {
            if let Some(d) = step.swap_into(t) {
                if let Some((_, last)) = self.undo.back_mut() { last.pack(); }
                self.undo.push_back((label, step));
                self.trim();
                return Some(d);
            }
        }
        None
    }

    /// The newest `max_undo` undo steps and the redo steps, pixel steps
    /// only (a group's one by one, in the order they apply).
    pub(crate) fn export(&self, max_undo: usize) -> (Vec<Exported<'_>>, Vec<Exported<'_>>) {
        fn save<'a>(label: &'a str, s: &'a Step, redo: bool, out: &mut Vec<Exported<'a>>) {
            match s {
                Step::Pixels { layer, patch, .. } => out.push(Exported { label, layer: *layer, patch }),
                Step::Tree { .. } | Step::Select(_) | Step::Profile(_) => {}
                Step::Group { steps, .. } => {
                    let parts: Vec<&Step> = if redo { steps.iter().rev().collect() } else { steps.iter().collect() };
                    for p in parts { save(label, p, redo, out); }
                }
            }
        }
        let mut undo = Vec::new();
        for (l, s) in &self.undo { save(l, s, false, &mut undo); }
        let mut redo = Vec::new();
        for (l, s) in &self.redo { save(l, s, true, &mut redo); }
        let skip = undo.len().saturating_sub(max_undo);
        (undo.into_iter().skip(skip).collect(), redo)
    }

    /// A history read from a file: pixel steps, oldest first; redo steps
    /// with the next one to redo last.
    pub(crate) fn import(budget: usize, undo: Vec<(String, u64, Patch<Rgba8>)>, redo: Vec<(String, u64, Patch<Rgba8>)>) -> Self {
        let step = |(label, layer, patch)| (label, Step::Pixels { layer, patch, fill: None });
        let mut h = Self { undo: undo.into_iter().map(step).collect(), redo: redo.into_iter().map(step).collect(), budget, serial: 0 };
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

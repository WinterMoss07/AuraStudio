//! Undo / redo of one document, within a byte budget. A pixel step keeps
//! the touched rectangle, LZ4-compressed, swapped with the layer on undo
//! (no extra copy); a layer step keeps the tree as it was and the nodes
//! only one side has.

use std::collections::VecDeque;

use aurastudio_canvas::Rect;
use aurastudio_layers::{LayerStack, Node, Selection, TreeState};

use crate::Change;

/// Default memory budget for one document's history (compressed bytes).
pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;
/// Upper bound on undo steps, whatever their size.
const MAX_STEPS: usize = 200;

enum Step {
    Pixels(PixelStep),
    Tree(TreeStep),
    /// The selection to go back to.
    Select(&'static str, Option<Selection>),
    /// Steps done as one (a transform: its layers and the selection);
    /// `undone`: which way the next swap goes.
    Group { label: &'static str, steps: Vec<Step>, undone: bool },
}

struct PixelStep {
    label:    &'static str,
    layer_id: u64,
    rect:     Rect,
    /// LZ4-compressed RGBA of `rect`, to write back on undo / redo.
    data:     Vec<u8>,
}

struct TreeStep {
    label: &'static str,
    /// The tree to go back to.
    state: TreeState,
    /// Nodes it has that the tree hasn't (deleted ones), with pixels.
    kept:  Vec<Node>,
    bytes: usize,
}

impl Step {
    fn label(&self) -> &'static str {
        match self { Step::Pixels(p) => p.label, Step::Tree(t) => t.label, Step::Select(l, _) => l, Step::Group { label, .. } => label }
    }

    fn bytes(&self) -> usize {
        match self {
            Step::Pixels(p) => p.data.len(),
            Step::Tree(t) => t.bytes,
            Step::Select(_, s) => s.as_ref().map_or(0, Selection::bytes),
            Step::Group { steps, .. } => steps.iter().map(Step::bytes).sum(),
        }
    }

    /// Undo / redo it: the step now goes the other way. None (and dropped)
    /// if it can't be (its layer is gone).
    fn swap_into(&mut self, layers: &mut LayerStack, selection: &mut Option<Selection>) -> Option<Change> {
        match self {
            Step::Pixels(p) => p.swap_into(layers).then(|| p.change()),
            Step::Tree(t) => {
                let now = layers.tree_state();
                let made = layers.restore_tree(&t.state, std::mem::take(&mut t.kept));
                t.state = now;
                t.bytes = made.iter().map(Node::bytes).sum();
                t.kept = made;
                Some(Change { layer_id: layers.active_id(), rect: (0, 0, layers.width, layers.height), tree: true, selection: false, more: Vec::new() })
            }
            Step::Select(_, s) => {
                std::mem::swap(s, selection);
                Some(Change { layer_id: layers.active_id(), rect: (0, 0, 0, 0), tree: false, selection: true, more: Vec::new() })
            }
            Step::Group { steps, undone, .. } => {
                // Back in the reverse order, forward in the order done.
                let mut changes = Vec::new();
                let n = steps.len();
                for k in 0..n {
                    let i = if *undone { k } else { n - 1 - k };
                    if let Some(c) = steps[i].swap_into(layers, selection) { changes.push(c); }
                }
                *undone = !*undone;
                let mut it = changes.into_iter();
                let mut first = it.next()?;
                for c in it {
                    first.tree |= c.tree;
                    first.selection |= c.selection;
                    if c.rect.2 > c.rect.0 { first.more.push((c.layer_id, c.rect)); }
                    first.more.extend(c.more);
                }
                Some(first)
            }
        }
    }
}

impl PixelStep {
    fn new(label: &'static str, layer_id: u64, rect: Rect, pixels: &[u8]) -> Self {
        Self { label, layer_id, rect, data: lz4_flex::compress_prepend_size(pixels) }
    }

    /// Put this step's pixels back and keep the replaced ones in it, so
    /// the same step can go the other way. False if its layer is gone.
    fn swap_into(&mut self, layers: &mut LayerStack) -> bool {
        let Some(current) = layers.layer_rect(self.layer_id, self.rect) else { return false };
        let Ok(saved) = lz4_flex::decompress_size_prepended(&self.data) else { return false };
        if !layers.set_layer_rect(self.layer_id, self.rect, &saved) { return false; }
        self.data = lz4_flex::compress_prepend_size(&current);
        true
    }

    fn change(&self) -> Change {
        Change { layer_id: self.layer_id, rect: self.rect, tree: false, selection: false, more: Vec::new() }
    }
}

/// One undo / redo step as stored in a file: its pixels stay
/// LZ4-compressed, exactly as the history keeps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedStep {
    pub label:    String,
    pub layer_id: u64,
    pub rect:     Rect,
    pub data:     Vec<u8>,
}

/// Step labels the app knows; anything else read from a file becomes a
/// generic one (labels are static strings).
const LABELS: [&str; 5] = ["Brush stroke", "Clear layer", "Fill", "Paste", "Transform"];

pub struct History {
    undo:   VecDeque<Step>,
    redo:   Vec<Step>,
    budget: usize,
}

impl History {
    pub fn new(budget: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), budget }
    }

    /// Record an action: `before` is the RGBA of `rect` before it.
    pub fn push(&mut self, label: &'static str, layer_id: u64, rect: Rect, before: &[u8]) {
        if rect.0 >= rect.2 || rect.1 >= rect.3 { return; }
        self.redo.clear();
        self.undo.push_back(Step::Pixels(PixelStep::new(label, layer_id, rect, before)));
        self.trim();
    }

    /// Record a selection change: `before` is the selection before it.
    pub fn push_selection(&mut self, label: &'static str, before: Option<Selection>) {
        self.redo.clear();
        self.undo.push_back(Step::Select(label, before));
        self.trim();
    }

    /// Record pixel steps on several layers and maybe a selection change
    /// as one: (layer, rect, pixels before) each.
    pub fn push_group(&mut self, label: &'static str, pixels: Vec<(u64, Rect, Vec<u8>)>, selection: Option<Option<Selection>>) {
        let mut steps: Vec<Step> = pixels.into_iter()
            .filter(|(_, r, _)| r.0 < r.2 && r.1 < r.3)
            .map(|(id, r, px)| Step::Pixels(PixelStep::new(label, id, r, &px)))
            .collect();
        if let Some(s) = selection { steps.push(Step::Select(label, s)); }
        if steps.is_empty() { return; }
        self.redo.clear();
        self.undo.push_back(Step::Group { label, steps, undone: false });
        self.trim();
    }

    /// Record a change of the layer tree: `before` is the tree before it,
    /// `removed` the nodes it deleted.
    pub fn push_tree(&mut self, label: &'static str, before: TreeState, removed: Vec<Node>) {
        self.redo.clear();
        let bytes = removed.iter().map(Node::bytes).sum();
        self.undo.push_back(Step::Tree(TreeStep { label, state: before, kept: removed, bytes }));
        self.trim();
    }

    /// What the next undo would undo.
    pub fn undo_label(&self) -> Option<&'static str> { self.undo.back().map(Step::label) }

    pub fn redo_label(&self) -> Option<&'static str> { self.redo.last().map(Step::label) }

    pub fn undo(&mut self, layers: &mut LayerStack, selection: &mut Option<Selection>) -> Option<Change> {
        // A step whose layer was deleted can't be applied: drop it and go
        // on with the previous one.
        while let Some(mut step) = self.undo.pop_back() {
            if let Some(change) = step.swap_into(layers, selection) {
                self.redo.push(step);
                return Some(change);
            }
        }
        None
    }

    pub fn redo(&mut self, layers: &mut LayerStack, selection: &mut Option<Selection>) -> Option<Change> {
        while let Some(mut step) = self.redo.pop() {
            if let Some(change) = step.swap_into(layers, selection) {
                self.undo.push_back(step);
                self.trim();
                return Some(change);
            }
        }
        None
    }

    /// The newest `max_undo` undo steps and the redo steps, for saving:
    /// pixel steps only (they apply to the layers that still exist).
    pub fn export(&self, max_undo: usize) -> (Vec<SavedStep>, Vec<SavedStep>) {
        // A group's pixel steps are saved one by one (undone one at a time
        // once opened), in the order they apply.
        fn save(s: &Step, redo: bool, out: &mut Vec<SavedStep>) {
            match s {
                Step::Pixels(p) => out.push(SavedStep { label: p.label.to_owned(), layer_id: p.layer_id, rect: p.rect, data: p.data.clone() }),
                Step::Tree(_) | Step::Select(..) => {}
                Step::Group { steps, .. } => {
                    let parts: Vec<&Step> = if redo { steps.iter().rev().collect() } else { steps.iter().collect() };
                    for p in parts { save(p, redo, out); }
                }
            }
        }
        let mut undo = Vec::new();
        for s in &self.undo { save(s, false, &mut undo); }
        let mut redo = Vec::new();
        for s in &self.redo { save(s, true, &mut redo); }
        let skip = undo.len().saturating_sub(max_undo);
        (undo.into_iter().skip(skip).collect(), redo)
    }

    /// History read from a file. Steps whose rectangle is empty are
    /// dropped; one that no longer fits its layer is dropped when undone,
    /// like a step of a deleted layer.
    pub fn import(budget: usize, undo: Vec<SavedStep>, redo: Vec<SavedStep>) -> Self {
        let load = |s: SavedStep| {
            let label = LABELS.iter().find(|l| **l == s.label).copied().unwrap_or("Action");
            (s.rect.0 < s.rect.2 && s.rect.1 < s.rect.3)
                .then_some(Step::Pixels(PixelStep { label, layer_id: s.layer_id, rect: s.rect, data: s.data }))
        };
        let mut h = Self {
            undo: undo.into_iter().filter_map(load).collect(),
            redo: redo.into_iter().filter_map(load).collect(),
            budget,
        };
        h.trim();
        h
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize { self.undo.len() }

    pub fn redo_len(&self) -> usize { self.redo.len() }

    /// Compressed bytes held by the history.
    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(self.redo.iter()).map(Step::bytes).sum()
    }

    /// Drop the oldest steps until within budget (the newest step is
    /// always kept, even if alone over budget).
    fn trim(&mut self) {
        let mut bytes = self.bytes();
        while self.undo.len() > 1 && (bytes > self.budget || self.undo.len() > MAX_STEPS) {
            if let Some(old) = self.undo.pop_front() {
                bytes -= old.bytes();
            }
        }
    }
}

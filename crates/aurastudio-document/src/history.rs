//! Undo / redo of one document, within a byte budget. A pixel step keeps
//! the touched rectangle, LZ4-compressed, swapped with the layer on undo
//! (no extra copy); a layer step keeps the tree as it was and the nodes
//! only one side has.

use std::collections::VecDeque;

use aurastudio_canvas::Rect;
use aurastudio_layers::{LayerStack, Node, TreeState};

use crate::Change;

/// Default memory budget for one document's history (compressed bytes).
pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;
/// Upper bound on undo steps, whatever their size.
const MAX_STEPS: usize = 200;

enum Step {
    Pixels(PixelStep),
    Tree(TreeStep),
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
        match self { Step::Pixels(p) => p.label, Step::Tree(t) => t.label }
    }

    fn bytes(&self) -> usize {
        match self { Step::Pixels(p) => p.data.len(), Step::Tree(t) => t.bytes }
    }

    /// Undo / redo it: the step now goes the other way. None (and dropped)
    /// if it can't be (its layer is gone).
    fn swap_into(&mut self, layers: &mut LayerStack) -> Option<Change> {
        match self {
            Step::Pixels(p) => p.swap_into(layers).then(|| p.change()),
            Step::Tree(t) => {
                let now = layers.tree_state();
                let made = layers.restore_tree(&t.state, std::mem::take(&mut t.kept));
                t.state = now;
                t.bytes = made.iter().map(Node::bytes).sum();
                t.kept = made;
                Some(Change { layer_id: layers.active_id(), rect: (0, 0, layers.width, layers.height), tree: true })
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
        Change { layer_id: self.layer_id, rect: self.rect, tree: false }
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
const LABELS: [&str; 4] = ["Brush stroke", "Clear layer", "Fill", "Paste"];

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

    pub fn undo(&mut self, layers: &mut LayerStack) -> Option<Change> {
        // A step whose layer was deleted can't be applied: drop it and go
        // on with the previous one.
        while let Some(mut step) = self.undo.pop_back() {
            if let Some(change) = step.swap_into(layers) {
                self.redo.push(step);
                return Some(change);
            }
        }
        None
    }

    pub fn redo(&mut self, layers: &mut LayerStack) -> Option<Change> {
        while let Some(mut step) = self.redo.pop() {
            if let Some(change) = step.swap_into(layers) {
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
        let save = |s: &Step| match s {
            Step::Pixels(p) => Some(SavedStep { label: p.label.to_owned(), layer_id: p.layer_id, rect: p.rect, data: p.data.clone() }),
            Step::Tree(_) => None,
        };
        let undo: Vec<SavedStep> = self.undo.iter().filter_map(save).collect();
        let skip = undo.len().saturating_sub(max_undo);
        (undo.into_iter().skip(skip).collect(), self.redo.iter().filter_map(save).collect())
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

//! Undo / redo history of one document.
//!
//! Memory first: a step only stores the rectangle an action touched, never
//! a whole layer (unless the action covered it), LZ4-compressed. A stroke
//! on an empty layer is mostly transparent pixels and shrinks a lot. The
//! history has a byte budget; the oldest steps are dropped beyond it.
//!
//! A step holds ONE buffer: the pixels to put back. Undoing swaps it with
//! the current pixels, which become the redo data, so undo and redo cost
//! no extra copy of the image.

use std::collections::VecDeque;

use aurastudio_canvas::Rect;
use aurastudio_layers::LayerStack;

use crate::Change;

/// Default memory budget for one document's history (compressed bytes).
pub const DEFAULT_BUDGET: usize = 512 * 1024 * 1024;
/// Upper bound on undo steps, whatever their size.
const MAX_STEPS: usize = 200;

struct Step {
    label:    &'static str,
    layer_id: u64,
    rect:     Rect,
    /// LZ4-compressed RGBA of `rect`, to write back on undo / redo.
    data:     Vec<u8>,
}

impl Step {
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
        Change { layer_id: self.layer_id, rect: self.rect, label: self.label }
    }
}

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
        self.undo.push_back(Step::new(label, layer_id, rect, before));
        self.trim();
    }

    pub fn undo(&mut self, layers: &mut LayerStack) -> Option<Change> {
        // A step whose layer was deleted can't be applied: drop it and go
        // on with the previous one.
        while let Some(mut step) = self.undo.pop_back() {
            if step.swap_into(layers) {
                let change = step.change();
                self.redo.push(step);
                return Some(change);
            }
        }
        None
    }

    pub fn redo(&mut self, layers: &mut LayerStack) -> Option<Change> {
        while let Some(mut step) = self.redo.pop() {
            if step.swap_into(layers) {
                let change = step.change();
                self.undo.push_back(step);
                self.trim();
                return Some(change);
            }
        }
        None
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize { self.undo.len() }

    pub fn redo_len(&self) -> usize { self.redo.len() }

    /// Compressed bytes held by the history.
    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(self.redo.iter()).map(|s| s.data.len()).sum()
    }

    /// Drop the oldest steps until within budget (the newest step is
    /// always kept, even if alone over budget).
    fn trim(&mut self) {
        let mut bytes = self.bytes();
        while self.undo.len() > 1 && (bytes > self.budget || self.undo.len() > MAX_STEPS) {
            if let Some(old) = self.undo.pop_front() {
                bytes -= old.data.len();
            }
        }
    }
}

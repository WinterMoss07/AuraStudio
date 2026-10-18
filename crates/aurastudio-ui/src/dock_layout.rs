//! Minimum heights for stacked docks.
//!
//! egui_dock lets a separator squash a dock down to its tab bar, hiding its
//! content. Here every dock has a minimum height (its content, measured
//! while drawing it, or a fixed minimum for lists that scroll), and a
//! separator dragged into a dock that is at its minimum pushes the docks
//! beyond it instead (CSP / Krita behaviour); once they are all at their
//! minimum, the separator stops.
//!
//! Docks stacked in one column are nested vertical splits whose fractions
//! are relative to their parent. They are handled as a flat list of items
//! with absolute heights: the drag moved one boundary, every other boundary
//! stays where it was, and the fractions are recomputed from the heights.

use std::collections::HashMap;

use egui_dock::{DockState, Node, NodeIndex, Tree};

use crate::dockers::Tab;

/// Heights and fractions of each column after the last frame, to tell
/// which separator moved.
#[derive(Default)]
pub struct DockLayout {
    /// Content height of each tab, measured when it was last drawn.
    pub content: HashMap<Tab, f32>,
    /// Per column (keyed by its top node): item heights and the fraction of
    /// each vertical split in it.
    columns: HashMap<usize, Column>,
}

struct Column {
    items:     Vec<NodeIndex>,
    heights:   Vec<f32>,
    fractions: Vec<(NodeIndex, f32)>,
}

impl DockLayout {
    /// Minimum body height of a tab.
    fn tab_min(&self, tab: Tab) -> f32 {
        match tab {
            // Lists that scroll by themselves: a few rows are enough.
            Tab::Layers => 140.0,
            Tab::Brushes => 150.0,
            // Everything else: its whole content, or a guess until drawn.
            // (+6: slack, so no scroll bar shows at the minimum.)
            _ => self.content.get(&tab).map_or(120.0, |h| h + 6.0),
        }
    }

    /// Apply minimum heights to the main dock area (after it was shown).
    pub fn enforce(&mut self, dock: &mut DockState<Tab>, bar: f32, sep: f32) {
        let tree = dock.main_surface_mut();
        let mut roots = Vec::new();
        column_roots(tree, NodeIndex::root(), false, &mut roots);
        let mut seen = Vec::new();
        for root in roots {
            seen.push(root.0);
            self.enforce_column(tree, root, bar, sep);
        }
        self.columns.retain(|k, _| seen.contains(k));
    }

    fn enforce_column(&mut self, tree: &mut Tree<Tab>, root: NodeIndex, bar: f32, sep: f32) {
        let Some(rect) = node(tree, root).and_then(|n| n.rect()) else { return };
        if !rect.is_finite() || rect.height() <= 0.0 { return };
        let mut items = Vec::new();
        column_items(tree, root, &mut items);
        if items.len() < 2 { return; }

        let mins: Vec<f32> = items.iter().map(|&i| self.min_height(tree, i, bar, sep)).collect();
        let total = rect.height();
        let from_fractions = heights_from_fractions(tree, root, total, sep);

        let mut heights = match self.columns.get(&root.0) {
            // Same docks as last frame: find the separator that moved.
            Some(prev) if prev.items == items && (prev.heights.iter().sum::<f32>() - from_fractions.iter().sum::<f32>()).abs() < 1.0 => {
                let moved = prev.fractions.iter()
                    .find(|(n, f)| node_fraction(tree, *n).is_some_and(|now| (now - f).abs() > 1e-5))
                    .map(|(n, _)| *n);
                match moved {
                    Some(split) => {
                        // Boundary index: last item of the split's upper part.
                        let mut upper = Vec::new();
                        column_items(tree, split.left(), &mut upper);
                        let k = items.iter().position(|i| Some(i) == upper.last()).unwrap_or(0);
                        let prev_pos: f32 = prev.heights[..=k].iter().sum();
                        let now_pos: f32 = from_fractions[..=k].iter().sum();
                        let mut h = prev.heights.clone();
                        move_boundary(&mut h, &mins, k, now_pos - prev_pos);
                        h
                    }
                    None => from_fractions,
                }
            }
            _ => from_fractions,
        };
        fit_minimums(&mut heights, &mins);

        // Back to fractions (each split: upper part / whole, separators
        // included as egui_dock lays them out).
        let mut fractions = Vec::new();
        set_fractions(tree, root, &items, &heights, sep, &mut fractions);
        self.columns.insert(root.0, Column { items, heights, fractions });
    }

    /// Smallest height of a column item (a dock, or docks side by side).
    fn min_height(&self, tree: &Tree<Tab>, i: NodeIndex, bar: f32, sep: f32) -> f32 {
        match node(tree, i) {
            Some(Node::Leaf { tabs, .. }) => bar + tabs.iter().map(|&t| self.tab_min(t)).fold(0.0, f32::max),
            Some(Node::Horizontal { .. }) => {
                self.min_height(tree, i.left(), bar, sep).max(self.min_height(tree, i.right(), bar, sep))
            }
            Some(Node::Vertical { .. }) => {
                self.min_height(tree, i.left(), bar, sep) + sep + self.min_height(tree, i.right(), bar, sep)
            }
            _ => 0.0,
        }
    }
}

fn node(tree: &Tree<Tab>, i: NodeIndex) -> Option<&Node<Tab>> {
    (i.0 < tree.len()).then(|| &tree[i])
}

fn node_fraction(tree: &Tree<Tab>, i: NodeIndex) -> Option<f32> {
    match node(tree, i)? {
        Node::Vertical { fraction, .. } => Some(*fraction),
        _ => None,
    }
}

/// Top nodes of every column: the root, and below any non-vertical node.
fn column_roots(tree: &Tree<Tab>, i: NodeIndex, parent_vertical: bool, out: &mut Vec<NodeIndex>) {
    let Some(n) = node(tree, i) else { return };
    let vertical = matches!(n, Node::Vertical { .. });
    if vertical && !parent_vertical { out.push(i); }
    if n.is_parent() {
        column_roots(tree, i.left(), vertical, out);
        column_roots(tree, i.right(), vertical, out);
    }
}

/// The items of a column, top to bottom.
fn column_items(tree: &Tree<Tab>, i: NodeIndex, out: &mut Vec<NodeIndex>) {
    match node(tree, i) {
        Some(Node::Vertical { .. }) => {
            column_items(tree, i.left(), out);
            column_items(tree, i.right(), out);
        }
        Some(Node::Empty) | None => {}
        Some(_) => out.push(i),
    }
}

/// Item heights as the current fractions lay them out.
fn heights_from_fractions(tree: &Tree<Tab>, i: NodeIndex, h: f32, sep: f32) -> Vec<f32> {
    match node(tree, i) {
        Some(Node::Vertical { fraction, .. }) => {
            let mut v = heights_from_fractions(tree, i.left(), h * fraction - sep * 0.5, sep);
            v.extend(heights_from_fractions(tree, i.right(), h * (1.0 - fraction) - sep * 0.5, sep));
            v
        }
        Some(Node::Empty) | None => Vec::new(),
        Some(_) => vec![h],
    }
}

/// Move the boundary below item `k` by `delta` (negative: up). The items
/// it moves into shrink nearest first, down to their minimum (pushing the
/// next boundaries along); what can't be taken is not moved.
fn move_boundary(h: &mut [f32], mins: &[f32], k: usize, delta: f32) {
    if k + 1 >= h.len() { return; }
    let mut need = delta.abs();
    let shrinking: Vec<usize> = if delta < 0.0 { (0..=k).rev().collect() } else { (k + 1..h.len()).collect() };
    for i in shrinking {
        let take = (h[i] - mins[i]).max(0.0).min(need);
        h[i] -= take;
        need -= take;
        if need <= 0.0 { break; }
    }
    let moved = delta.abs() - need;
    if delta < 0.0 { h[k + 1] += moved } else { h[k] += moved }
}

/// Raise any item under its minimum, taking the height from the items that
/// have the most to spare (window resized, docks rearranged, first frame).
fn fit_minimums(h: &mut [f32], mins: &[f32]) {
    for i in 0..h.len() {
        let mut need = mins[i] - h[i];
        while need > 0.01 {
            let Some(j) = (0..h.len()).filter(|&j| j != i && h[j] - mins[j] > 0.01)
                .max_by(|&a, &b| (h[a] - mins[a]).total_cmp(&(h[b] - mins[b]))) else { break };
            let take = (h[j] - mins[j]).min(need);
            h[j] -= take;
            h[i] += take;
            need -= take;
        }
    }
}

/// Set the fraction of every vertical split of the column from the item
/// heights; returns the subtree height. Records each split's fraction.
fn set_fractions(tree: &mut Tree<Tab>, i: NodeIndex, items: &[NodeIndex], h: &[f32], sep: f32,
                 out: &mut Vec<(NodeIndex, f32)>) -> f32 {
    if !matches!(node(tree, i), Some(Node::Vertical { .. })) {
        return items.iter().position(|&x| x == i).map_or(0.0, |k| h[k]);
    }
    let upper = set_fractions(tree, i.left(), items, h, sep, out);
    let lower = set_fractions(tree, i.right(), items, h, sep, out);
    let whole = upper + lower + sep;
    if let Node::Vertical { fraction, .. } = &mut tree[i] {
        if whole > 0.0 {
            *fraction = ((upper + sep * 0.5) / whole).clamp(0.0, 1.0);
        }
        out.push((i, *fraction));
    }
    whole
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_into_a_dock_at_its_minimum_pushes_the_one_beyond() {
        // Color 200, Tool options 300 (min 300), Layers 300. Drag the
        // Tool options / Layers boundary up by 100: Tool options can't
        // shrink, so Color gives the 100.
        let mut h = vec![200.0, 300.0, 300.0];
        move_boundary(&mut h, &[100.0, 300.0, 150.0], 1, -100.0);
        assert_eq!(h, vec![100.0, 300.0, 400.0]);
    }

    #[test]
    fn separator_stops_when_everything_is_at_its_minimum() {
        let mut h = vec![200.0, 300.0, 300.0];
        move_boundary(&mut h, &[100.0, 300.0, 150.0], 1, -500.0);
        assert_eq!(h, vec![100.0, 300.0, 400.0]);
    }

    #[test]
    fn small_docks_get_their_minimum_back() {
        let mut h = vec![500.0, 20.0, 280.0];
        fit_minimums(&mut h, &[100.0, 300.0, 150.0]);
        assert_eq!(h, vec![220.0, 300.0, 280.0]);
    }
}

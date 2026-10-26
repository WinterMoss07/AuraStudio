//! Dock tabs, one file each. A docker only draws and edits what the app
//! lends it through `Model`; the data itself lives in the data crates
//! (aurastudio-layers, aurastudio-brush...), which know nothing about egui.

pub mod canvas;
pub mod cel_template;
pub mod color;
pub mod layers;
pub mod tool_list;
pub mod tool_options;

use egui::Ui;
use egui_dock::{DockState, NodeIndex, TabViewer};

use crate::{Actions, Model, Notice};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tab { Color, ToolOptions, Tools, Layers, Canvas, CelTemplate }

impl Tab {
    /// In the Window menu's order.
    pub const ALL: [Tab; 6] = [Tab::Color, Tab::Tools, Tab::ToolOptions, Tab::Layers, Tab::Canvas, Tab::CelTemplate];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Color       => "Color",
            Tab::ToolOptions => "Tool options",
            Tab::Tools       => "Tools",
            Tab::Layers      => "Layers",
            Tab::Canvas      => "Canvas",
            Tab::CelTemplate => "Cel template",
        }
    }

    /// The docker it shares a dock with by default.
    fn partner(self) -> Option<Tab> {
        match self {
            Tab::Color => Some(Tab::Tools),
            Tab::Tools => Some(Tab::Color),
            Tab::Layers => Some(Tab::Canvas),
            Tab::Canvas => Some(Tab::Layers),
            Tab::ToolOptions | Tab::CelTemplate => None,
        }
    }
}

/// The docker is in the docks.
pub fn is_shown(dock: &DockState<Tab>, tab: Tab) -> bool { dock.find_tab(&tab).is_some() }

/// Show or hide a docker (Window menu). Shown again: beside its usual
/// partner if that one is there, else in a new dock at the bottom.
pub fn set_shown(dock: &mut DockState<Tab>, tab: Tab, on: bool) {
    match (dock.find_tab(&tab), on) {
        (Some(at), false) => { dock.remove_tab(at); }
        (None, true) => {
            let tree = dock.main_surface_mut();
            let beside = tab.partner().and_then(|p| tree.find_tab(&p));
            if let Some((node, _)) = beside {
                tree.set_focused_node(node);
                tree.push_to_focused_leaf(tab);
            } else if tree.root_node().is_none_or(|n| n.is_empty()) {
                tree.push_to_first_leaf(tab);
            } else {
                tree.split_below(NodeIndex::root(), 0.72, vec![tab]);
            }
            if let Some(at) = dock.find_tab(&tab) { dock.set_active_tab(at); }
        }
        _ => {}
    }
}

/// Right column, top to bottom: Color + Tools, Tool options, Layers +
/// Canvas.
pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Color, Tab::Tools]);
    let surface = dock.main_surface_mut();
    let [_, rest] = surface.split_below(NodeIndex::root(), 0.25, vec![Tab::ToolOptions]);
    surface.split_below(rest, 0.45, vec![Tab::Layers, Tab::Canvas]);
    dock
}

/// Routes each tab to its docker, lending it only the state it needs.
pub struct Viewer<'s, 'a> {
    pub color:  &'s mut color::ColorState,
    pub layers: &'s mut layers::LayersState,
    pub tool_list: &'s mut tool_list::ToolListState,
    pub cel: &'s mut cel_template::CelState,
    pub notice: &'s mut Notice,
    pub m:      &'s mut Model<'a>,
    pub a:      &'s mut Actions,
    /// Content height of each tab, for the docks' minimum heights.
    pub content: &'s mut std::collections::HashMap<Tab, f32>,
}

impl TabViewer for Viewer<'_, '_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText { tab.title().into() }

    // Docks stay in the dock column (no floating windows).
    fn allowed_in_windows(&self, _tab: &mut Self::Tab) -> bool { false }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        // No document open: grayed out (egui_dock gives each tab a fresh
        // ui, which doesn't inherit the dock's).
        if self.m.home { ui.disable(); }
        // Drawn in a scope to measure the content alone (the tab's own ui
        // is at least as tall as the dock).
        let content = ui.scope(|ui| match tab {
            Tab::Color       => color::show(ui, self.color, self.m),
            Tab::ToolOptions => tool_options::show(ui, &mut self.tool_list.previews, self.m, self.a),
            Tab::Tools       => tool_list::show(ui, self.tool_list, self.m, self.a),
            Tab::Layers | Tab::Canvas | Tab::CelTemplate if self.m.home => {
                ui.label(egui::RichText::new("No document").color(crate::theme::TEXT_WEAK));
            }
            Tab::Layers      => layers::show(ui, self.layers, self.notice, self.m, self.a),
            Tab::Canvas      => canvas::show(ui, self.m, self.a),
            Tab::CelTemplate => cel_template::show(ui, self.cel, self.m, self.a),
        }).response.rect;
        // Body margins included.
        self.content.insert(*tab, content.height() + 16.0);
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool { false }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The lists scroll themselves; the color docker keeps its size.
        [false, !matches!(tab, Tab::Layers | Tab::Tools | Tab::Color)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf_of(dock: &DockState<Tab>, tab: Tab) -> NodeIndex { dock.find_tab(&tab).unwrap().1 }

    /// Hidden and shown again from the Window menu: every docker comes
    /// back, beside its partner when that one is there; all of them can
    /// be hidden.
    #[test]
    fn dockers_hide_and_come_back() {
        let mut dock = default_dock();
        // (The Cel template: hidden at first, shown in animation work.)
        for tab in Tab::ALL { assert_eq!(is_shown(&dock, tab), tab != Tab::CelTemplate); }
        set_shown(&mut dock, Tab::Tools, false);
        assert!(!is_shown(&dock, Tab::Tools) && is_shown(&dock, Tab::Color));
        set_shown(&mut dock, Tab::Tools, true);
        assert_eq!(leaf_of(&dock, Tab::Tools), leaf_of(&dock, Tab::Color));
        // Its dock gone: in a new one.
        set_shown(&mut dock, Tab::ToolOptions, false);
        set_shown(&mut dock, Tab::ToolOptions, true);
        assert!(is_shown(&dock, Tab::ToolOptions));
        assert_ne!(leaf_of(&dock, Tab::ToolOptions), leaf_of(&dock, Tab::Layers));
        // All hidden, then all back.
        for tab in Tab::ALL { set_shown(&mut dock, tab, false); }
        for tab in Tab::ALL { assert!(!is_shown(&dock, tab)); }
        for tab in Tab::ALL { set_shown(&mut dock, tab, true); }
        for tab in Tab::ALL { assert!(is_shown(&dock, tab), "{tab:?}"); }
        assert_eq!(leaf_of(&dock, Tab::Layers), leaf_of(&dock, Tab::Canvas));
        // Twice the same: no copy.
        set_shown(&mut dock, Tab::Color, true);
        assert_eq!(dock.iter_all_tabs().filter(|t| *t.1 == Tab::Color).count(), 1);
    }
}

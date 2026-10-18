//! Dock tabs, one file each. A docker only draws and edits what the app
//! lends it through `Model`; the data itself lives in the data crates
//! (aurastudio-layers, aurastudio-brush...), which know nothing about egui.

pub mod brushes;
pub mod canvas;
pub mod color;
pub mod layers;
pub mod tool_options;

use egui::Ui;
use egui_dock::{DockState, NodeIndex, TabViewer};

use crate::{Actions, Model, Notice};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tab { Color, ToolOptions, Brushes, Layers, Canvas }

/// Right column, top to bottom: Color + Brushes, Tool options, Layers +
/// Canvas.
pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Color, Tab::Brushes]);
    let surface = dock.main_surface_mut();
    let [_, rest] = surface.split_below(NodeIndex::root(), 0.25, vec![Tab::ToolOptions]);
    surface.split_below(rest, 0.45, vec![Tab::Layers, Tab::Canvas]);
    dock
}

/// Routes each tab to its docker, lending it only the state it needs.
pub struct Viewer<'s, 'a> {
    pub color:  &'s mut color::ColorState,
    pub layers: &'s mut layers::LayersState,
    pub brushes: &'s mut brushes::BrushesState,
    pub notice: &'s mut Notice,
    pub m:      &'s mut Model<'a>,
    pub a:      &'s mut Actions,
    /// Content height of each tab, for the docks' minimum heights.
    pub content: &'s mut std::collections::HashMap<Tab, f32>,
}

impl TabViewer for Viewer<'_, '_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Color       => "Color",
            Tab::ToolOptions => "Tool options",
            Tab::Brushes     => "Brushes",
            Tab::Layers      => "Layers",
            Tab::Canvas      => "Canvas",
        }.into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        // Drawn in a scope to measure the content alone (the tab's own ui
        // is at least as tall as the dock).
        let content = ui.scope(|ui| match tab {
            Tab::Color       => color::show(ui, self.color, self.m),
            Tab::ToolOptions => tool_options::show(ui, &mut self.brushes.previews, self.m, self.a),
            Tab::Brushes     => brushes::show(ui, self.brushes, self.m, self.a),
            Tab::Layers | Tab::Canvas if self.m.home => {
                ui.label(egui::RichText::new("No document").color(crate::theme::TEXT_WEAK));
            }
            Tab::Layers      => layers::show(ui, self.layers, self.notice, self.m, self.a),
            Tab::Canvas      => canvas::show(ui, self.m, self.a),
        }).response.rect;
        // Body margins included.
        self.content.insert(*tab, content.height() + 16.0);
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool { false }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The layer and brush lists scroll by themselves; the color docker
        // has a fixed size, kept as its dock's minimum height (a scroll bar
        // showed there, by a pixel, with everything in view); nothing
        // scrolls sideways.
        [false, !matches!(tab, Tab::Layers | Tab::Brushes | Tab::Color)]
    }
}

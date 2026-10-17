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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab { Color, ToolOptions, Brushes, Layers, Canvas }

/// Right column: Color + Tool options + Brushes on top, Layers + Canvas
/// below.
pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Color, Tab::ToolOptions, Tab::Brushes]);
    dock.main_surface_mut().split_below(NodeIndex::root(), 0.42, vec![Tab::Layers, Tab::Canvas]);
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
        match tab {
            Tab::Color       => color::show(ui, self.color, self.m),
            Tab::ToolOptions => tool_options::show(ui, self.m),
            Tab::Brushes     => brushes::show(ui, self.brushes, self.m, self.a),
            Tab::Layers      => layers::show(ui, self.layers, self.notice, self.m, self.a),
            Tab::Canvas      => canvas::show(ui, self.m, self.a),
        }
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool { false }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The layer list scrolls by itself; nothing scrolls sideways.
        [false, !matches!(tab, Tab::Layers | Tab::Brushes)]
    }
}

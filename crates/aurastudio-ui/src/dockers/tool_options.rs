//! Tool options docker: the brush's name, then the settings it shows
//! (chosen with the eyes of the Advanced tool options docker, kept per
//! brush), each a full-width slider with its pen inputs' icon (CSP).

use egui::{RichText, Ui};

use crate::brush_settings::{by_id, row, shown};
use crate::dockers::Tab;
use crate::widgets::{icon_button, Icon};
use crate::Model;

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut crate::Actions) {
    if !m.tool.paints() {
        crate::tools::options(ui, m, a);
        return;
    }
    let Some(p) = m.presets.get(*m.preset) else { return };
    let name = if m.tool.is_shape() { format!("{} · {}", m.tool.name(), p.name) } else { p.name.clone() };
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, Icon::FilterLayer, "Advanced tool options: every setting of the brush", true) { a.show_docker = Some(Tab::BrushEditor); }
        });
    });
    ui.add_space(4.0);
    let w = ui.available_width();
    for id in shown(m.brush) {
        // (An eraser has no blend mode.)
        if id == "blend" && m.brush.erase { continue; }
        if let Some(st) = by_id(id) { row(ui, st, m.brush, w, None); }
    }
    if m.tool.is_shape() { crate::tools::shape_options(ui, m); }
    crate::tools::stay_inside_option(ui, m);
}

//! Tool options docker: the brush's name, then its settings as full-width
//! sliders (label and value inside: fits a narrow dock).

use egui::{RichText, Ui};

use crate::widgets::{blend_combo, hardness_slider, opacity_slider, size_slider};
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut crate::Actions) {
    if !m.tool.paints() {
        crate::tools::options(ui, m, a);
        return;
    }
    let Some(p) = m.presets.get(*m.preset) else { return };
    let name = if m.tool.is_shape() { format!("{} · {}", m.tool.name(), p.name) } else { p.name.clone() };
    ui.label(RichText::new(name).strong());
    ui.add_space(4.0);
    let w = ui.available_width();
    size_slider(ui, "dock_size", m.brush, w);
    opacity_slider(ui, "dock_opacity", m.brush, w);
    hardness_slider(ui, "dock_hardness", m.brush, w);
    if m.tool.is_shape() { crate::tools::shape_options(ui, m); }
    crate::tools::stay_inside_option(ui, m);
    if m.brush.erase { return; }
    ui.add_space(2.0);
    ui.label(RichText::new("Blend mode").small().color(theme::TEXT_WEAK));
    blend_combo(ui, "brush_blend_dock", &mut m.brush.blend, w);
}

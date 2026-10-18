//! Tool options docker: settings of the selected brush, as
//! full-width sliders with their label and value inside, so the dock can
//! be narrow without anything being cut off.

use egui::{RichText, Ui};

use crate::widgets::{blend_combo, hardness_slider, opacity_slider, size_slider};
use crate::dockers::brushes::engine_label;
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut crate::Actions) {
    if !m.tool.paints() {
        crate::tools::options(ui, m, a);
        return;
    }
    let label = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).small().color(theme::TEXT_WEAK)); };

    // Which brush these settings belong to (each brush keeps its own).
    let mypaint = m.presets.get(*m.preset).is_some_and(|p| p.engine.is_mypaint());
    if let Some(p) = m.presets.get(*m.preset) {
        let prefix = if *m.tool == crate::Tool::Brush { String::new() } else { format!("{} · ", m.tool.name()) };
        ui.label(RichText::new(format!("{prefix}{}", p.name)).strong());
        ui.label(RichText::new(format!("{}  ·  {}", p.group, engine_label(&p.engine))).small().color(theme::TEXT_WEAK));
        ui.add_space(4.0);
    }

    // Full-width sliders, label and value inside.
    let w = ui.available_width();
    size_slider(ui, "dock_size", m.brush, w);
    opacity_slider(ui, "dock_opacity", m.brush, w);
    hardness_slider(ui, "dock_hardness", m.brush, w);
    if m.tool.is_shape() { crate::tools::shape_options(ui, m); }

    if mypaint {
        // MyPaint brushes blend their dabs themselves (normal, eraser,
        // smudge...) and have their own pressure dynamics.
        ui.add_space(2.0);
        ui.label(RichText::new("Blending and pressure dynamics come from the MyPaint brush.").small().color(theme::TEXT_WEAK));
        return;
    }

    // Per-brush setting (CSP): each brush keeps its own blend mode.
    ui.add_space(2.0);
    label(ui, "Blend mode");
    let w = ui.available_width();
    blend_combo(ui, "brush_blend_dock", &mut m.brush.blend, w);
}

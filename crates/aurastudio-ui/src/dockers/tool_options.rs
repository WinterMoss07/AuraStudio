//! Tool options docker: settings of the selected brush. Each setting is a label above a full-width control, so the dock
//! can be narrow without anything being cut off.

use egui::{RichText, Ui};

use crate::widgets::{blend_combo, opacity_slider, size_slider};
use crate::dockers::brushes::engine_label;
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model) {
    let full = |ui: &mut Ui| {
        // Slider = track + value box; keep the total inside the dock.
        ui.spacing_mut().slider_width = (ui.available_width() - 64.0).max(40.0);
    };
    let label = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).small().color(theme::TEXT_WEAK)); };

    // Which brush these settings belong to (each brush keeps its own).
    let mypaint = m.presets.get(*m.preset).is_some_and(|p| p.engine.is_mypaint());
    if let Some(p) = m.presets.get(*m.preset) {
        ui.label(RichText::new(&p.name).strong());
        ui.label(RichText::new(format!("{}  ·  {}", p.group, engine_label(&p.engine))).small().color(theme::TEXT_WEAK));
        ui.add_space(2.0);
    }

    label(ui, "Size");
    full(ui);
    size_slider(ui, m.brush);

    label(ui, "Opacity");
    full(ui);
    opacity_slider(ui, m.brush);

    label(ui, "Hardness");
    full(ui);
    let mut hard = (m.brush.hardness * 100.0).round() as u32;
    if ui.add(egui::Slider::new(&mut hard, 0..=100).suffix(" %"))
        .on_hover_text("100 % = crisp edge. Lower values fade the edge out toward the center.")
        .changed()
    {
        m.brush.hardness = hard as f32 / 100.0;
    }

    if mypaint {
        // MyPaint brushes blend their dabs themselves (normal, eraser,
        // smudge...) and have their own pressure dynamics.
        ui.add_space(2.0);
        ui.label(RichText::new("Blending and pressure dynamics come from the MyPaint brush.").small().color(theme::TEXT_WEAK));
        return;
    }

    // Per-brush setting (CSP): each brush keeps its own blend mode.
    label(ui, "Blend mode");
    let w = ui.available_width();
    blend_combo(ui, "brush_blend_dock", &mut m.brush.blend, w);
}

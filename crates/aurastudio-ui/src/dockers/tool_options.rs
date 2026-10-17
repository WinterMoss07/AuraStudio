//! Tool options docker: settings of the current tool (only the brush for
//! now). Each setting is a label above a full-width control, so the dock
//! can be narrow without anything being cut off.

use egui::{RichText, Ui};

use crate::widgets::{blend_combo, opacity_slider, size_slider};
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model) {
    let full = |ui: &mut Ui| {
        // Slider = track + value box; keep the total inside the dock.
        ui.spacing_mut().slider_width = (ui.available_width() - 64.0).max(40.0);
    };
    let label = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).small().color(theme::TEXT_WEAK)); };

    label(ui, "Size");
    full(ui);
    size_slider(ui, m.brush);

    label(ui, "Opacity");
    full(ui);
    opacity_slider(ui, m.brush);

    label(ui, "Blend mode");
    let w = ui.available_width();
    blend_combo(ui, "brush_blend_dock", &mut m.brush.blend, w);

    label(ui, "Pressure smoothing");
    full(ui);
    ui.add(egui::Slider::new(m.pressure_smoothing, 0.0..=1.5).fixed_decimals(2))
        .on_hover_text("Limits sudden pressure jumps over a distance relative to the brush size.\n0 = raw pen pressure.");

    ui.add_space(4.0);
    label(ui, "Engine");
    ui.radio_value(m.use_hokusai, false, "Round pen")
        .on_hover_text("Continuous stroke, crisp and fast. Opacity never builds up within a stroke.");
    ui.radio_value(m.use_hokusai, true, "hokusai (libmypaint)")
        .on_hover_text("Dab-based engine, for textured / painterly brushes.");
}

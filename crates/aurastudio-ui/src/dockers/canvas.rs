//! Canvas docker: custom canvas size with presets and a RAM estimate.

use egui::{Color32, RichText, Ui};

use crate::{theme, Actions, Model};

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    ui.label(RichText::new(format!("Current: {} × {} px", m.layers.width, m.layers.height)).color(theme::TEXT_WEAK));
    ui.add_space(4.0);

    egui::Grid::new("canvas_size").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Width");
        ui.add(egui::DragValue::new(&mut m.canvas_edit.0).range(1..=m.max_canvas).suffix(" px"));
        ui.end_row();
        ui.label("Height");
        ui.add(egui::DragValue::new(&mut m.canvas_edit.1).range(1..=m.max_canvas).suffix(" px"));
        ui.end_row();
    });

    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        for (name, w, h) in [("Square", 2048, 2048), ("A4 300 dpi", 2480, 3508), ("Full HD", 1920, 1080), ("4K", 3840, 2160)] {
            if ui.small_button(name).on_hover_text(format!("{w} × {h}")).clicked() {
                *m.canvas_edit = (w.min(m.max_canvas), h.min(m.max_canvas));
            }
        }
    });

    // RAM: 4 bytes/px per layer, plus composite (4), stroke snapshot (4)
    // and stroke mask (1).
    let px = m.canvas_edit.0 as f64 * m.canvas_edit.1 as f64;
    let n = m.layers.layer_count();
    let mb = px * (4.0 * n as f64 + 9.0) / (1024.0 * 1024.0);
    ui.add_space(4.0);
    ui.label(RichText::new(format!("≈ {mb:.0} MB RAM with {n} layer(s) · content stays centered"))
        .small().color(theme::TEXT_WEAK));
    ui.add_space(4.0);

    let unchanged = *m.canvas_edit == (m.layers.width, m.layers.height);
    let button = egui::Button::new(RichText::new("Resize canvas").color(Color32::WHITE)).fill(theme::ACCENT);
    if ui.add_enabled(!unchanged, button).clicked() {
        a.resize_to = Some(*m.canvas_edit);
    }
}

//! Tool options docker: the brush's stroke preview, then its settings as
//! full-width sliders (label and value inside: fits a narrow dock).

use aurastudio_brushes::stroke_preview::Mode;
use egui::{RichText, Ui};

use crate::stroke_preview::{Slot, Stroke, StrokePreviews};
use crate::widgets::{blend_combo, hardness_slider, opacity_slider, size_slider};
use crate::{theme, Model, Tool};

/// Height of the stroke preview (points).
const STRIP: f32 = 56.0;

pub fn show(ui: &mut Ui, previews: &mut StrokePreviews, m: &mut Model, a: &mut crate::Actions) {
    if !m.tool.paints() {
        crate::tools::options(ui, m, a);
        return;
    }
    let Some(p) = m.presets.get(*m.preset) else { return };
    let name = if m.tool.is_shape() { format!("{} · {}", m.tool.name(), p.name) } else { p.name.clone() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), STRIP), egui::Sense::hover());
    // The preview's empty half shows the canvas desk's color.
    ui.painter().rect_filled(rect, 2.0, theme::DESK);
    let settings = aurastudio_brush::Brush { erase: *m.tool == Tool::Eraser, ..m.brush.clone() };
    let stroke = Stroke { preset: p, settings: &settings, shape: m.tool.stroke_shape(), mode: Mode::Detail };
    previews.show(ui, rect, Slot::ToolOptions, stroke, Some(&name));
    resp.on_hover_text(format!("{}  ·  {}", p.medium, p.author));
    ui.add_space(4.0);

    let w = ui.available_width();
    size_slider(ui, "dock_size", m.brush, w);
    opacity_slider(ui, "dock_opacity", m.brush, w);
    hardness_slider(ui, "dock_hardness", m.brush, w);
    if m.tool.is_shape() { crate::tools::shape_options(ui, m); }
    crate::tools::stay_inside_option(ui, m);
    // Build-up brushes blend their dabs themselves.
    if !p.blends() { return; }
    ui.add_space(2.0);
    ui.label(RichText::new("Blend mode").small().color(theme::TEXT_WEAK));
    blend_combo(ui, "brush_blend_dock", &mut m.brush.blend, w);
}

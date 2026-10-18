//! Tool options docker: settings of the selected brush. Each setting is a label above a full-width control, so the dock
//! can be narrow without anything being cut off.

use egui::{RichText, Ui};

use crate::widgets::{blend_combo, opacity_slider, size_slider};
use crate::dockers::brushes::engine_label;
use aurastudio_brushes::Engine;

use crate::{theme, Actions, Model};

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
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

    // Round brushes: continuous pen or dabs.
    if let Some(engine) = m.presets.get(*m.preset).map(|p| &p.engine).filter(|e| !e.is_mypaint()) {
        let dabs = matches!(engine, Engine::DabRound);
        label(ui, "Engine");
        ui.horizontal(|ui| {
            if ui.selectable_label(!dabs, "Continuous")
                .on_hover_text("One swept shape, no dabs: no rings when the pressure changes.\nBest for hard edges.")
                .clicked() && dabs
            {
                a.use_dabs = Some(false);
            }
            if ui.selectable_label(dabs, "Dabs")
                .on_hover_text("Round dabs that blend into each other: smooth soft edges and crossings,\nand much faster at big sizes.")
                .clicked() && !dabs
            {
                a.use_dabs = Some(true);
            }
        });
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

    label(ui, "Pressure smoothing");
    full(ui);
    ui.add(egui::Slider::new(m.pressure_smoothing, 0.0..=1.5).fixed_decimals(2))
        .on_hover_text("Limits sudden pressure jumps over a distance relative to the brush size.\n0 = raw pen pressure.");
}

//! Advanced tool options docker (Photoshop's Brush Settings, CSP's sub
//! tool detail): the sections on the left, each with its switch; the
//! chosen section's settings on the right, each with an eye that shows it
//! in the Tool options docker too.

use egui::{vec2, RichText, ScrollArea, Ui};

use crate::brush_settings::{all, mixer, row, set_shown, shown, Section};
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model) {
    let Some(p) = m.presets.get(*m.preset) else { return };
    ui.label(RichText::new(&p.name).strong());
    ui.add_space(4.0);
    let id = ui.make_persistent_id("brush_editor_section");
    let mut section: Section = ui.data(|d| d.get_temp(id)).unwrap_or(Section::Brush);
    let left = 132.0;
    ui.horizontal_top(|ui| {
        // The sections.
        ui.allocate_ui_with_layout(vec2(left, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(left);
            for s in Section::ALL {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    match s.enabled(m.brush) {
                        Some(mut on) => { if ui.checkbox(&mut on, "").changed() { s.set_enabled(m.brush, on); } }
                        None => { ui.add_space(20.0); }
                    }
                    if ui.selectable_label(section == s, s.name()).clicked() { section = s; }
                });
            }
        });
        ui.separator();
        // Its settings.
        ui.vertical(|ui| {
            let w = ui.available_width().max(120.0);
            ScrollArea::vertical().id_salt("brush_editor_rows").show(ui, |ui| {
                if section.enabled(m.brush) == Some(false) {
                    ui.label(RichText::new(format!("{} is off.", section.name())).small().color(theme::TEXT_WEAK));
                }
                let on_list = shown(m.brush);
                let list = if section == Section::Mixer { mixer() } else { all() };
                for st in list.iter().filter(|s| s.section == section) {
                    let was = on_list.contains(&st.id);
                    let mut on = was;
                    row(ui, st, m.brush, w, Some(&mut on));
                    if on != was { set_shown(m.brush, st.id, on); }
                }
            });
        });
    });
    ui.data_mut(|d| d.insert_temp(id, section));
}

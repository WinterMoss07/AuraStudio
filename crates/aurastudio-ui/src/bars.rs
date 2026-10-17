//! The two bars at the top: the menu bar, and the Photoshop-like tool
//! options bar under it (size and opacity of the current tool).

use egui::{Align, Frame, Layout, Margin, RichText};

use crate::dockers::{self, layers, Tab};
use crate::widgets::{opacity_slider, size_slider};
use crate::{theme, Actions, Model, Notice, UiState};

pub fn menu_bar(ctx: &egui::Context, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::TopBottomPanel::top("menu_bar")
        .exact_height(28.0)
        .frame(Frame::none().fill(theme::BACKDROP).inner_margin(Margin::symmetric(8.0, 0.0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("AuraStudio").strong().color(theme::TEXT));
                ui.add_space(8.0);

                ui.menu_button("Layer", |ui| {
                    if ui.button("New layer").clicked() { layers::new_layer(m, a); ui.close_menu(); }
                    if ui.button("New folder").clicked() { layers::new_folder(m, a); ui.close_menu(); }
                    if ui.button("Duplicate").clicked() {
                        m.layers.duplicate_active();
                        a.recomposite = true;
                        ui.close_menu();
                    }
                    if ui.add(egui::Button::new("Clear").shortcut_text("Del")).clicked() {
                        layers::clear_layer(&mut state.layers, &mut state.notice, m, a);
                        ui.close_menu();
                    }
                    if ui.button("Delete").clicked() {
                        layers::delete_layer(&mut state.notice, m, a);
                        ui.close_menu();
                    }
                });
                ui.menu_button("Canvas", |ui| {
                    if ui.button("Canvas size…").clicked() {
                        state.focus_tab = Some(Tab::Canvas);
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Reset docks").clicked() {
                        state.dock = dockers::default_dock();
                        ui.close_menu();
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = m.layers.layer_count();
                    ui.label(RichText::new(format!(
                        "{} × {} px  ·  {} layer{}",
                        m.layers.width, m.layers.height, n, if n > 1 { "s" } else { "" }
                    )).small().color(theme::TEXT_WEAK));
                });
            });
        });
}

pub fn options_bar(ctx: &egui::Context, notice: &mut Notice, m: &mut Model) {
    egui::TopBottomPanel::top("options_bar")
        .exact_height(34.0)
        .frame(Frame::none().fill(theme::PANEL).inner_margin(Margin::symmetric(10.0, 0.0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(if *m.use_hokusai { "hokusai brush" } else { "Round pen" }).strong());
                ui.separator();

                ui.label(RichText::new("Size").color(theme::TEXT_WEAK));
                ui.spacing_mut().slider_width = 150.0;
                size_slider(ui, m.brush);
                ui.add_space(6.0);

                ui.label(RichText::new("Opacity").color(theme::TEXT_WEAK));
                opacity_slider(ui, m.brush);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some((text, left)) = notice.current() {
                        ui.label(RichText::new(text).color(theme::ACCENT));
                        ui.ctx().request_repaint_after(left);
                    }
                });
            });
        });
}

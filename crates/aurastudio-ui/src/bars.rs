//! The two bars at the top: the menu bar, and the Photoshop-like tool
//! options bar under it (size and opacity of the current tool).

use egui::{Align, Frame, Layout, Margin, RichText};

use crate::dockers::{self, layers, Tab};
use crate::widgets::{opacity_slider, size_slider};
use crate::{theme, Actions, Model, Notice, UiState, ViewCmd};

pub fn menu_bar(ctx: &egui::Context, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::TopBottomPanel::top("menu_bar")
        .exact_height(28.0)
        .show_separator_line(false)
        .frame(Frame::none().fill(theme::BACKDROP).inner_margin(Margin::symmetric(8.0, 0.0)))
        .show(ctx, |ui| {
            // Flat menu titles: only hovered / open ones get a background.
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("AuraStudio").strong().color(theme::TEXT));
                ui.add_space(8.0);

                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(m.can_undo, egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                        a.undo = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(m.can_redo, egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() {
                        a.redo = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Layer", |ui| {
                    if ui.button("New layer").clicked() { layers::new_layer(m, a); ui.close_menu(); }
                    if ui.button("New folder").clicked() { layers::new_folder(m, a); ui.close_menu(); }
                    if ui.button("Duplicate").clicked() {
                        m.layers.duplicate_active();
                        a.recomposite = true;
                        ui.close_menu();
                    }
                    if ui.add(egui::Button::new("Clear").shortcut_text("Del")).clicked() {
                        // Done by the app (undoable, after any stroke in progress).
                        a.clear_layer = true;
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
                    for (label, key, cmd) in [
                        ("Zoom in", "+", ViewCmd::ZoomIn),
                        ("Zoom out", "-", ViewCmd::ZoomOut),
                        ("Actual size (100 %)", "1", ViewCmd::ActualSize),
                        ("Fit to window", "2", ViewCmd::Fit),
                        ("Rotate left 15°", "4", ViewCmd::RotateLeft),
                        ("Rotate right 15°", "6", ViewCmd::RotateRight),
                        ("Reset rotation", "5", ViewCmd::ResetRotation),
                    ] {
                        if ui.add(egui::Button::new(label).shortcut_text(key)).clicked() {
                            a.view = Some(cmd);
                            ui.close_menu();
                        }
                    }
                    ui.label(RichText::new("Wheel: zoom · Space+drag: pan · Shift+Space+drag: rotate · Ctrl+Space+drag: zoom")
                        .small().color(theme::TEXT_WEAK));
                    ui.separator();
                    // Display color management of the canvas (and color docker).
                    let profile = m.display_profile.unwrap_or("none found (sRGB)");
                    ui.label(RichText::new(format!("Monitor profile: {profile}")).small().color(theme::TEXT_WEAK));
                    ui.add_enabled_ui(m.display_profile.is_some(), |ui| {
                        ui.checkbox(m.color_managed, "Color-manage canvas");
                        ui.add_enabled_ui(*m.color_managed, |ui| {
                            ui.radio_value(m.perceptual, true, "Perceptual");
                            ui.radio_value(m.perceptual, false, "Relative colorimetric");
                        });
                    });
                    ui.separator();
                    if ui.button("Reset docks").clicked() {
                        state.dock = dockers::default_dock();
                        ui.close_menu();
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = m.layers.layer_count();
                    // Angle shown only when the canvas is turned.
                    let angle = if m.view_angle.abs() >= 0.05 { format!("  ·  {:.0}°", m.view_angle) } else { String::new() };
                    ui.label(RichText::new(format!(
                        "{} × {} px  ·  {} layer{}  ·  {:.0} %{}",
                        m.layers.width, m.layers.height, n, if n > 1 { "s" } else { "" },
                        m.view_zoom * 100.0, angle,
                    )).small().color(theme::TEXT_WEAK));
                });
            });
        });
}

/// Tool options bar, part of the same surface as the menu bar and the
/// docks: brush name, size and opacity sliders (value written inside).
pub fn options_bar(ctx: &egui::Context, notice: &mut Notice, m: &mut Model) {
    egui::TopBottomPanel::top("options_bar")
        .exact_height(36.0)
        .show_separator_line(false)
        .frame(Frame::none().fill(theme::PANEL).inner_margin(Margin::symmetric(10.0, 0.0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                let name = m.presets.get(*m.preset).map(|p| p.name.as_str()).unwrap_or("Brush");
                ui.label(RichText::new(name).strong());
                ui.add_space(10.0);
                size_slider(ui, "bar_size", m.brush, 190.0);
                ui.add_space(4.0);
                opacity_slider(ui, "bar_opacity", m.brush, 190.0);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some((text, left)) = notice.current() {
                        ui.label(RichText::new(text).color(theme::ACCENT));
                        ui.ctx().request_repaint_after(left);
                    }
                });
            });
        });
}

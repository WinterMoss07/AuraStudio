//! The two bars at the top: the menu bar, and the Photoshop-like tool
//! options bar under it (size and opacity of the current tool).

use egui::{Align, Frame, Layout, Margin, RichText};

use crate::dockers::{self, layers, Tab};
use crate::{theme, Actions, FileCmd, Model, Notice, UiState, ViewCmd};

pub fn menu_bar(ctx: &egui::Context, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::TopBottomPanel::top("menu_bar")
        .exact_height(28.0)
        .show_separator_line(false)
        .frame(Frame::none().fill(theme::BACKDROP).inner_margin(Margin::symmetric(8.0, 0.0)))
        .show(ctx, |ui| {
            // Flat menu titles: only hovered / open ones get a background.
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.horizontal_centered(|ui| {

                ui.menu_button("File", |ui| {
                    for (label, key, cmd) in [
                        ("New…", "Ctrl+N", FileCmd::New),
                        ("Open…", "Ctrl+O", FileCmd::Open),
                        ("Save", "Ctrl+S", FileCmd::Save),
                        ("Save As…", "Ctrl+Shift+S", FileCmd::SaveAs),
                        ("Export…", "Ctrl+Shift+E", FileCmd::Export),
                        ("Close", "Ctrl+W", FileCmd::Close),
                    ] {
                        // Only New and Open without a document.
                        let enabled = !m.home || matches!(cmd, FileCmd::New | FileCmd::Open);
                        if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(key)).clicked() {
                            a.file = Some(cmd);
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(m.can_undo, egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
                        a.undo = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(m.can_redo, egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() {
                        a.redo = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    for (label, key, cmd) in [("Copy", "Ctrl+C", crate::EditCmd::Copy), ("Paste", "Ctrl+V", crate::EditCmd::Paste)] {
                        if ui.add_enabled(!m.home, egui::Button::new(label).shortcut_text(key)).clicked() {
                            a.edit = Some(cmd);
                            ui.close_menu();
                        }
                    }
                });
                ui.add_enabled_ui(!m.home, |ui| ui.menu_button("Layer", |ui| {
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
                }));
                ui.add_enabled_ui(!m.home, |ui| ui.menu_button("Canvas", |ui| {
                    if ui.button("Canvas size…").clicked() {
                        state.focus_tab = Some(Tab::Canvas);
                        ui.close_menu();
                    }
                }));
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
                    ui.checkbox(m.hidpi_scaling, "HiDPI image scaling")
                        .on_hover_text("On: at 100 %, a picture has the same size on a 3K / 4K screen and on an HD one (it follows Windows' display scaling). Off: one image pixel per physical screen pixel, much smaller on 3K / 4K.");
                    ui.menu_button("Interface size", |ui| {
                        for s in crate::UI_SCALES {
                            let label = format!("{:.0} %", s * 100.0);
                            if ui.radio((*m.ui_scale - s).abs() < 1e-3, label).clicked() {
                                *m.ui_scale = s;
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Reset docks").clicked() {
                        state.dock = dockers::default_dock();
                        ui.close_menu();
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if m.home { return; }
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

/// Toolbar under the menu, part of the same surface as the menu bar and
/// the docks: fixed actions (each room will get its own toolbar); the
/// current tool's settings live in the Tool options docker.
pub fn toolbar(ctx: &egui::Context, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    use crate::icons::Icon;
    use crate::widgets::icon_button;
    egui::TopBottomPanel::top("options_bar")
        .exact_height(36.0)
        .show_separator_line(false)
        .frame(Frame::none().fill(theme::PANEL).inner_margin(Margin::symmetric(8.0, 0.0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let doc = !m.home;
                if icon_button(ui, Icon::FileNew, "New… (Ctrl+N)", true) { a.file = Some(FileCmd::New); }
                if icon_button(ui, Icon::FileOpen, "Open… (Ctrl+O)", true) { a.file = Some(FileCmd::Open); }
                if icon_button(ui, Icon::Save, "Save (Ctrl+S)", doc) { a.file = Some(FileCmd::Save); }
                ui.add_space(8.0);
                if icon_button(ui, Icon::Copy, "Copy (Ctrl+C)", doc) { a.edit = Some(crate::EditCmd::Copy); }
                if icon_button(ui, Icon::Paste, "Paste (Ctrl+V)", doc) { a.edit = Some(crate::EditCmd::Paste); }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some((text, left)) = notice.current() {
                        ui.label(RichText::new(text).color(theme::ACCENT));
                        ui.ctx().request_repaint_after(left);
                    }
                });
            });
        });
}

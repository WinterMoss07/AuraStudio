//! The two bars at the top: the menu bar, and the toolbar under it (file
//! and clipboard actions, notices).

use egui::{Align, Frame, Layout, Margin, RichText};

use crate::dockers::{self, layers, Tab};
use crate::{theme, Actions, FileCmd, Model, Notice, UiState, ViewCmd};

pub fn menu_bar(root: &mut egui::Ui, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::Panel::top("menu_bar")
        .exact_size(28.0)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme::BACKDROP).inner_margin(Margin::symmetric(8, 0)))
        .show(root, |ui| {
            // Flat menu titles: only hovered / open ones get a background.
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.horizontal_centered(|ui| {

                crate::widgets::menu(ui, "File", |ui| {
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
                            ui.close();
                        }
                    }
                });
                crate::widgets::menu(ui, "Edit", |ui| {
                    let undo = m.undo_label.map_or("Undo".to_owned(), |l| format!("Undo {l}"));
                    let redo = m.redo_label.map_or("Redo".to_owned(), |l| format!("Redo {l}"));
                    if ui.add_enabled(m.can_undo, egui::Button::new(undo).shortcut_text("Ctrl+Z")).clicked() {
                        a.undo = true;
                        ui.close();
                    }
                    if ui.add_enabled(m.can_redo, egui::Button::new(redo).shortcut_text("Ctrl+Y")).clicked() {
                        a.redo = true;
                        ui.close();
                    }
                    ui.separator();
                    for (label, key, cmd) in [("Copy", "Ctrl+C", crate::EditCmd::Copy), ("Paste", "Ctrl+V", crate::EditCmd::Paste)] {
                        if ui.add_enabled(!m.home, egui::Button::new(label).shortcut_text(key)).clicked() {
                            a.edit = Some(cmd);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.add_enabled(!m.home, egui::Button::new("Transform").shortcut_text("Ctrl+T")).clicked() {
                        *m.tool = crate::Tool::Transform;
                        ui.close();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Preferences…").shortcut_text("Ctrl+K")).clicked() {
                        state.open_prefs();
                        ui.close();
                    }
                });
                ui.add_enabled_ui(!m.home, |ui| crate::widgets::menu(ui, "Layer", |ui| {
                    if ui.button("New layer").clicked() { layers::new_layer(m, a); ui.close(); }
                    if ui.button("New folder").clicked() { layers::new_folder(m, a); ui.close(); }
                    if ui.button("Duplicate").clicked() {
                        layers::duplicate(&mut state.layers, m, a);
                        ui.close();
                    }
                    if ui.add(egui::Button::new("Clear").shortcut_text("Del")).clicked() {
                        // Done by the app (undoable, after any stroke in progress).
                        a.clear_layer = true;
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        layers::delete_layer(&mut state.layers, &mut state.notice, m, a);
                        ui.close();
                    }
                    ui.separator();
                    layers::menu_entries(ui, &mut state.layers, m, a);
                }));
                ui.add_enabled_ui(!m.home, |ui| crate::widgets::menu(ui, "Select", |ui| {
                    use crate::SelectCmd as C;
                    let sel = m.has_selection;
                    for (label, key, cmd, on) in [
                        ("All", "Ctrl+A", C::All, true),
                        ("Deselect", "Ctrl+D", C::None, sel),
                        ("Reselect", "Ctrl+Shift+D", C::Reselect, !sel),
                        ("Invert", "Ctrl+Shift+I", C::Invert, true),
                    ] {
                        if ui.add_enabled(on, egui::Button::new(label).shortcut_text(key)).clicked() {
                            a.select = Some(cmd);
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (label, kind) in [("Grow…", crate::SelectKind::Grow), ("Shrink…", crate::SelectKind::Shrink), ("Feather…", crate::SelectKind::Feather), ("Border…", crate::SelectKind::Border)] {
                        if ui.add_enabled(sel, egui::Button::new(label)).clicked() {
                            state.ask_select(kind);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("From layer").on_hover_text("Its opaque pixels").clicked() {
                        a.select = Some(C::FromLayer);
                        ui.close();
                    }
                }));
                ui.add_enabled_ui(!m.home, |ui| crate::widgets::menu(ui, "Canvas", |ui| {
                    if ui.button("Canvas size…").clicked() {
                        state.focus_tab = Some(Tab::Canvas);
                        ui.close();
                    }
                }));
                ui.add_enabled_ui(!m.home, |ui| crate::widgets::menu(ui, "Filter", |ui| {
                    crate::filters::menu(ui, &mut state.filter, m);
                }));
                crate::widgets::menu(ui, "View", |ui| {
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
                            ui.close();
                        }
                    }
                    ui.label(RichText::new("Wheel: zoom · Space+drag: pan · Shift+Space+drag: rotate · Ctrl+Space+drag: zoom")
                        .small().color(theme::TEXT_WEAK));
                });
                crate::widgets::menu(ui, "Window", |ui| {
                    // Each docker, checked when shown.
                    for tab in Tab::ALL.into_iter().filter(|t| *t != Tab::CelTemplate) {
                        let mut on = dockers::is_shown(&state.dock, tab);
                        if ui.checkbox(&mut on, tab.title()).changed() { dockers::set_shown(&mut state.dock, tab, on); }
                    }
                    ui.separator();
                    // The X-sheet / timeline (Animation documents).
                    let anim = m.doc_kind == aurastudio_document::Kind::Animation;
                    let mut on = state.anim.open;
                    if ui.add_enabled(anim, egui::Checkbox::new(&mut on, "Animation (X-sheet / timeline)")).on_disabled_hover_text("Animation documents only").changed() {
                        state.anim.open = on;
                    }
                    // (The Cel template goes with the timeline and the X-sheet.)
                    let mut on = dockers::is_shown(&state.dock, Tab::CelTemplate);
                    if ui.checkbox(&mut on, Tab::CelTemplate.title()).changed() { dockers::set_shown(&mut state.dock, Tab::CelTemplate, on); }
                    ui.separator();
                    ui.checkbox(&mut state.memory_open, "Memory report")
                        .on_hover_text("Every megabyte the app holds, by name, to compare with Task Manager");
                    ui.separator();
                    if ui.button("Reset docks").clicked() {
                        state.dock = dockers::default_dock();
                        ui.close();
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if m.home { return; }
                    let n = m.layers.layer_count();
                    // Angle shown only when the canvas is turned.
                    let angle = if m.view_angle.abs() >= 0.05 { format!("  ·  {:.0}°", m.view_angle) } else { String::new() };
                    // Animation: the current frame first.
                    let frame = if m.doc_kind == aurastudio_document::Kind::Animation { format!("Frame {}  ·  ", m.layers.time()) } else { String::new() };
                    ui.label(RichText::new(format!(
                        "{frame}{}  ·  {} × {} px  ·  {} layer{}  ·  {:.0} %{}",
                        m.doc_kind.name(), m.layers.width, m.layers.height, n, layers::plural(n),
                        m.view_zoom * 100.0, angle,
                    )).small().color(theme::TEXT_WEAK));
                });
            });
        });
}

/// Toolbar under the menu: fixed actions, and the notices on the right.
pub fn toolbar(root: &mut egui::Ui, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    use crate::icons::Icon;
    use crate::widgets::icon_button;
    egui::Panel::top("options_bar")
        .exact_size(36.0)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme::PANEL).inner_margin(Margin::symmetric(8, 0)))
        .show(root, |ui| {
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

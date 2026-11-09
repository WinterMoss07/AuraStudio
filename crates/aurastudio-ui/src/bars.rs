//! The two bars at the top: the title bar with the menus in it (PhotoCraft:
//! menus, the app's name, then the window's buttons, all on the UI's own
//! surface instead of the system's black bar), and the toolbar under it
//! (file and clipboard actions, the document's figures, notices).

use egui::{pos2, vec2, Align, Frame, Layout, Margin, Rect, RichText, Sense, Stroke};

use crate::dockers::{self, layers, Tab};
use crate::{theme, Actions, FileCmd, Model, Notice, UiState, ViewCmd, WindowCmd};

/// Height of the title bar (points), and width of a window button.
pub const TITLE_BAR: f32 = 32.0;
const WINDOW_BUTTON: f32 = 46.0;

pub fn menu_bar(root: &mut egui::Ui, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::Panel::top("menu_bar")
        .exact_size(TITLE_BAR)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme::BACKDROP).inner_margin(Margin { left: 8, ..Default::default() }))
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
                    let undo = m.doc.history().undo_label().map_or("Undo".to_owned(), |l| format!("Undo {l}"));
                    let redo = m.doc.history().redo_label().map_or("Redo".to_owned(), |l| format!("Redo {l}"));
                    if ui.add_enabled(m.can_undo(), egui::Button::new(undo).shortcut_text("Ctrl+Z")).clicked() {
                        a.undo = true;
                        ui.close();
                    }
                    if ui.add_enabled(m.can_redo(), egui::Button::new(redo).shortcut_text("Ctrl+Y")).clicked() {
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
                    if ui.button("New layer").clicked() { layers::new_layer(m); ui.close(); }
                    if ui.button("New folder").clicked() { layers::new_folder(m); ui.close(); }
                    if ui.button("Duplicate").clicked() {
                        layers::duplicate(&mut state.layers, m);
                        ui.close();
                    }
                    if ui.add(egui::Button::new("Clear").shortcut_text("Del")).clicked() {
                        // Done by the app (undoable, after any stroke in progress).
                        a.clear_layer = true;
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        layers::delete_layer(&mut state.layers, &mut state.notice, m);
                        ui.close();
                    }
                    ui.separator();
                    layers::menu_entries(ui, &mut state.layers, m, a);
                }));
                ui.add_enabled_ui(!m.home, |ui| crate::widgets::menu(ui, "Select", |ui| {
                    use crate::SelectCmd as C;
                    let sel = m.has_selection();
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
                    ui.separator();
                    for (label, cmd, on) in [("Mirror horizontally", ViewCmd::MirrorH, m.view_mirror.0), ("Mirror vertically", ViewCmd::MirrorV, m.view_mirror.1)] {
                        let mut v = on;
                        if ui.checkbox(&mut v, label).on_hover_text("Only the view: the picture is not changed").changed() {
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
                    let anim = m.doc_kind() == aurastudio_core::DocType::Animation;
                    let mut on = state.anim.open;
                    if ui.add_enabled(anim, egui::Checkbox::new(&mut on, "Animation (X-sheet / timeline)")).on_disabled_hover_text("Animation documents only").changed() {
                        state.anim.open = on;
                    }
                    // (The Cel template goes with the timeline and the X-sheet.)
                    let mut on = dockers::is_shown(&state.dock, Tab::CelTemplate);
                    if ui.checkbox(&mut on, Tab::CelTemplate.title()).changed() { dockers::set_shown(&mut state.dock, Tab::CelTemplate, on); }
                    ui.separator();
                    ui.add(egui::Checkbox::new(&mut state.advanced.open, "Advanced tool options")).on_hover_text("Every setting of the brush (F5)");
                    ui.add(egui::Checkbox::new(&mut state.advanced.brushes_open, "Brushes")).on_hover_text("Every brush, in a window of its own (F6)");
                    ui.separator();
                    ui.checkbox(&mut state.memory_open, "Memory report")
                        .on_hover_text("Every megabyte the app holds, by name, to compare with Task Manager");
                    ui.separator();
                    if ui.button("Reset docks").clicked() {
                        state.dock = dockers::default_dock();
                        ui.close();
                    }
                });

                // The room the window is moved by, the app's name centered.
                ui.add_space(14.0);
                let (from, bar) = (ui.cursor().min.x, ui.max_rect());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (cmd, tip) in [(WindowCmd::Close, "Close"), (WindowCmd::ToggleMaximize, if m.maximized { "Restore" } else { "Maximize" }), (WindowCmd::Minimize, "Minimize")] {
                        if window_button(ui, cmd, m.maximized).on_hover_text(tip).clicked() { a.window = Some(cmd); }
                    }
                    let room = Rect::from_min_max(pos2(from, ui.max_rect().top()), pos2(ui.cursor().max.x, ui.max_rect().bottom()));
                    // Centered on the window, unless the menus reach there.
                    let font = egui::FontId::proportional(14.0);
                    let galley = ui.painter().layout_no_wrap("AuraStudio".to_owned(), font, theme::TEXT);
                    let w = galley.size().x;
                    let x = (bar.center().x - w / 2.0).max(from).min(room.right() - w);
                    if x >= from {
                        ui.painter().galley(pos2(x, bar.center().y - galley.size().y / 2.0), galley, theme::TEXT);
                    }
                    if room.width() > 0.0 {
                        let r = ui.interact(room, egui::Id::unique("title_bar_room"), Sense::click_and_drag());
                        if r.double_clicked() { a.window = Some(WindowCmd::ToggleMaximize); }
                        else if r.drag_started() { a.window = Some(WindowCmd::Drag); }
                    }
                });
            });
        });
}

/// One of the window's buttons (minimize, maximize / restore, close), drawn
/// as thin lines; Close turns red under the pointer (Windows).
fn window_button(ui: &mut egui::Ui, cmd: WindowCmd, maximized: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(WINDOW_BUTTON, TITLE_BAR), Sense::click());
    let close = cmd == WindowCmd::Close;
    if resp.hovered() { ui.painter().rect_filled(rect, 0.0, if close { theme::CLOSE_RED } else { theme::WIDGET_HOVER }); }
    let c = rect.center();
    let s = 5.0;
    let line = Stroke::new(1.0, theme::TEXT);
    let p = ui.painter();
    match cmd {
        WindowCmd::Minimize => { p.hline(c.x - s..=c.x + s, c.y, line); }
        WindowCmd::ToggleMaximize if maximized => {
            // Two squares, the back one showing at the top right.
            let front = Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(2.0 * s - 2.0, 2.0 * s - 2.0));
            p.rect_stroke(front, 0.0, line, egui::StrokeKind::Middle);
            p.line_segment([pos2(front.left() + 2.0, front.top() - 2.0), pos2(front.right() + 2.0, front.top() - 2.0)], line);
            p.line_segment([pos2(front.right() + 2.0, front.top() - 2.0), pos2(front.right() + 2.0, front.bottom() - 2.0)], line);
        }
        WindowCmd::ToggleMaximize => { p.rect_stroke(Rect::from_center_size(c, vec2(2.0 * s, 2.0 * s)), 0.0, line, egui::StrokeKind::Middle); }
        WindowCmd::Close => {
            p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], line);
            p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], line);
        }
        WindowCmd::Drag => {}
    }
    resp
}

/// Toolbar under the menu: fixed actions.
pub fn toolbar(root: &mut egui::Ui, adv: &mut crate::advanced::AdvancedState, m: &mut Model, a: &mut Actions) {
    use crate::icons::Icon;
    use crate::widgets::{icon_button, icon_toggle};
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
                ui.add_space(8.0);
                // The view's mirrors (the picture stays as it is).
                if icon_toggle(ui, Icon::FlipH, "Mirror the view horizontally", doc, m.view_mirror.0).clicked() && doc { a.view = Some(ViewCmd::MirrorH); }
                if icon_toggle(ui, Icon::FlipV, "Mirror the view vertically", doc, m.view_mirror.1).clicked() && doc { a.view = Some(ViewCmd::MirrorV); }
                ui.add_space(8.0);
                // The brushes and every setting of the brush, in their windows.
                if icon_toggle(ui, Icon::Brush, "Brushes (F6)", doc, adv.brushes_open).clicked() && doc { adv.brushes_open = !adv.brushes_open; }
                if icon_toggle(ui, Icon::FilterLayer, "Advanced tool options (F5)", doc, adv.open).clicked() && doc { adv.open = !adv.open; }

            });
        });
}

/// The status bar at the bottom: what the document is, how it is viewed.
pub fn status_bar(root: &mut egui::Ui, m: &mut Model) {
    egui::Panel::bottom("status_bar")
        .exact_size(18.0)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(theme::PANEL).inner_margin(Margin::symmetric(10, 0)))
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                if m.home { return; }
                let n = m.layers().layer_count();
                // Angle shown only when the canvas is turned.
                let angle = if m.view_angle.abs() >= 0.05 { format!("  ·  {:.0}°", m.view_angle) } else { String::new() };
                // Animation: the current frame first.
                let frame = if m.doc_kind() == aurastudio_core::DocType::Animation { format!("Frame {}  ·  ", m.time()) } else { String::new() };
                ui.label(RichText::new(format!(
                    "{}  ·  {frame}{}  ·  {} × {} px  ·  {} layer{}  ·  {:.0} %{}",
                    m.doc.name, m.doc_kind().name(), m.layers().width, m.layers().height, n, layers::plural(n),
                    m.view_zoom * 100.0, angle,
                )).small().color(theme::TEXT_WEAK));
            });
        });
}

/// The notice of the moment, over the canvas at its top left (Krita).
pub fn notice_overlay(ctx: &egui::Context, canvas: Rect, notice: &mut Notice) {
    let Some((text, left)) = notice.current() else { return };
    egui::Area::new(egui::Id::unique("canvas_notice"))
        .fixed_pos(canvas.min + vec2(12.0, 12.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(egui::Color32::from_black_alpha(190))
                .corner_radius(4)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| { ui.label(RichText::new(text).color(theme::TEXT)); });
        });
    ctx.request_repaint_after(left);
}

//! The Animation panel: the X-sheet (vertical, beside the dockers) or the
//! timeline (horizontal, under the canvas): the same grid of animation
//! folders × frames, turned (OpenToonz's orientations). A drawn frame is a
//! filled cell, a hold a line until the next one. On top: new folders, the
//! playback bar, timing edits, the orientation.
//!
//! Everything is laid out on two axes: the frame axis (time) and the
//! layer axis (one folder per column / row); `Grid` maps them to the
//! screen either way.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use aurastudio_layers::{LayerStack, Onto, Shift};

use crate::icons::Icon;
use crate::widgets::{icon_button, icon_toggle};
use crate::{theme, Actions, Model};

/// Cell size along (layer axis, frame axis), and the header sizes: the
/// folders' names (layer header) and the frame numbers (frame header).
struct Sizes { layer: f32, frame: f32, layer_head: f32, frame_head: f32 }

const XSHEET: Sizes = Sizes { layer: 78.0, frame: 20.0, layer_head: 52.0, frame_head: 40.0 };
const TIMELINE: Sizes = Sizes { layer: 30.0, frame: 22.0, layer_head: 170.0, frame_head: 24.0 };

/// The panel's state (kept by the UI).
pub struct AnimPanel {
    /// Shown (Window menu), in Animation documents.
    pub open:     bool,
    /// X-sheet (true) or timeline.
    pub vertical: bool,
    /// Scroll along the frame axis and the layer axis, points.
    scroll:       Vec2,
    /// The frame last kept in view (scrolled to when it changes).
    followed:     u32,
    /// A drawn frame dragged: folder, from, to (the cell under the pointer).
    drag:         Option<(u64, u32, u32)>,
    /// The frame header or a cell dragged to change the current frame.
    scrubbing:    bool,
    /// A move / copy onto a drawn frame, waiting for the answer: folder,
    /// from, to, copy.
    ask:          Option<(u64, u32, u32, bool)>,
    /// A folder being renamed: id, text, focus asked.
    rename:       Option<(u64, String, bool)>,
}

impl Default for AnimPanel {
    fn default() -> Self {
        Self { open: true, vertical: false, scroll: Vec2::ZERO, followed: 0, drag: None, scrubbing: false, ask: None, rename: None }
    }
}

impl AnimPanel {
    /// A question is open (the canvas waits).
    pub fn is_asking(&self) -> bool { self.ask.is_some() }
}

/// Screen place of (column, frame) cells; `col` counts from the first
/// shown column, frames from 1.
struct Grid { vertical: bool, cells: Rect, layer: f32, frame: f32, scroll: Vec2 }

impl Grid {
    fn cell(&self, col: usize, frame: u32) -> Rect {
        let (l, f) = (col as f32 * self.layer - self.scroll.y, (frame as f32 - 1.0) * self.frame - self.scroll.x);
        if self.vertical {
            Rect::from_min_size(self.cells.min + vec2(l, f), vec2(self.layer, self.frame))
        } else {
            Rect::from_min_size(self.cells.min + vec2(f, l), vec2(self.frame, self.layer))
        }
    }

    /// The frame under `p` (its frame axis only).
    fn frame_at(&self, p: Pos2) -> u32 {
        let along = if self.vertical { p.y - self.cells.top() } else { p.x - self.cells.left() } + self.scroll.x;
        (along / self.frame).floor().max(0.0) as u32 + 1
    }

    fn col_at(&self, p: Pos2) -> usize {
        let across = if self.vertical { p.x - self.cells.left() } else { p.y - self.cells.top() } + self.scroll.y;
        (across / self.layer).floor().max(0.0) as usize
    }

    /// Frames shown in full or in part.
    fn frames_shown(&self) -> (u32, u32) {
        let len = if self.vertical { self.cells.height() } else { self.cells.width() };
        let first = (self.scroll.x / self.frame).floor() as u32 + 1;
        (first, first + (len / self.frame).ceil() as u32 + 1)
    }

    /// The frame header strip at `frame` (beside / above its row).
    fn frame_head(&self, head: Rect, frame: u32) -> Rect {
        let c = self.cell(0, frame);
        if self.vertical { Rect::from_min_max(pos2(head.left(), c.top()), pos2(head.right(), c.bottom())) }
        else { Rect::from_min_max(pos2(c.left(), head.top()), pos2(c.right(), head.bottom())) }
    }

    /// The layer header of column `col`.
    fn layer_head(&self, head: Rect, col: usize) -> Rect {
        let c = self.cell(col, 1);
        if self.vertical { Rect::from_min_max(pos2(c.left(), head.top()), pos2(c.right(), head.bottom())) }
        else { Rect::from_min_max(pos2(head.left(), c.top()), pos2(head.right(), c.bottom())) }
    }
}

/// A folder's color: its label, else a quiet blue.
fn folder_color(layers: &LayerStack, id: u64) -> Color32 {
    let l = layers.node(id).map_or(0, |n| n.props().label);
    match aurastudio_layers::LABELS.get(l as usize) {
        Some((_, c)) if l > 0 => Color32::from_rgb(c[0], c[1], c[2]),
        _ => Color32::from_rgb(92, 120, 170),
    }
}

/// The panel: under the canvas (timeline) or beside the dockers (X-sheet).
pub fn panel(ctx: &egui::Context, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    let frame = egui::Frame::none().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(6.0, 4.0));
    if st.vertical {
        egui::SidePanel::right("anim_xsheet").resizable(true).show_separator_line(false).default_width(320.0)
            .width_range(220.0..=1200.0).frame(frame).show(ctx, |ui| contents(ui, st, m, a));
    } else {
        egui::TopBottomPanel::bottom("anim_timeline").resizable(true).show_separator_line(false).default_height(220.0)
            .height_range(120.0..=700.0).frame(frame).show(ctx, |ui| contents(ui, st, m, a));
    }
    ask_dialog(ctx, st, m, a);
}

fn contents(ui: &mut Ui, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    controls(ui, st, m, a);
    ui.add_space(4.0);
    grid(ui, st, m, a);
}

/// Go to frame `n` (and show it).
fn go(m: &mut Model, a: &mut Actions, n: u32) {
    let n = n.max(1);
    if m.layers.time() != n {
        m.layers.set_time(n);
        a.recomposite = true;
    }
}

/// The bar: folders, playback, timing, orientation.
fn controls(ui: &mut Ui, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    // Groups that wrap as a whole when the panel is narrow (X-sheet).
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        let last = m.layers.last_frame();
        let t = m.layers.time();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(ui, Icon::Film, "New animation folder", true) { crate::dockers::layers::new_animation_folder(m, a); }
            icon_button(ui, Icon::Camera, "New camera folder (coming)", false);
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(ui, Icon::SkipBack, "First frame", true) { a.play = Some(false); go(m, a, 1); }
            if icon_button(ui, Icon::StepBack, "Previous frame (,)", true) { a.play = Some(false); go(m, a, t.saturating_sub(1)); }
            let (icon, tip) = if m.playing { (Icon::Pause, "Pause") } else { (Icon::Play, "Play") };
            if icon_toggle(ui, icon, tip, true, m.playing).clicked() { a.play = Some(!m.playing); }
            if icon_button(ui, Icon::StepForward, "Next frame (.)", true) { a.play = Some(false); go(m, a, t + 1); }
            if icon_button(ui, Icon::SkipForward, "Last frame drawn", true) { a.play = Some(false); go(m, a, last); }
            if icon_toggle(ui, Icon::Repeat, "Loop", true, *m.looping).clicked() { *m.looping ^= true; }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(m.fps).range(1..=120).suffix(" fps").speed(0.2)).on_hover_text("Frames per second");
            ui.label(egui::RichText::new(format!("{t} / {last}")).color(theme::TEXT)).on_hover_text("Current frame / last frame drawn");
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let folder = m.layers.anim_of(m.layers.active_id()).map(|x| x.0);
            if icon_button(ui, Icon::InsertFrame, "Insert a frame here: the next ones move one later", folder.is_some()) {
                if let Some(f) = folder { if m.layers.shift_frames(f, t, Shift::Insert) { done(a, "Insert frame"); } }
            }
            if icon_button(ui, Icon::RemoveFrame, "Remove this frame: the next ones move one earlier", folder.is_some()) {
                if let Some(f) = folder { if m.layers.shift_frames(f, t, Shift::Remove) { done(a, "Remove frame"); } }
            }
            let (icon, tip) = if st.vertical { (Icon::Timeline, "Show as a timeline (under the canvas)") } else { (Icon::XSheet, "Show as an X-sheet (beside the dockers)") };
            if icon_button(ui, icon, tip, true) {
                st.vertical ^= true;
                st.scroll = Vec2::ZERO;
                st.followed = 0;
            }
        });
    });
}

/// A timing edit made: undoable, shown.
fn done(a: &mut Actions, label: &'static str) {
    a.recomposite = true;
    a.layer_edit = Some(label);
}

fn grid(ui: &mut Ui, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    let s = if st.vertical { &XSHEET } else { &TIMELINE };
    let (area, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if area.width() < 40.0 || area.height() < 30.0 { return; }
    // Columns: X-sheet left to right = bottom to top (Toonz); timeline top
    // to bottom = top to bottom (Krita, CSP).
    let mut cols = m.layers.animation_folders();
    if !st.vertical { cols.reverse(); }
    let (layer_head, frame_head, cells) = if st.vertical {
        (Rect::from_min_max(pos2(area.left() + s.frame_head, area.top()), pos2(area.right(), area.top() + s.layer_head)),
         Rect::from_min_max(pos2(area.left(), area.top() + s.layer_head), pos2(area.left() + s.frame_head, area.bottom())),
         Rect::from_min_max(pos2(area.left() + s.frame_head, area.top() + s.layer_head), area.max))
    } else {
        (Rect::from_min_max(pos2(area.left(), area.top() + s.frame_head), pos2(area.left() + s.layer_head, area.bottom())),
         Rect::from_min_max(pos2(area.left() + s.layer_head, area.top()), pos2(area.right(), area.top() + s.frame_head)),
         Rect::from_min_max(pos2(area.left() + s.layer_head, area.top() + s.frame_head), area.max))
    };

    // Scrolling: the wheel goes through time, Shift + wheel across folders.
    if resp.hovered() {
        let (d, shift) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift));
        let along = if d.y != 0.0 { -d.y } else { -d.x };
        if shift { st.scroll.y += along; } else { st.scroll.x += along; }
    }
    let span = if st.vertical { cells.height() } else { cells.width() };
    let across = if st.vertical { cells.width() } else { cells.height() };
    st.scroll.y = st.scroll.y.clamp(0.0, (cols.len() as f32 * s.layer - across).max(0.0));
    st.scroll.x = st.scroll.x.max(0.0);
    // The current frame kept in view when it changes (playback, keys).
    let t = m.layers.time();
    if t != st.followed {
        let at = (t as f32 - 1.0) * s.frame;
        if at < st.scroll.x { st.scroll.x = at; }
        if at + s.frame > st.scroll.x + span { st.scroll.x = at + s.frame - span; }
        st.followed = t;
    }
    let g = Grid { vertical: st.vertical, cells, layer: s.layer, frame: s.frame, scroll: st.scroll };
    let (f0, f1) = g.frames_shown();
    let fps = (*m.fps).max(1);

    let p = ui.painter_at(area);
    p.rect_filled(area, 2.0, theme::DESK);
    // Cells.
    let cp = p.with_clip_rect(cells);
    let active = m.layers.active_id();
    let active_folder = m.layers.anim_of(active).map(|x| x.0);
    for (c, &folder) in cols.iter().enumerate() {
        let frames = m.layers.frames(folder);
        let color = folder_color(m.layers, folder);
        let visible = m.layers.node(folder).is_some_and(|n| n.visible());
        let dim = if visible { 1.0 } else { 0.45 };
        for f in f0..=f1 {
            let r = g.cell(c, f);
            // Every 6 frames, the shade changes (the half seconds at 12).
            let bg = if f == t { theme::WIDGET_HOVER } else if ((f - 1) / 6) % 2 == 0 { theme::WIDGET } else { theme::SURFACE };
            cp.rect_filled(r.shrink(0.5), 0.0, bg);
            let drawn = frames.iter().any(|x| x.0 == f);
            let held = !drawn && frames.iter().any(|x| x.0 < f);
            if drawn {
                let id = frames.iter().find(|x| x.0 == f).map(|x| x.1);
                let on = id == Some(active);
                cp.rect_filled(r.shrink(1.5), 2.0, color.gamma_multiply(if on { 1.0 } else { 0.75 } * dim));
                if on { cp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.5_f32, Color32::WHITE)); }
                cp.text(r.center(), Align2::CENTER_CENTER, f.to_string(), FontId::proportional(if g.vertical { 11.5 } else { 10.0 }), Color32::WHITE);
            } else if held {
                // A hold: a line through the cell (a continue line).
                let (a0, a1) = if g.vertical { (pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom())) } else { (pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)) };
                cp.line_segment([a0, a1], Stroke::new(2.0_f32, color.gamma_multiply(0.8 * dim)));
            }
        }
        // The folder chosen: its column / row outlined.
        if Some(folder) == active_folder {
            let (r0, r1) = (g.cell(c, f0), g.cell(c, f1));
            cp.rect_stroke(r0.union(r1), 0.0, Stroke::new(1.0_f32, theme::ACCENT));
        }
    }
    // Second (fps) and half-second lines across the cells.
    for f in f0..=f1 + 1 {
        if (f - 1) % 6 != 0 { continue; }
        let r = g.cell(0, f);
        let strong = (f - 1) % fps == 0;
        let w = Stroke::new(if strong { 1.5_f32 } else { 1.0 }, if strong { theme::TEXT_WEAK } else { theme::BORDER });
        let line = if g.vertical { [pos2(cells.left(), r.top()), pos2(cells.right(), r.top())] } else { [pos2(r.left(), cells.top()), pos2(r.left(), cells.bottom())] };
        cp.line_segment(line, w);
    }
    // The current frame: a line across.
    let tc = g.cell(0, t);
    let now_line = if g.vertical { [pos2(cells.left(), tc.center().y), pos2(cells.right(), tc.center().y)] } else { [pos2(tc.center().x, cells.top()), pos2(tc.center().x, cells.bottom())] };
    cp.line_segment(now_line, Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.7)));
    // A frame being dragged: where it would land.
    if let Some((folder, from, to)) = st.drag {
        if let Some(c) = cols.iter().position(|x| *x == folder) {
            let copy = ui.input(|i| i.modifiers.command);
            let r = g.cell(c, to);
            cp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(2.0_f32, if copy { theme::TEXT } else { theme::ACCENT }));
            if to != from { cp.text(r.center(), Align2::CENTER_CENTER, if copy { "+" } else { "↦" }, FontId::proportional(12.0), theme::TEXT); }
        }
    }

    // Frame header: the numbers (every one; on the timeline every 5th),
    // the current frame lit.
    let hp = p.with_clip_rect(frame_head);
    hp.rect_filled(frame_head, 0.0, theme::SURFACE);
    for f in f0..=f1 {
        let r = g.frame_head(frame_head, f);
        if f == t { hp.rect_filled(r, 2.0, theme::ACCENT); }
        let label = g.vertical || f == t || f % 5 == 0 || f == 1;
        if label {
            hp.text(r.center(), Align2::CENTER_CENTER, f.to_string(), FontId::proportional(10.5), if f == t { Color32::WHITE } else { theme::TEXT_WEAK });
        }
    }
    // Layer header: each folder's name, eye and lock.
    let lp = p.with_clip_rect(layer_head);
    lp.rect_filled(layer_head, 0.0, theme::SURFACE);
    let mut head_hits = Vec::new();
    for (c, &folder) in cols.iter().enumerate() {
        let r = g.layer_head(layer_head, c).shrink(1.0);
        if !r.intersects(layer_head) { continue; }
        let Some(n) = m.layers.node(folder) else { continue };
        let on = Some(folder) == active_folder;
        lp.rect_filled(r, 2.0, if on { theme::ACCENT_DIM } else { theme::WIDGET });
        lp.rect_filled(Rect::from_min_size(r.min, if g.vertical { vec2(r.width(), 3.0) } else { vec2(3.0, r.height()) }), 1.0, folder_color(m.layers, folder));
        let (eye, lock, name) = if g.vertical {
            (Rect::from_center_size(pos2(r.left() + 12.0, r.bottom() - 12.0), vec2(20.0, 20.0)),
             Rect::from_center_size(pos2(r.right() - 12.0, r.bottom() - 12.0), vec2(20.0, 20.0)),
             Rect::from_min_max(pos2(r.left() + 4.0, r.top() + 4.0), pos2(r.right() - 4.0, r.top() + 22.0)))
        } else {
            (Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(20.0, 20.0)),
             Rect::from_center_size(pos2(r.right() - 12.0, r.center().y), vec2(20.0, 20.0)),
             Rect::from_min_max(pos2(r.left() + 28.0, r.top()), pos2(r.right() - 26.0, r.bottom())))
        };
        crate::icons::paint(ui, if n.visible() { Icon::Eye } else { Icon::EyeClosed }, eye, 14.0, if n.visible() { theme::TEXT } else { theme::TEXT_WEAK });
        crate::icons::paint(ui, if n.locked() { Icon::Lock } else { Icon::LockOpen }, lock, 13.0, if n.locked() { theme::TEXT } else { theme::TEXT_WEAK.gamma_multiply(0.6) });
        if st.rename.as_ref().is_some_and(|x| x.0 == folder) {
            let (_, buf, first) = st.rename.as_mut().unwrap();
            let edit = ui.put(name, egui::TextEdit::singleline(buf).font(FontId::proportional(12.0)));
            if *first { edit.request_focus(); *first = false; }
            if edit.lost_focus() {
                let new = buf.trim().to_owned();
                st.rename = None;
                if !new.is_empty() {
                    if let Some(n) = m.layers.node_mut(folder) { *n.name_mut() = new; }
                    a.modified = true;
                    a.layer_edit = Some("Rename");
                }
            }
        } else {
            crate::widgets::elided_text(&lp, pos2(name.left(), name.center().y + 5.0), n.name(), FontId::proportional(12.0), theme::TEXT, name.width());
        }
        head_hits.push((folder, r, eye, lock, name));
    }
    // Top-left corner.
    let corner = Rect::from_min_max(area.min, cells.min);
    p.rect_filled(corner, 0.0, theme::SURFACE);
    if cols.is_empty() {
        p.text(cells.center(), Align2::CENTER_CENTER, "No animation folder: the film button makes one", FontId::proportional(12.5), theme::TEXT_WEAK);
    }

    // --- Pointer -------------------------------------------------------
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    let in_cells = |q: Pos2| cells.contains(q);
    if resp.drag_started() {
        // (Where the button went down: the pointer may have left the cell.)
        if let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
            if in_cells(q) {
                let (c, f) = (g.col_at(q), g.frame_at(q));
                match cols.get(c) {
                    Some(&folder) if m.layers.frame_at(folder, f).is_some() => st.drag = Some((folder, f, f)),
                    _ => st.scrubbing = true,
                }
            } else if frame_head.contains(q) {
                st.scrubbing = true;
            }
        }
    }
    if resp.dragged() {
        if let Some(q) = pointer {
            let f = g.frame_at(q);
            if st.scrubbing { a.play = Some(false); go(m, a, f); }
            if let Some(d) = &mut st.drag { d.2 = f; }
            // Near the edge: scroll on.
            let edge = if g.vertical { (q.y - cells.top(), cells.bottom() - q.y) } else { (q.x - cells.left(), cells.right() - q.x) };
            if edge.0 < 12.0 { st.scroll.x = (st.scroll.x - 6.0).max(0.0); }
            if edge.1 < 12.0 { st.scroll.x += 6.0; }
        }
    }
    if resp.drag_stopped() {
        st.scrubbing = false;
        if let Some((folder, from, to)) = st.drag.take() {
            let copy = ui.input(|i| i.modifiers.command);
            if to != from {
                if m.layers.frame_at(folder, to).is_some() {
                    st.ask = Some((folder, from, to, copy));
                } else if apply_move(m, folder, from, to, copy, Onto::Refuse) {
                    done(a, if copy { "Copy frame" } else { "Move frame" });
                    go(m, a, to);
                }
            }
        }
    }
    if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if in_cells(q) {
                let (c, f) = (g.col_at(q), g.frame_at(q));
                a.play = Some(false);
                if let Some(&folder) = cols.get(c) { choose(m, folder, f); }
                go(m, a, f);
            } else if frame_head.contains(q) {
                a.play = Some(false);
                go(m, a, g.frame_at(q));
            } else if let Some((folder, _, eye, lock, _)) = head_hits.iter().find(|h| h.1.contains(q)) {
                if eye.contains(q) {
                    if let Some(n) = m.layers.node_mut(*folder) { *n.visible_mut() ^= true; }
                    a.recomposite = true;
                } else if lock.contains(q) {
                    if let Some(n) = m.layers.node_mut(*folder) { *n.locked_mut() ^= true; }
                    a.modified = true;
                    a.layer_edit = Some("Lock");
                } else {
                    choose(m, *folder, t);
                }
            }
        }
    }
    if resp.double_clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if let Some((folder, ..)) = head_hits.iter().find(|h| h.4.contains(q)) {
                st.rename = m.layers.node(*folder).map(|n| (*folder, n.name().to_owned(), true));
            }
        }
    }
    // Right click on a cell: timing edits there.
    let menu_at = resp.interact_pointer_pos().or(resp.hover_pos());
    resp.context_menu(|ui| {
        let Some(q) = menu_at else { return };
        if !in_cells(q) { ui.close_menu(); return; }
        let (c, f) = (g.col_at(q), g.frame_at(q));
        let Some(&folder) = cols.get(c) else { ui.close_menu(); return };
        let drawn = m.layers.frame_at(folder, f).is_some();
        if ui.button(format!("Insert a frame at {f}")).clicked() {
            if m.layers.shift_frames(folder, f, Shift::Insert) { done(a, "Insert frame"); }
            ui.close_menu();
        }
        if ui.button(format!("Remove frame {f}")).on_hover_text("The next frames move one earlier").clicked() {
            if m.layers.shift_frames(folder, f, Shift::Remove) { done(a, "Remove frame"); }
            ui.close_menu();
        }
        if ui.add_enabled(drawn, egui::Button::new(format!("Delete the drawing of frame {f}"))).on_hover_text("The frame before holds on").clicked() {
            if m.layers.delete_frame(folder, f) { done(a, "Delete frame"); }
            ui.close_menu();
        }
    });
    if pointer.is_some_and(in_cells) && resp.hovered() && !resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
}

/// Folder `folder` chosen at frame `f`: its frame there, else the one it
/// shows, else the folder (strokes then make a frame).
fn choose(m: &mut Model, folder: u64, f: u32) {
    let target = m.layers.frame_at(folder, f).unwrap_or(folder);
    m.layers.set_active(target);
}

fn apply_move(m: &mut Model, folder: u64, from: u32, to: u32, copy: bool, onto: Onto) -> bool {
    if copy { m.layers.copy_frame(folder, from, to, onto) } else { m.layers.move_frame(folder, from, to, onto) }
}

/// A frame moved or copied onto a drawn one: replace it, swap them
/// (moves only), or not (the default: it is usually a slip).
fn ask_dialog(ctx: &egui::Context, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    let Some((folder, from, to, copy)) = st.ask else { return };
    let name = m.layers.node(folder).map_or(String::new(), |n| n.name().to_owned());
    let mut answer = None;
    egui::Modal::new(egui::Id::new("anim_onto")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(egui::RichText::new(format!("Frame {to} of {name} is already drawn")).strong());
        ui.add_space(4.0);
        ui.label(format!("{} frame {from} onto it:", if copy { "Copy" } else { "Move" }));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Replace it").clicked() { answer = Some(Onto::Replace); }
            if !copy && ui.button("Swap them").clicked() { answer = Some(Onto::Swap); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(egui::RichText::new("Cancel").color(theme::TEXT)).fill(theme::ACCENT);
                if ui.add(b).clicked() { answer = Some(Onto::Refuse); }
            });
        });
    });
    // Enter or Escape: cancel.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) { answer = answer.or(Some(Onto::Refuse)); }
    let Some(onto) = answer else { return };
    st.ask = None;
    if onto != Onto::Refuse && apply_move(m, folder, from, to, copy, onto) {
        done(a, if copy { "Copy frame" } else { "Move frame" });
        go(m, a, to);
    }
}

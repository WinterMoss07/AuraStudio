//! The Animation panel: the X-sheet (vertical, beside the dockers) or the
//! timeline (horizontal, under the canvas): the same grid of animation
//! folders × frames, turned (OpenToonz's orientations). A drawn frame is a
//! filled cell, a hold a line until the next one. On top: new folders, the
//! playback bar, timing edits, the orientation.
//!
//! Groups (organisation only) show once; a double click opens one: the grid
//! then lists its folders alone, until "Back". Folders are picked like
//! layers (Ctrl / Shift, shared with the Layers docker), dragged to reorder,
//! into a group, or out of it (onto "Back"). Frames are picked in one folder
//! (click, Shift range, Ctrl add, a box on empty cells) and moved together.
//!
//! Everything is laid out on two axes: the frame axis (time) and the
//! layer axis (one folder per column / row); `Grid` maps them to the
//! screen either way.

use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

use aurastudio_layers::{Column, FolderKind, LayerStack, Node, Onto, Place, Shift};

use crate::dockers::layers::{delete_layer, LayersState};
use crate::icons::Icon;
use crate::widgets::{icon_button, icon_toggle};
use crate::{theme, Actions, Model, Notice};

/// Cell size along (layer axis, frame axis), and the header sizes: the
/// folders' names (layer header) and the frame numbers (frame header).
struct Sizes { layer: f32, frame: f32, layer_head: f32, frame_head: f32 }

const XSHEET: Sizes = Sizes { layer: 96.0, frame: 20.0, layer_head: 64.0, frame_head: 40.0 };
const TIMELINE: Sizes = Sizes { layer: 34.0, frame: 22.0, layer_head: 190.0, frame_head: 24.0 };

/// Least width of a context menu (it never shrinks to a letter a line).
const MENU_W: f32 = 200.0;

/// Frames dragged: the folder, the frames, where the grabbed one was and is.
struct Drag { folder: u64, frames: Vec<u32>, from: u32, to: u32 }

/// A folder (or group) header dragged, and where it would land.
struct ColDrag { id: u64, target: Option<(u64, Place)>, exit: bool }

/// A move / copy onto drawn frames, waiting for the answer.
struct Ask { folder: u64, frames: Vec<u32>, delta: i32, copy: bool }

/// What a right click was on (kept while the menu is open: the pointer is
/// on the menu then, not on the grid).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Menu { Cell(Column, u32), Head(Column) }

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
    /// The group opened (its folders alone are listed).
    group:        Option<u64>,
    /// Frames picked, all in one folder; the one a Shift range starts at.
    picked:       Vec<u32>,
    picked_in:    Option<u64>,
    anchor:       Option<u32>,
    drag:         Option<Drag>,
    col_drag:     Option<ColDrag>,
    /// A box being drawn on the cells: where the button went down.
    boxsel:       Option<Pos2>,
    /// The frame header or a cell dragged to change the current frame.
    scrubbing:    bool,
    ask:          Option<Ask>,
    /// A folder being renamed: id, text, focus asked.
    rename:       Option<(u64, String, bool)>,
    menu:         Option<Menu>,
    /// The "Back" button (a folder dropped there leaves its group).
    exit_rect:    Option<Rect>,
    /// Where the panel was drawn last (the Delete key acts here when the
    /// pointer is).
    pub(crate) rect: Option<Rect>,
    /// The Delete key was pressed over the panel: done at the next frame.
    pub(crate) delete_req: bool,
}

impl Default for AnimPanel {
    fn default() -> Self {
        Self {
            open: true, vertical: false, scroll: Vec2::ZERO, followed: 0, group: None, picked: Vec::new(), picked_in: None,
            anchor: None, drag: None, col_drag: None, boxsel: None, scrubbing: false, ask: None, rename: None, menu: None,
            exit_rect: None, rect: None, delete_req: false,
        }
    }
}

impl AnimPanel {
    /// A question is open (the canvas waits).
    pub fn is_asking(&self) -> bool { self.ask.is_some() }
}

/// A right click on `hit` opens the menu on it; it stays until another
/// right click (the pointer then leaves the grid for the menu).
fn open_menu(menu: &mut Option<Menu>, right_clicked: bool, hit: Option<Menu>) {
    if right_clicked { *menu = hit; }
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

/// Where a header dropped lands, from the screen side (before / after the
/// column under the pointer) in stack terms: the X-sheet runs bottom to
/// top, the timeline top to bottom.
fn place_for(vertical: bool, before: bool, inside: bool) -> Place {
    if inside { Place::Inside } else if before == !vertical { Place::Above } else { Place::Below }
}

/// A folder's color: its label, else a quiet blue.
fn folder_color(layers: &LayerStack, id: u64) -> Color32 {
    let l = layers.node(id).map_or(0, |n| n.props().label);
    match aurastudio_layers::LABELS.get(l as usize) {
        Some((_, c)) if l > 0 => Color32::from_rgb(c[0], c[1], c[2]),
        _ => Color32::from_rgb(92, 120, 170),
    }
}

/// The frames a column shows: a folder's own, a group's together.
fn column_frames(layers: &LayerStack, col: Column) -> Vec<(u32, Option<u64>)> {
    match col {
        Column::Folder(f) => layers.frames(f).into_iter().map(|(n, id)| (n, Some(id))).collect(),
        Column::Group(g) => {
            let mut v: Vec<(u32, Option<u64>)> = layers.group_members(g).into_iter().flat_map(|f| layers.frames(f)).map(|(n, _)| (n, None)).collect();
            v.sort_unstable();
            v.dedup_by_key(|x| x.0);
            v
        }
    }
}

/// "12 frames - 4 cels" for a folder, "3 folders" for a group.
fn info(layers: &LayerStack, col: Column) -> String {
    match col {
        Column::Folder(f) => {
            let frames = layers.frames(f).len();
            let cels = layers.anim(f).map_or(0, |a| a.template.len());
            format!("{frames} frame{} - {cels} cel{}", if frames == 1 { "" } else { "s" }, if cels == 1 { "" } else { "s" })
        }
        Column::Group(g) => {
            let n = layers.group_members(g).len();
            format!("{n} folder{}", if n == 1 { "" } else { "s" })
        }
    }
}

/// The animation folders `ls` has picked (the frames and cels chosen count
/// for their folder), once each, groups as they are.
fn picked_folders(ls: &LayersState, layers: &LayerStack) -> Vec<u64> {
    let mut v: Vec<u64> = Vec::new();
    for id in ls.selection(layers) {
        let id = layers.anim_of(id).map_or(id, |x| x.0);
        if !v.contains(&id) { v.push(id); }
    }
    v
}

/// The panel: under the canvas (timeline) or beside the dockers (X-sheet).
pub fn panel(ctx: &egui::Context, st: &mut AnimPanel, ls: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    let frame = egui::Frame::none().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(6.0, 4.0));
    if st.vertical {
        egui::SidePanel::right("anim_xsheet").resizable(true).show_separator_line(false).default_width(340.0)
            .width_range(220.0..=1200.0).frame(frame).show(ctx, |ui| contents(ui, st, ls, notice, m, a));
    } else {
        egui::TopBottomPanel::bottom("anim_timeline").resizable(true).show_separator_line(false).default_height(220.0)
            .height_range(120.0..=700.0).frame(frame).show(ctx, |ui| contents(ui, st, ls, notice, m, a));
    }
    ask_dialog(ctx, st, m, a);
}

fn contents(ui: &mut Ui, st: &mut AnimPanel, ls: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    st.rect = Some(ui.max_rect());
    // (A group deleted, undone away: back to the top.)
    if let Some(g) = st.group {
        if !matches!(m.layers.node(g), Some(Node::Folder(f)) if f.kind == FolderKind::Group) { st.group = None; }
    }
    controls(ui, st, m, a);
    ui.add_space(4.0);
    grid(ui, st, ls, notice, m, a);
}

/// Go to frame `n` (and show it).
fn go(m: &mut Model, a: &mut Actions, n: u32) {
    let n = n.max(1);
    if m.layers.time() != n {
        m.layers.set_time(n);
        a.recomposite = true;
    }
}

/// The switch between X-sheet and timeline: big, labelled, apart; pressed
/// while the X-sheet is shown.
fn switch_button(ui: &mut Ui, st: &mut AnimPanel) {
    let label = if st.vertical { "X-sheet" } else { "Timeline" };
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 28.0), Sense::click());
    let bg = if st.vertical { theme::ACCENT_DIM } else if resp.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, bg);
    p.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, if st.vertical { theme::ACCENT } else { theme::BORDER }));
    crate::icons::paint(ui, Icon::SwitchView, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(22.0, 22.0)), 17.0, theme::TEXT);
    p.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), theme::TEXT);
    let tip = if st.vertical { "X-sheet shown (beside the dockers). Click for the timeline (under the canvas)" }
        else { "Timeline shown (under the canvas). Click for the X-sheet (beside the dockers)" };
    if resp.on_hover_text(tip).clicked() {
        st.vertical ^= true;
        st.scroll = Vec2::ZERO;
        st.followed = 0;
    }
}

/// The bar: view, folders, playback, timing.
fn controls(ui: &mut Ui, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    st.exit_rect = None;
    // Groups that wrap as a whole when the panel is narrow (X-sheet).
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        let last = m.layers.last_frame();
        let t = m.layers.time();
        switch_button(ui, st);
        ui.separator();
        // Inside a group: the way out (a folder dropped on it leaves).
        if let Some(g) = st.group {
            let name = m.layers.node(g).map_or(String::new(), |n| n.name().to_owned());
            let b = ui.add(egui::Button::new(format!("‹ Back  ({name})")).min_size(vec2(0.0, 24.0)))
                .on_hover_text("Leave the group (drop a folder here to take it out)");
            st.exit_rect = Some(b.rect);
            if b.clicked() { st.group = None; st.scroll = Vec2::ZERO; }
            ui.separator();
        }
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
            ui.label(RichText::new(format!("{t} / {last}")).color(theme::TEXT)).on_hover_text("Current frame / last frame drawn");
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
        });
    });
}

/// A timing edit made: undoable, shown.
fn done(a: &mut Actions, label: &'static str) {
    a.recomposite = true;
    a.layer_edit = Some(label);
}

/// One header on screen, for the pointer.
struct Head { col: Column, rect: Rect, eye: Rect, lock: Rect, name: Rect }

fn grid(ui: &mut Ui, st: &mut AnimPanel, ls: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    let s = if st.vertical { &XSHEET } else { &TIMELINE };
    let (area, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if area.width() < 40.0 || area.height() < 30.0 { return; }
    // Columns: X-sheet left to right = bottom to top (Toonz); timeline top
    // to bottom = top to bottom (Krita, CSP).
    let mut cols = m.layers.columns(st.group);
    if !st.vertical { cols.reverse(); }
    // (Frames gone with an undo, a delete: the picked ones too.)
    if let Some(f) = st.picked_in {
        st.picked.retain(|n| m.layers.frame_at(f, *n).is_some());
        if st.picked.is_empty() { st.picked_in = None; }
    }

    // The Delete key: the picked frames, else the folders picked.
    if std::mem::take(&mut st.delete_req) {
        match st.picked_in {
            Some(f) if !st.picked.is_empty() => {
                let mut any = false;
                for n in std::mem::take(&mut st.picked) { any |= m.layers.delete_frame(f, n); }
                st.picked_in = None;
                if any { done(a, "Delete frames"); }
            }
            _ => delete_layer(ls, notice, m, a),
        }
    }

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
    let is_picked = |id: u64| ls.selected.contains(&id) || Some(id) == active_folder || id == active;
    for (c, &col) in cols.iter().enumerate() {
        let frames = column_frames(m.layers, col);
        let color = folder_color(m.layers, col.id());
        let visible = m.layers.node(col.id()).is_some_and(|n| n.visible());
        let dim = if visible { 1.0 } else { 0.45 };
        for f in f0..=f1 {
            let r = g.cell(c, f);
            // Every 6 frames, the shade changes (the half seconds at 12).
            let bg = if f == t { theme::WIDGET_HOVER } else if ((f - 1) / 6) % 2 == 0 { theme::WIDGET } else { theme::SURFACE };
            cp.rect_filled(r.shrink(0.5), 0.0, bg);
            let drawn = frames.iter().any(|x| x.0 == f);
            let held = !drawn && frames.iter().any(|x| x.0 < f);
            if drawn {
                let id = frames.iter().find(|x| x.0 == f).and_then(|x| x.1);
                let on = id.is_some() && id == Some(active);
                let lit = st.picked_in == col_folder(col) && st.picked.contains(&f);
                cp.rect_filled(r.shrink(1.5), 2.0, color.gamma_multiply(if on || lit { 1.0 } else { 0.75 } * dim));
                if on { cp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.5_f32, Color32::WHITE)); }
                if lit { cp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.5_f32, theme::ACCENT)); cp.rect_filled(r.shrink(1.5), 2.0, Color32::from_white_alpha(40)); }
                cp.text(r.center(), Align2::CENTER_CENTER, f.to_string(), FontId::proportional(if g.vertical { 11.5 } else { 10.0 }), Color32::WHITE);
            } else if held {
                // A hold: a line through the cell (a continue line).
                let (a0, a1) = if g.vertical { (pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom())) } else { (pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)) };
                cp.line_segment([a0, a1], Stroke::new(2.0_f32, color.gamma_multiply(0.8 * dim)));
            }
        }
        // The folder chosen: its column / row outlined.
        if Some(col.id()) == active_folder {
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
    // Frames being dragged: where they would land.
    if let Some(d) = &st.drag {
        if let Some(c) = cols.iter().position(|x| x.id() == d.folder) {
            let copy = ui.input(|i| i.modifiers.command);
            let delta = d.to as i64 - d.from as i64;
            for &n in &d.frames {
                let to = (n as i64 + delta).max(1) as u32;
                let r = g.cell(c, to);
                cp.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(2.0_f32, if copy { theme::TEXT } else { theme::ACCENT }));
            }
            if delta != 0 {
                let r = g.cell(c, d.to);
                cp.text(r.center(), Align2::CENTER_CENTER, if copy { "+" } else { "↦" }, FontId::proportional(12.0), theme::TEXT);
            }
        }
    }
    // A box being drawn.
    if let (Some(o), Some(q)) = (st.boxsel, resp.interact_pointer_pos()) {
        let r = Rect::from_two_pos(o, q).intersect(cells);
        cp.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.2));
        cp.rect_stroke(r, 0.0, Stroke::new(1.0_f32, theme::ACCENT));
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
    // Layer header: each column's name, what it holds, eye and lock.
    let lp = p.with_clip_rect(layer_head);
    lp.rect_filled(layer_head, 0.0, theme::SURFACE);
    let hover = ui.input(|i| i.pointer.hover_pos());
    let mut heads: Vec<Head> = Vec::new();
    for (c, &col) in cols.iter().enumerate() {
        let folder = col.id();
        let r = g.layer_head(layer_head, c).shrink(1.0);
        if !r.intersects(layer_head) { continue; }
        let Some(n) = m.layers.node(folder) else { continue };
        let on = is_picked(folder);
        lp.rect_filled(r, 2.0, if on { theme::ACCENT_DIM } else { theme::WIDGET });
        lp.rect_filled(Rect::from_min_size(r.min, if g.vertical { vec2(r.width(), 3.0) } else { vec2(3.0, r.height()) }), 1.0, folder_color(m.layers, folder));
        let (eye, lock, name, sub) = if g.vertical {
            (Rect::from_center_size(pos2(r.left() + 14.0, r.bottom() - 13.0), vec2(22.0, 22.0)),
             Rect::from_center_size(pos2(r.right() - 14.0, r.bottom() - 13.0), vec2(22.0, 22.0)),
             Rect::from_min_max(pos2(r.left() + 4.0, r.top() + 5.0), pos2(r.right() - 4.0, r.top() + 22.0)),
             pos2(r.left() + 4.0, r.top() + 31.0))
        } else {
            (Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(22.0, 22.0)),
             Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(22.0, 22.0)),
             Rect::from_min_max(pos2(r.left() + 32.0, r.top() + 2.0), pos2(r.right() - 30.0, r.center().y + 1.0)),
             pos2(r.left() + 34.0, r.center().y + 8.0))
        };
        crate::icons::paint(ui, if n.visible() { Icon::Eye } else { Icon::EyeClosed }, eye, 14.0, if n.visible() { theme::TEXT } else { theme::TEXT_WEAK });
        crate::icons::paint_lock(ui, lock, n.locked(), false, hover.is_some_and(|q| lock.contains(q)), on);
        let mut name_rect = name;
        if matches!(col, Column::Group(_)) {
            // A group: its folders icon before the name.
            let icon = Rect::from_center_size(pos2(name.left() + 7.0, name.center().y), vec2(16.0, 16.0));
            crate::icons::paint(ui, Icon::Folders, icon, 13.0, theme::TEXT);
            name_rect = Rect::from_min_max(pos2(name.left() + 18.0, name.top()), name.max);
        }
        if st.rename.as_ref().is_some_and(|x| x.0 == folder) {
            let (_, buf, first) = st.rename.as_mut().unwrap();
            let edit = ui.put(name_rect, egui::TextEdit::singleline(buf).font(FontId::proportional(12.0)));
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
            crate::widgets::elided_text(&lp, pos2(name_rect.left(), name_rect.center().y), n.name(), FontId::proportional(12.0), theme::TEXT, name_rect.width());
        }
        let sub_w = if g.vertical { r.width() - 8.0 } else { (lock.left() - sub.x - 2.0).max(10.0) };
        crate::widgets::elided_text(&lp, sub, &info(m.layers, col), FontId::proportional(10.0), theme::TEXT_WEAK, sub_w);
        heads.push(Head { col, rect: r, eye, lock, name });
    }
    // Where a header dragged would land.
    if let Some(cd) = &st.col_drag {
        if let Some((target, place)) = cd.target {
            if let Some(h) = heads.iter().find(|h| h.col.id() == target) {
                let stroke = Stroke::new(2.5_f32, theme::DROP);
                match place {
                    Place::Inside => { lp.rect_stroke(h.rect, 2.0, stroke); }
                    _ => {
                        let before = place_for(g.vertical, true, false) == place;
                        let line = match (g.vertical, before) {
                            (true, true) => [h.rect.left_top(), h.rect.left_bottom()],
                            (true, false) => [h.rect.right_top(), h.rect.right_bottom()],
                            (false, true) => [h.rect.left_top(), h.rect.right_top()],
                            (false, false) => [h.rect.left_bottom(), h.rect.right_bottom()],
                        };
                        lp.line_segment(line, stroke);
                    }
                }
            }
        }
        if cd.exit { if let Some(r) = st.exit_rect { ui.painter().rect_stroke(r, 3.0, Stroke::new(2.0_f32, theme::DROP)); } }
        let name = m.layers.node(cd.id).map_or(String::new(), |n| n.name().to_owned());
        crate::widgets::drag_ghost(ui.ctx(), &name);
    }
    // Top-left corner.
    let corner = Rect::from_min_max(area.min, cells.min);
    p.rect_filled(corner, 0.0, theme::SURFACE);
    if cols.is_empty() {
        let text = if st.group.is_some() { "Empty group" } else { "No animation folder: the film button makes one" };
        p.text(cells.center(), Align2::CENTER_CENTER, text, FontId::proportional(12.5), theme::TEXT_WEAK);
    }

    // --- Pointer -------------------------------------------------------
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    let in_cells = |q: Pos2| cells.contains(q);
    let mods = ui.input(|i| i.modifiers);
    let head_at = |q: Pos2| heads.iter().find(|h| h.rect.contains(q));
    if resp.drag_started() {
        // (Where the button went down: the pointer may have left the cell.)
        if let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
            if in_cells(q) {
                let (c, f) = (g.col_at(q), g.frame_at(q));
                match cols.get(c) {
                    Some(&Column::Folder(folder)) if m.layers.frame_at(folder, f).is_some() => {
                        // The frames dragged: those picked, if it is one of them.
                        if !(st.picked_in == Some(folder) && st.picked.contains(&f)) {
                            st.picked = vec![f];
                            st.picked_in = Some(folder);
                        }
                        st.drag = Some(Drag { folder, frames: st.picked.clone(), from: f, to: f });
                    }
                    _ => st.boxsel = Some(q),
                }
            } else if frame_head.contains(q) {
                st.scrubbing = true;
            } else if let Some(h) = head_at(q) {
                if !h.eye.contains(q) && !h.lock.contains(q) && !st.rename.as_ref().is_some_and(|x| x.0 == h.col.id()) {
                    st.col_drag = Some(ColDrag { id: h.col.id(), target: None, exit: false });
                    // A folder dragged is picked (with the others, if it is one of them).
                    if !ls.selected.contains(&h.col.id()) { ls.selected.clear(); choose(m, h.col.id(), t); }
                }
            }
        }
    }
    if resp.dragged() {
        if let Some(q) = pointer {
            let f = g.frame_at(q);
            if st.scrubbing { a.play = Some(false); go(m, a, f); }
            if let Some(d) = &mut st.drag { d.to = f; }
            // A folder dragged: the column under the pointer, before / after / inside.
            if let Some(cd) = &mut st.col_drag {
                cd.exit = st.exit_rect.is_some_and(|r| r.contains(q));
                cd.target = None;
                if let (false, Some(h)) = (cd.exit, heads.iter().find(|h| {
                    if g.vertical { h.rect.x_range().contains(q.x) } else { h.rect.y_range().contains(q.y) }
                })) {
                    let u = if g.vertical { (q.x - h.rect.left()) / h.rect.width() } else { (q.y - h.rect.top()) / h.rect.height() };
                    let inside = matches!(h.col, Column::Group(_)) && (0.25..0.75).contains(&u);
                    let place = place_for(g.vertical, u < 0.5, inside);
                    let ids = drag_ids(ls, m.layers, cd.id);
                    if h.col.id() != cd.id && ids.iter().all(|id| m.layers.can_drop(*id, h.col.id(), place)) { cd.target = Some((h.col.id(), place)); }
                }
            }
            // Near the edge: scroll on.
            if st.drag.is_some() || st.scrubbing || st.boxsel.is_some() {
                let edge = if g.vertical { (q.y - cells.top(), cells.bottom() - q.y) } else { (q.x - cells.left(), cells.right() - q.x) };
                if edge.0 < 12.0 { st.scroll.x = (st.scroll.x - 6.0).max(0.0); }
                if edge.1 < 12.0 { st.scroll.x += 6.0; }
            }
        }
    }
    if resp.drag_stopped() {
        st.scrubbing = false;
        if let Some(d) = st.drag.take() {
            let copy = mods.command;
            let delta = d.to as i32 - d.from as i32;
            if delta != 0 {
                // Landing on a drawn frame that stays: ask first.
                let clash = d.frames.iter().any(|n| {
                    let t = *n as i64 + delta as i64;
                    t >= 1 && m.layers.frame_at(d.folder, t as u32).is_some() && (copy || !d.frames.contains(&(t as u32)))
                });
                if clash {
                    st.ask = Some(Ask { folder: d.folder, frames: d.frames, delta, copy });
                } else if m.layers.move_frames(d.folder, &d.frames, delta, copy, Onto::Refuse) {
                    after_move(st, m, a, d.folder, &d.frames, delta, copy);
                }
            }
        }
        if let Some(o) = st.boxsel.take() {
            // The drawn frames of the folder the box began in, within its frames.
            if let (Some(q), Some(Column::Folder(folder))) = (resp.interact_pointer_pos(), cols.get(g.col_at(o)).copied()) {
                let (a0, a1) = (g.frame_at(o), g.frame_at(q));
                let (lo, hi) = (a0.min(a1), a0.max(a1));
                let mut hit: Vec<u32> = m.layers.frames(folder).into_iter().map(|x| x.0).filter(|n| (lo..=hi).contains(n)).collect();
                if mods.command && st.picked_in == Some(folder) { hit.extend(st.picked.iter().copied()); hit.sort_unstable(); hit.dedup(); }
                st.picked_in = (!hit.is_empty()).then_some(folder);
                st.anchor = hit.first().copied();
                st.picked = hit;
            }
        }
        if let Some(cd) = st.col_drag.take() {
            let ids = drag_ids(ls, m.layers, cd.id);
            let moved = if cd.exit {
                st.group.is_some_and(|gr| ids.iter().fold(false, |any, id| m.layers.move_node(*id, gr, Place::Above) | any))
            } else if let Some((target, place)) = cd.target {
                if ids.len() > 1 { m.layers.move_nodes(&ids, cd.id, target, place) } else { m.layers.move_node(cd.id, target, place) }
            } else { false };
            if moved {
                ls.selected.clear();
                a.recomposite = true;
                a.layer_edit = Some("Move folder");
            }
        }
    }
    if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if in_cells(q) {
                let (c, f) = (g.col_at(q), g.frame_at(q));
                a.play = Some(false);
                match cols.get(c).copied() {
                    Some(Column::Folder(folder)) => {
                        pick_frame(st, m.layers, folder, f, mods);
                        choose(m, folder, f);
                    }
                    Some(Column::Group(gr)) => {
                        st.picked.clear();
                        st.picked_in = None;
                        m.layers.set_active(gr);
                    }
                    None => { st.picked.clear(); st.picked_in = None; }
                }
                go(m, a, f);
            } else if frame_head.contains(q) {
                a.play = Some(false);
                go(m, a, g.frame_at(q));
            } else if let Some(h) = head_at(q) {
                let folder = h.col.id();
                if h.eye.contains(q) {
                    if let Some(n) = m.layers.node_mut(folder) { *n.visible_mut() ^= true; }
                    a.recomposite = true;
                } else if h.lock.contains(q) {
                    if let Some(n) = m.layers.node_mut(folder) { *n.locked_mut() ^= true; }
                    a.modified = true;
                    a.layer_edit = Some("Lock");
                } else {
                    pick_folder(ls, m, &cols, folder, t, mods);
                }
            }
        }
    }
    if resp.double_clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            if let Some(h) = head_at(q) {
                match h.col {
                    // A group opens: its folders alone.
                    Column::Group(gr) => { st.group = Some(gr); st.scroll = Vec2::ZERO; ls.selected.clear(); }
                    Column::Folder(folder) if h.name.contains(q) => {
                        st.rename = m.layers.node(folder).map(|n| (folder, n.name().to_owned(), true));
                    }
                    Column::Folder(_) => {}
                }
            }
        }
    }
    // Right click: the menu is for what was under the pointer then.
    let hit = resp.interact_pointer_pos().and_then(|q| {
        if in_cells(q) {
            let col = cols.get(g.col_at(q)).copied()?;
            Some(Menu::Cell(col, g.frame_at(q)))
        } else {
            head_at(q).map(|h| Menu::Head(h.col))
        }
    });
    if resp.secondary_clicked() {
        // A folder (or frame) right clicked joins the picks, unless it is in them.
        match hit {
            Some(Menu::Head(col)) if !ls.selection(m.layers).contains(&col.id()) => { ls.selected.clear(); choose(m, col.id(), t); }
            Some(Menu::Cell(Column::Folder(folder), f))
                if !(st.picked_in == Some(folder) && st.picked.contains(&f)) && m.layers.frame_at(folder, f).is_some() =>
            {
                st.picked = vec![f];
                st.picked_in = Some(folder);
            }
            _ => {}
        }
    }
    open_menu(&mut st.menu, resp.secondary_clicked(), hit);
    let menu = st.menu;
    resp.context_menu(|ui| {
        ui.set_min_width(MENU_W);
        match menu {
            Some(Menu::Cell(col, f)) => cell_menu(ui, st, m, a, col, f),
            Some(Menu::Head(col)) => head_menu(ui, st, ls, notice, m, a, col),
            None => ui.close_menu(),
        }
    });
    if pointer.is_some_and(in_cells) && resp.hovered() && !resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
}

/// The folders a header drag carries: those picked, if it is one of them.
fn drag_ids(ls: &LayersState, layers: &LayerStack, dragged: u64) -> Vec<u64> {
    let picked = picked_folders(ls, layers);
    if picked.len() > 1 && picked.contains(&dragged) { picked } else { vec![dragged] }
}

/// Frames moved or copied: the picks follow, the time goes to the grabbed one.
fn after_move(st: &mut AnimPanel, m: &mut Model, a: &mut Actions, folder: u64, frames: &[u32], delta: i32, copy: bool) {
    done(a, if copy { "Copy frames" } else { "Move frames" });
    st.picked = frames.iter().map(|n| (*n as i64 + delta as i64).max(1) as u32).collect();
    st.picked.sort_unstable();
    st.picked_in = Some(folder);
    let to = st.picked.first().copied().unwrap_or(1);
    let _ = to;
    if let Some(&last) = frames.last() { go(m, a, (last as i64 + delta as i64).max(1) as u32); }
}

/// A click on frame `f` of `folder`: it alone, Shift to a range from the
/// anchor, Ctrl adds / takes it off. An empty cell lets go.
fn pick_frame(st: &mut AnimPanel, layers: &LayerStack, folder: u64, f: u32, mods: egui::Modifiers) {
    let drawn = layers.frame_at(folder, f).is_some();
    if st.picked_in != Some(folder) { st.picked.clear(); st.anchor = None; }
    if !drawn {
        if !mods.command && !mods.shift { st.picked.clear(); st.picked_in = None; }
        return;
    }
    st.picked_in = Some(folder);
    if mods.shift {
        let a0 = st.anchor.unwrap_or(f);
        let (lo, hi) = (a0.min(f), a0.max(f));
        st.picked = layers.frames(folder).into_iter().map(|x| x.0).filter(|n| (lo..=hi).contains(n)).collect();
    } else if mods.command {
        if let Some(i) = st.picked.iter().position(|n| *n == f) { st.picked.remove(i); } else { st.picked.push(f); st.picked.sort_unstable(); }
        st.anchor = Some(f);
    } else {
        st.picked = vec![f];
        st.anchor = Some(f);
    }
}

/// A click on a header: the folder alone, Ctrl adds / takes it off, Shift
/// takes the range from the active folder (shared with the Layers docker).
fn pick_folder(ls: &mut LayersState, m: &mut Model, cols: &[Column], id: u64, t: u32, mods: egui::Modifiers) {
    if mods.command {
        if ls.selected.is_empty() { ls.selected = picked_folders(ls, m.layers); }
        if let Some(i) = ls.selected.iter().position(|x| *x == id) { ls.selected.remove(i); } else { ls.selected.push(id); choose(m, id, t); }
    } else if mods.shift {
        let from = m.layers.anim_of(m.layers.active_id()).map_or(m.layers.active_id(), |x| x.0);
        match (cols.iter().position(|c| c.id() == from), cols.iter().position(|c| c.id() == id)) {
            (Some(i), Some(j)) => {
                ls.selected = cols[i.min(j)..=i.max(j)].iter().map(|c| c.id()).collect();
                choose(m, id, t);
            }
            _ => { ls.selected.clear(); choose(m, id, t); }
        }
    } else {
        ls.selected.clear();
        choose(m, id, t);
    }
}

/// The folder of a column, if it is one (the frames picked belong to it).
fn col_folder(col: Column) -> Option<u64> {
    match col { Column::Folder(f) => Some(f), Column::Group(_) => None }
}

/// The menu on a cell: timing edits there, the frames picked.
fn cell_menu(ui: &mut Ui, st: &mut AnimPanel, m: &mut Model, a: &mut Actions, col: Column, f: u32) {
    let Column::Folder(folder) = col else {
        if ui.button("Open group").clicked() { st.group = Some(col.id()); st.scroll = Vec2::ZERO; ui.close_menu(); }
        return;
    };
    let drawn = m.layers.frame_at(folder, f).is_some();
    if ui.button(format!("Insert a frame at {f}")).clicked() {
        if m.layers.shift_frames(folder, f, Shift::Insert) { done(a, "Insert frame"); }
        ui.close_menu();
    }
    if ui.button(format!("Remove frame {f}")).on_hover_text("The next frames move one earlier").clicked() {
        if m.layers.shift_frames(folder, f, Shift::Remove) { done(a, "Remove frame"); }
        ui.close_menu();
    }
    let n = if st.picked_in == Some(folder) { st.picked.len() } else { 0 };
    let label = if n > 1 { format!("Delete the {n} frames picked") } else { format!("Delete the drawing of frame {f}") };
    if ui.add_enabled(drawn, egui::Button::new(label)).on_hover_text("The frame before holds on").clicked() {
        let list: Vec<u32> = if n > 1 { st.picked.clone() } else { vec![f] };
        let mut any = false;
        for x in list { any |= m.layers.delete_frame(folder, x); }
        if any { done(a, if n > 1 { "Delete frames" } else { "Delete frame" }); }
        st.picked.clear();
        st.picked_in = None;
        ui.close_menu();
    }
}

/// The menu on a header: groups, rename, delete.
fn head_menu(ui: &mut Ui, st: &mut AnimPanel, ls: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions, col: Column) {
    let id = col.id();
    let sel: Vec<u64> = picked_folders(ls, m.layers);
    let groupable: Vec<u64> = sel.iter().copied().filter(|x| m.layers.node(*x).is_some_and(Node::is_animation)).collect();
    if let Column::Group(_) = col {
        if ui.button("Open group").clicked() { st.group = Some(id); st.scroll = Vec2::ZERO; ls.selected.clear(); ui.close_menu(); }
        if ui.button("Ungroup animation folders").clicked() {
            m.layers.set_active(id);
            if m.layers.ungroup_active() { a.layer_edit = Some("Ungroup animation folders"); a.recomposite = true; }
            ls.selected.clear();
            if st.group == Some(id) { st.group = None; }
            ui.close_menu();
        }
    } else {
        if ui.add_enabled(!groupable.is_empty(), egui::Button::new("Group animation folders")).clicked() {
            if m.layers.group_animation_folders(&groupable, "Group").is_some() {
                a.layer_edit = Some("Group animation folders");
                a.recomposite = true;
            }
            ls.selected.clear();
            ui.close_menu();
        }
        if ui.add_enabled(m.layers.group_of(id).is_some(), egui::Button::new("Take out of its group")).clicked() {
            if let Some(gr) = m.layers.group_of(id) {
                if m.layers.move_node(id, gr, Place::Above) { a.layer_edit = Some("Move folder"); a.recomposite = true; }
            }
            ui.close_menu();
        }
    }
    ui.separator();
    if ui.button("Rename").clicked() {
        st.rename = m.layers.node(id).map(|n| (id, n.name().to_owned(), true));
        ui.close_menu();
    }
    if ui.button(if sel.len() > 1 { "Delete the folders picked" } else { "Delete" }).clicked() {
        delete_layer(ls, notice, m, a);
        ui.close_menu();
    }
}

/// Folder `folder` chosen at frame `f`: its frame there, else the one it
/// shows, else the folder (strokes then make a frame).
fn choose(m: &mut Model, folder: u64, f: u32) {
    let target = m.layers.frame_at(folder, f).unwrap_or(folder);
    m.layers.set_active(target);
}

/// Frames moved or copied onto drawn ones: replace them, swap them (moves
/// only), or not (the default: it is usually a slip).
fn ask_dialog(ctx: &egui::Context, st: &mut AnimPanel, m: &mut Model, a: &mut Actions) {
    let Some(Ask { folder, frames, delta, copy }) = st.ask.as_ref().map(|x| Ask { folder: x.folder, frames: x.frames.clone(), delta: x.delta, copy: x.copy }) else { return };
    let name = m.layers.node(folder).map_or(String::new(), |n| n.name().to_owned());
    let mut answer = None;
    egui::Modal::new(egui::Id::new("anim_onto")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(format!("Some frames of {name} are already drawn")).strong());
        ui.add_space(4.0);
        ui.label(format!("{} {} frame{} onto them:", if copy { "Copy" } else { "Move" }, frames.len(), if frames.len() == 1 { "" } else { "s" }));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Replace them").clicked() { answer = Some(Onto::Replace); }
            if !copy && ui.button("Swap them").clicked() { answer = Some(Onto::Swap); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new("Cancel").color(theme::TEXT)).fill(theme::ACCENT);
                if ui.add(b).clicked() { answer = Some(Onto::Refuse); }
            });
        });
    });
    // Enter or Escape: cancel.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) { answer = answer.or(Some(Onto::Refuse)); }
    let Some(onto) = answer else { return };
    st.ask = None;
    if onto != Onto::Refuse && m.layers.move_frames(folder, &frames, delta, copy, onto) {
        after_move(st, m, a, folder, &frames, delta, copy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_stays_on_what_was_right_clicked() {
        let hit = Menu::Cell(Column::Folder(7), 3);
        let mut menu = None;
        open_menu(&mut menu, true, Some(hit));
        assert_eq!(menu, Some(hit));
        // The pointer goes onto the menu (nothing under it in the grid), the
        // menu is still for that cell.
        open_menu(&mut menu, false, None);
        assert_eq!(menu, Some(hit));
        // Another right click moves it.
        open_menu(&mut menu, true, Some(Menu::Head(Column::Group(2))));
        assert_eq!(menu, Some(Menu::Head(Column::Group(2))));
    }

    #[test]
    fn a_menu_keeps_a_readable_width() {
        // Entries in a popup used to collapse to a letter a line (the menu
        // read the pointer, found nothing, and drew nothing to size it).
        let ctx = egui::Context::default();
        let mut width = 0.0;
        for k in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), time: Some(k as f64 * 0.1), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                let r = egui::Area::new(egui::Id::new("menu_test")).fixed_pos(pos2(100.0, 100.0)).show(ctx, |ui| {
                    ui.set_min_width(MENU_W);
                    let _ = ui.button("I");
                });
                width = r.response.rect.width();
            });
        }
        assert!(width >= MENU_W, "{width}");
    }

    #[test]
    fn headers_land_before_or_after_by_the_axis() {
        // X-sheet: left is the bottom of the stack; timeline: top is the top.
        assert_eq!(place_for(true, true, false), Place::Below);
        assert_eq!(place_for(true, false, false), Place::Above);
        assert_eq!(place_for(false, true, false), Place::Above);
        assert_eq!(place_for(false, false, false), Place::Below);
        assert_eq!(place_for(false, false, true), Place::Inside);
    }
}

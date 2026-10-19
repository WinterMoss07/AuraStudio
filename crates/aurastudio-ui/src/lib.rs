//! AuraStudio user interface (egui): menu and options bars, the toolbox,
//! the dock column (tabs rearrangeable, floatable), the canvas card.
//!
//! The app lends its data through `Model` each frame; the UI edits it and
//! reports what the app must do in `Actions`. `bars`: menu + options bar,
//! `dockers/`: one file per tab, `widgets`: shared widgets, `theme`.

pub mod theme;
mod bars;
mod stroke_preview;
mod dock_layout;
mod dockers;
mod filters;
mod home;
mod prefs;
mod icons;
mod tools;
mod widgets;

use std::time::{Duration, Instant};

use aurastudio_brush::Brush;
use aurastudio_brushes::Preset;
use aurastudio_layers::{FillParams, LayerStack};
use egui::Frame;
use egui_dock::{DockArea, DockState};

pub use aurastudio_canvas::BlendMode;
pub use dockers::Tab;
pub use filters::FilterAction;
pub use prefs::Prefs;
pub use home::{NewDoc, RecentFile};
pub use tools::{Group, Overlay, SelectCmd, ShapeOptions, Tool, ToolMemory};
pub use widgets::set_display_transform;

/// Brush size range, as a diameter in pixels.
pub const MAX_BRUSH_SIZE: u32 = 1000;

/// Everything the UI may edit this frame.
pub struct Model<'a> {
    /// Settings of the selected brush (the color is global).
    pub brush:              &'a mut Brush,
    /// Brush presets (Tools docker) and the selected one.
    pub presets:            &'a [Preset],
    /// Every brush medium, in order.
    pub mediums:            &'a [aurastudio_brushes::Medium],
    pub preset:             &'a mut usize,
    pub layers:             &'a mut LayerStack,
    /// Width / height typed in the Canvas dock, applied by "Resize".
    pub canvas_edit:        &'a mut (u32, u32),
    pub max_canvas:         u32,
    /// The document's history has something to undo / redo.
    pub can_undo:           bool,
    pub can_redo:           bool,
    /// Current view, for display: zoom (1.0 = 100 %) and angle in degrees.
    pub view_zoom:          f32,
    pub view_angle:         f32,
    /// The preferences (Edit > Preferences…), and the monitor profile's
    /// name (None: no profile found, shown as sRGB).
    pub prefs:              &'a mut Prefs,
    pub display_profile:    Option<&'a str>,
    /// No document open: the start screen shows in place of the canvas
    /// (the layer stack lent is an empty placeholder).
    pub home:               bool,
    /// Selected tool and the fill tool's settings.
    pub tool:               &'a mut Tool,
    /// The tool and brush last used in each toolbox group.
    pub tools:              &'a mut ToolMemory,
    pub fill:               &'a mut FillParams,
    pub shape:              &'a mut ShapeOptions,
    pub has_selection:      bool,
    /// The selection, if any (what a filter changes).
    pub selection:          Option<&'a aurastudio_layers::Selection>,
    /// Drawn over the canvas, placed with `to_points` (canvas pixel ->
    /// egui point).
    pub overlay:            Overlay<'a>,
    pub to_points:          &'a dyn Fn((f32, f32)) -> (f32, f32),
}


/// View commands from the View menu (the app also binds them to keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewCmd { ZoomIn, ZoomOut, ActualSize, Fit, RotateLeft, RotateRight, ResetRotation }

/// What the app must do after the UI ran.
#[derive(Default)]
pub struct Actions {
    /// Layer visibility / opacity / blend / order / content changed outside
    /// a stroke: recomposite the whole canvas.
    pub recomposite: bool,
    pub resize_to:   Option<(u32, u32)>,
    pub undo:        bool,
    pub redo:        bool,
    /// Clear the active layer (the app does it, undoable).
    pub clear_layer: bool,
    pub view:        Option<ViewCmd>,
    /// Open a file dialog to import MyPaint brushes.
    pub import_brushes: Option<ImportBrushes>,
    /// A brush dragged: (brush, its new medium, before which brush; None:
    /// at the end).
    pub move_brush:  Option<(usize, String, Option<usize>)>,
    /// The filter dialog: show / keep / drop a filter on the active layer.
    pub filter:      Option<FilterAction>,
    /// A medium dragged: (medium, before which medium; None: last).
    pub move_medium: Option<(String, Option<String>)>,
    /// The + after the medium icons: a new medium of that kind.
    pub add_medium:  Option<aurastudio_brushes::Kind>,
    /// (medium, new name).
    pub rename_medium: Option<(String, String)>,
    pub remove_medium: Option<String>,
    /// The document changed without needing a recomposite (lock, rename).
    pub modified:    bool,
    /// File menu / shortcut.
    pub file:        Option<FileCmd>,
    /// Answer to the dialog shown (see UiState::show_dialog).
    pub dialog:      Option<DialogChoice>,
    /// The New dialog was confirmed.
    pub new_doc:     Option<NewDoc>,
    /// A recent file was clicked.
    pub open_path:   Option<std::path::PathBuf>,
    /// "Remove from list" on a recent file.
    pub forget_recent: Option<std::path::PathBuf>,
    /// "Clear list" on the start screen.
    pub clear_recent: bool,
    /// Select all / deselect.
    pub select:      Option<SelectCmd>,
    /// Copy / paste pixels.
    pub edit:        Option<EditCmd>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileCmd { New, Open, Save, SaveAs, Export, Close }

/// Copy: the selected pixels of the active layer (all of it without a
/// selection). Paste: onto a new layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCmd { Copy, Paste }

/// A question the app asks, shown over everything until answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// Closing / replacing a document with unsaved changes.
    UnsavedChanges { name: String },
    /// An autosave from a session that didn't close normally was found.
    Recover { name: String, saved: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogChoice { Save, Discard, Cancel, Recover }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportBrushes { Files, Folder }

/// Short message shown in the options bar for a few seconds
/// ("Layer is locked"...).
#[derive(Default)]
pub(crate) struct Notice(Option<(String, Instant)>);

impl Notice {
    const DURATION: Duration = Duration::from_millis(2500);

    pub fn set(&mut self, text: &str) {
        self.0 = Some((text.to_owned(), Instant::now()));
    }

    /// Current text, if still showing, and how long it has left.
    pub fn current(&mut self) -> Option<(&str, Duration)> {
        let left = Self::DURATION.saturating_sub(self.0.as_ref()?.1.elapsed());
        if left.is_zero() {
            self.0 = None;
            return None;
        }
        self.0.as_ref().map(|(t, _)| (t.as_str(), left))
    }
}

/// UI state kept between frames.
pub struct UiState {
    dock:          DockState<Tab>,
    theme_applied: bool,
    focus_tab:     Option<Tab>,
    notice:        Notice,
    color:         dockers::color::ColorState,
    layers:        dockers::layers::LayersState,
    tool_list:     dockers::tool_list::ToolListState,
    /// The open filter dialog.
    filter:        Option<filters::FilterDialog>,
    /// The Preferences dialog is open, on this section.
    prefs_open:    Option<prefs::Section>,
    dock_layout:   dock_layout::DockLayout,
    dialog:        Option<Dialog>,
    new_form:      Option<home::NewForm>,
    /// The New dialog's tab last used (Painting, Animation, Comics).
    new_category:  usize,
    recent:        Vec<RecentFile>,
}

impl Default for UiState {
    fn default() -> Self { Self::new() }
}

impl UiState {
    pub fn new() -> Self {
        Self {
            dock:          dockers::default_dock(),
            theme_applied: false,
            focus_tab:     None,
            notice:        Notice::default(),
            color:         Default::default(),
            layers:        Default::default(),
            tool_list:     Default::default(),
            filter:        None,
            prefs_open:    None,
            dock_layout:   Default::default(),
            dialog:        None,
            new_form:      None,
            new_category:  0,
            recent:        Vec::new(),
        }
    }

    /// Show the New document dialog.
    pub fn open_new_dialog(&mut self) {
        self.new_form = Some(home::NewForm::new(self.new_category));
    }

    /// Recent files of the start screen (thumbnails come later, see
    /// set_recent_thumbnail).
    pub fn set_recent(&mut self, recent: Vec<RecentFile>) {
        self.recent = recent;
    }

    pub fn set_recent_thumbnail(&mut self, path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
        home::set_thumbnail(&mut self.recent, path, w, h, rgba);
    }

    /// Ask the user; the answer comes back in `Actions::dialog`.
    pub fn show_dialog(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
    }

    /// A dialog is waiting for an answer (the canvas ignores the pen).
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some() || self.new_form.is_some() || self.prefs_open.is_some()
    }

    /// The pixels of this layer changed: rebuild its thumbnail.
    pub fn invalidate_thumbnail(&mut self, layer_id: u64) {
        self.layers.invalidate(layer_id);
    }

    /// Every layer changed (canvas resize...).
    pub fn invalidate_all_thumbnails(&mut self) {
        self.layers.invalidate_all();
    }

    /// Edit > Preferences… (Ctrl+K).
    pub fn open_prefs(&mut self) {
        if self.prefs_open.is_none() { self.prefs_open = Some(prefs::Section::default()); }
    }

    /// Open the dialog of filter `n` (any number: taken modulo their count)
    /// on the active layer, as its menu entry does. For the fuzz test.
    #[doc(hidden)]
    pub fn open_filter_dialog(&mut self, ctx: &egui::Context, n: usize, m: &Model) {
        let kinds: Vec<aurastudio_filters::Kind> = aurastudio_filters::GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
        if self.filter.is_none() { self.filter = filters::new_dialog(ctx, kinds[n % kinds.len()], m); }
    }

    /// A filter dialog is open (the layer shows its preview: no painting).
    pub fn filtering(&self) -> bool { self.filter.is_some() }

    /// Close the filter dialog without keeping its filter (the app sends
    /// the Cancel itself: a document is closing...).
    pub fn close_filter(&mut self) -> bool { self.filter.take().is_some() }

    /// A medium was just made: show it and edit its name.
    pub fn rename_medium(&mut self, name: &str) {
        self.tool_list.rename(name);
    }

    /// Show a short message in the tool options bar for a few seconds.
    pub fn notify(&mut self, text: &str) {
        self.notice.set(text);
    }
}

/// Build the whole UI for this frame.
pub fn show(ctx: &egui::Context, state: &mut UiState, model: &mut Model, actions: &mut Actions) {
    if !state.theme_applied {
        theme::apply(ctx);
        state.theme_applied = true;
    }
    state.layers.refresh_thumbnails(ctx, model.layers);

    bars::menu_bar(ctx, state, model, actions);
    bars::toolbar(ctx, &mut state.notice, model, actions);

    if let Some(tab) = state.focus_tab.take() {
        if let Some(loc) = state.dock.find_tab(&tab) {
            state.dock.set_active_tab(loc);
        }
    }

    egui::SidePanel::right("dock")
        .resizable(true)
        .show_separator_line(false)
        .default_width(300.0)
        .width_range(240.0..=600.0)
        // A little room on the left: the tabs no longer touch the canvas.
        .frame(Frame::none().fill(theme::PANEL).inner_margin(egui::Margin { left: 6.0, ..Default::default() }))
        .show(ctx, |ui| {
            // No document open: grayed out, like the toolbox.
            if model.home { ui.disable(); }
            let style = theme::dock_style(ui.style());
            let mut viewer = dockers::Viewer {
                color:  &mut state.color,
                layers: &mut state.layers,
                tool_list: &mut state.tool_list,
                notice: &mut state.notice,
                m:      model,
                a:      actions,
                content: &mut state.dock_layout.content,
            };
            let (bar, sep) = (style.tab_bar.height, style.separator.width);
            DockArea::new(&mut state.dock)
                .style(style)
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .window_bounds(ctx.screen_rect())
                .show_inside(ui, &mut viewer);
            // Docks keep their content visible: a separator pushes the
            // next docks instead of squashing one.
            state.dock_layout.enforce(&mut state.dock, bar, sep);
        });

    if let Some(card) = canvas_card(ctx, model) {
        if model.home {
            home::start_screen(ctx, card, &mut state.recent, actions);
        } else {
            tools::draw_overlay(ctx, card, &model.overlay, model.to_points);
        }
    }
    if let Some(section) = &mut state.prefs_open {
        let mut reset = false;
        if !prefs::dialog(ctx, section, model.prefs, model.display_profile, &mut reset) { state.prefs_open = None; }
        if reset { state.dock = dockers::default_dock(); }
    }
    if let Some(d) = &mut state.filter {
        if !filters::dialog(ctx, d, actions) { state.filter = None; }
    }
    if let Some(form) = &mut state.new_form {
        state.new_category = form.category();
        if !home::new_dialog(ctx, form, model.max_canvas, actions) { state.new_form = None; }
    }
    dialog(ctx, state, actions);
}

/// The open dialog, if any: modal, the rest of the UI is dimmed.
fn dialog(ctx: &egui::Context, state: &mut UiState, a: &mut Actions) {
    use egui::RichText;
    let Some(d) = state.dialog.clone() else { return };
    let mut choice = None;
    let button = |ui: &mut egui::Ui, text: &str, primary: bool| {
        let b = egui::Button::new(RichText::new(text).color(theme::TEXT)).min_size(egui::vec2(96.0, 28.0));
        ui.add(if primary { b.fill(theme::ACCENT) } else { b })
    };
    let modal = egui::Modal::new(egui::Id::new("app_dialog"))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(18.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            match &d {
                Dialog::UnsavedChanges { name } => {
                    ui.label(RichText::new(format!("Save changes to “{name}”?")).heading());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Your changes will be lost if you don't save them.").color(theme::TEXT_WEAK));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if button(ui, "Don't save", false).clicked() { choice = Some(DialogChoice::Discard); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if button(ui, "Save", true).clicked() { choice = Some(DialogChoice::Save); }
                            if button(ui, "Cancel", false).clicked() { choice = Some(DialogChoice::Cancel); }
                        });
                    });
                }
                Dialog::Recover { name, saved } => {
                    ui.label(RichText::new("Recover unsaved work?").heading());
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!(
                        "AuraStudio didn't close normally. An automatic save of “{name}” from {saved} was found."
                    )).color(theme::TEXT_WEAK));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if button(ui, "Discard", false).clicked() { choice = Some(DialogChoice::Discard); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if button(ui, "Recover", true).clicked() { choice = Some(DialogChoice::Recover); }
                        });
                    });
                }
            }
        });
    // Escape / click outside: cancel (the recovery question stays).
    if modal.should_close() && choice.is_none() && matches!(d, Dialog::UnsavedChanges { .. }) {
        choice = Some(DialogChoice::Cancel);
    }
    if choice.is_some() {
        state.dialog = None;
        a.dialog = choice;
    }
}

/// egui memory key of the canvas rect (points), read by the GPU crate to
/// place the canvas viewport.
pub const CANVAS_RECT_ID: &str = "aurastudio_canvas_rect";

/// The canvas card: what is left of the window, rounded (the UI surface is
/// painted over its corners; the GPU draws the canvas under the UI).
fn canvas_card(ctx: &egui::Context, model: &mut Model) -> Option<egui::Rect> {
    use egui::{pos2, Rect, Shape};
    let free = ctx.available_rect();
    let g = theme::CANVAS_FRAME;
    // From the window's left edge (the toolbox sits on it).
    let card = Rect::from_min_max(free.min, pos2(free.right(), free.bottom() - g));
    if card.width() <= 0.0 || card.height() <= 0.0 { return None; }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(CANVAS_RECT_ID), card));
    let tools = tools::toolbox(ctx, card.left_center(), model);

    let painter = ctx.layer_painter(egui::LayerId::background());
    let fill = theme::PANEL;
    painter.rect_filled(Rect::from_min_max(pos2(card.left(), card.bottom()), free.max), 0.0, fill);
    // Each rounded corner (the toolbox's too): a fan of triangles between
    // the corner and a quarter circle.
    let r = theme::CANVAS_ROUNDING;
    for (corner, sx, sy) in [
        (card.left_top(), 1.0, 1.0),
        (pos2(card.left(), tools.top()), 1.0, -1.0),
        (pos2(card.left(), tools.bottom()), 1.0, 1.0),
        (card.right_top(), -1.0, 1.0),
        (card.left_bottom(), 1.0, -1.0),
        (card.right_bottom(), -1.0, -1.0),
    ] {
        let center = corner + egui::vec2(sx * r, sy * r);
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(corner, fill);
        const STEPS: u32 = 8;
        for i in 0..=STEPS {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / STEPS as f32;
            mesh.colored_vertex(center - egui::vec2(sx * r * a.cos(), sy * r * a.sin()), fill);
            if i > 0 { mesh.add_triangle(0, i, i + 1); }
        }
        painter.add(Shape::mesh(mesh));
    }
    Some(card)
}

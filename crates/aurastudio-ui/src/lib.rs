//! AuraStudio user interface (egui): menu bar, Photoshop-like tool options
//! bar under it, CSP-like dock column on the right (tabs can be rearranged,
//! resized and dragged out as floating windows), canvas in the middle.
//!
//! The app owns the data (brush, layers...) and lends it through `Model`
//! each frame; the UI edits it directly and reports anything the app must
//! act on (recomposite, canvas resize) in `Actions`.
//!
//! Layout of this crate: `bars` = menu bar + options bar, `dockers/` = one
//! file per dock tab, `widgets` = shared custom widgets, `theme` = colors.
//! Data crates (layers, brush, canvas) know nothing about the UI.

pub mod theme;
mod bars;
mod dock_layout;
mod dockers;
mod widgets;

use std::time::{Duration, Instant};

use aurastudio_brush::Brush;
use aurastudio_brushes::Preset;
use aurastudio_layers::LayerStack;
use egui::Frame;
use egui_dock::{DockArea, DockState};

pub use aurastudio_canvas::BlendMode;
pub use dockers::Tab;
pub use widgets::set_display_transform;

/// Brush size range, as a diameter in pixels.
pub const MAX_BRUSH_SIZE: u32 = 1000;

/// Everything the UI may edit this frame.
pub struct Model<'a> {
    /// Settings of the selected brush (the color is global).
    pub brush:              &'a mut Brush,
    /// Brush presets (Brushes docker) and the selected one.
    pub presets:            &'a [Preset],
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
    /// Display color management (View menu): on / off, intent, and the
    /// monitor profile name (None: no profile found, shown as sRGB).
    pub color_managed:      &'a mut bool,
    pub perceptual:         &'a mut bool,
    pub display_profile:    Option<&'a str>,
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
}

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
    brushes:       dockers::brushes::BrushesState,
    dock_layout:   dock_layout::DockLayout,
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
            brushes:       Default::default(),
            dock_layout:   Default::default(),
        }
    }

    /// The pixels of this layer changed: rebuild its thumbnail.
    pub fn invalidate_thumbnail(&mut self, layer_id: u64) {
        self.layers.invalidate(layer_id);
    }

    /// Every layer changed (canvas resize...).
    pub fn invalidate_all_thumbnails(&mut self) {
        self.layers.invalidate_all();
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
    bars::options_bar(ctx, &mut state.notice, model);

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
        .frame(Frame::none().fill(theme::BACKDROP).inner_margin(egui::Margin {
            left: 0.0, right: theme::GAP, top: 0.0, bottom: theme::GAP,
        }))
        .show(ctx, |ui| {
            let style = theme::dock_style(ui.style());
            let mut viewer = dockers::Viewer {
                color:  &mut state.color,
                layers: &mut state.layers,
                brushes: &mut state.brushes,
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

    canvas_card(ctx);
}

/// egui memory key of the canvas rect (points), read by the GPU crate to
/// place the canvas viewport.
pub const CANVAS_RECT_ID: &str = "aurastudio_canvas_rect";

/// The canvas is a rounded card like the docks: what is left of the
/// window, inset by the gap, with the backdrop painted around it and over
/// its corners (the GPU draws the canvas below the whole UI).
fn canvas_card(ctx: &egui::Context) {
    use egui::{pos2, Rect, Shape};
    let free = ctx.available_rect();
    let g = theme::GAP;
    let card = Rect::from_min_max(pos2(free.left() + g, free.top()), pos2(free.right() - g, free.bottom() - g));
    if card.width() <= 0.0 || card.height() <= 0.0 { return; }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(CANVAS_RECT_ID), card));

    let painter = ctx.layer_painter(egui::LayerId::background());
    let fill = theme::BACKDROP;
    for strip in [
        Rect::from_min_max(free.min, pos2(card.left(), free.bottom())),
        Rect::from_min_max(pos2(card.right(), free.top()), free.max),
        Rect::from_min_max(pos2(card.left(), card.bottom()), pos2(card.right(), free.bottom())),
    ] {
        painter.rect_filled(strip, 0.0, fill);
    }
    // Each corner: the area between the corner and a quarter circle, as a
    // fan of triangles from the corner.
    let r = theme::CARD_ROUNDING;
    for (corner, sx, sy) in [
        (card.left_top(), 1.0, 1.0),
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
}

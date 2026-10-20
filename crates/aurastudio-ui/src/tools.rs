//! Toolbox (groups of tools, each remembering its last tool and brush),
//! tool options, and what tools draw over the canvas.

use std::collections::HashMap;

use aurastudio_brushes::{Kind, Preset};
use egui::{pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui};

use crate::widgets::{fill_slider, Fill};
use crate::{theme, Actions, Model};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tool {
    #[default]
    Brush,
    Eraser,
    Picker,
    Fill,
    Lasso,
    RectSelect,
    EllipseSelect,
    PolySelect,
    MagicWand,
    Transform,
    Line,
    Rectangle,
    Ellipse,
    Arc,
}

impl Tool {
    /// Geometry tools: they draw a shape with the current brush. U cycles
    /// through them (Photoshop).
    pub const SHAPES: [Tool; 4] = [Tool::Line, Tool::Rectangle, Tool::Ellipse, Tool::Arc];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Brush      => "Brush",
            Tool::Eraser     => "Eraser",
            Tool::Picker     => "Color picker",
            Tool::Fill       => "Fill",
            Tool::Lasso      => "Lasso selection",
            Tool::RectSelect => "Rectangle selection",
            Tool::EllipseSelect => "Ellipse selection",
            Tool::PolySelect => "Polygon selection",
            Tool::MagicWand  => "Magic wand",
            Tool::Transform  => "Transform",
            Tool::Line       => "Line",
            Tool::Rectangle  => "Rectangle",
            Tool::Ellipse    => "Ellipse",
            Tool::Arc        => "Arc",
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Tool::Brush      => Icon::Brush,
            Tool::Eraser     => Icon::Eraser,
            Tool::Picker     => Icon::Picker,
            Tool::Fill       => Icon::Fill,
            Tool::Lasso      => Icon::Lasso,
            Tool::RectSelect => Icon::RectSelect,
            Tool::EllipseSelect => Icon::EllipseSelect,
            Tool::PolySelect => Icon::PolySelect,
            Tool::MagicWand  => Icon::Wand,
            Tool::Transform  => Icon::Transform,
            Tool::Line       => Icon::Line,
            Tool::Rectangle  => Icon::Rectangle,
            Tool::Ellipse    => Icon::Ellipse,
            Tool::Arc        => Icon::Arc,
        }
    }

    /// A few words shown on hover.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Brush      => "Paint",
            Tool::Eraser     => "Erase",
            Tool::Picker     => "Click: take the color",
            Tool::Fill       => "Click: fill an area",
            Tool::Lasso      => "Drag. Shift: add, Alt: subtract",
            Tool::RectSelect => "Drag. Shift: add, Alt: subtract",
            Tool::EllipseSelect => "Drag. Shift: add, Alt: subtract",
            Tool::PolySelect => "Click points. Enter: done",
            Tool::MagicWand  => "Click. Shift: add, Alt: subtract",
            Tool::Transform  => "Enter: apply, Esc: cancel",
            Tool::Line       => "Drag. Shift: 15° steps",
            Tool::Rectangle  => "Drag. Shift: square, Alt: centered",
            Tool::Ellipse    => "Drag. Shift: circle, Alt: centered",
            Tool::Arc        => "Start, end, bend. Enter: done",
        }
    }

    /// The stroke preview's shape for this tool.
    pub fn stroke_shape(self) -> aurastudio_brushes::stroke_preview::Shape {
        use aurastudio_brushes::stroke_preview::Shape;
        match self {
            Tool::Line => Shape::Line,
            Tool::Rectangle => Shape::Rectangle,
            Tool::Ellipse => Shape::Ellipse,
            Tool::Arc => Shape::Arc,
            _ => Shape::S,
        }
    }

    /// Paints with the brush (size, opacity... apply; outline cursor).
    pub fn paints(self) -> bool { matches!(self, Tool::Brush | Tool::Eraser) || self.is_shape() }

    pub fn is_shape(self) -> bool { Tool::SHAPES.contains(&self) }

    /// Makes a selection.
    pub fn selects(self) -> bool { matches!(self, Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect | Tool::PolySelect | Tool::MagicWand) }
}

/// A toolbox entry: a set of tools listed in the Tools docker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group { Draw, Paint, Eraser, Fill, Picker, Geometry, Selection, Transform }

impl Group {
    /// Toolbox order. No hand or zoom tool: Space / Ctrl+Space drags and
    /// the wheel do that from any tool.
    pub const ALL: [Group; 8] = [Group::Draw, Group::Paint, Group::Eraser, Group::Fill, Group::Picker, Group::Geometry, Group::Selection, Group::Transform];

    pub fn name(self) -> &'static str {
        match self {
            Group::Draw => "Draw",
            Group::Paint => "Paint",
            Group::Eraser => "Eraser",
            Group::Fill => "Fill",
            Group::Picker => "Color picker",
            Group::Geometry => "Geometry",
            Group::Selection => "Selection",
            Group::Transform => "Transform",
        }
    }

    /// Shortcut key; pressed again, it goes to the group's next tool.
    pub fn key(self) -> &'static str {
        match self {
            Group::Draw => "B",
            Group::Paint => "P",
            Group::Eraser => "E",
            Group::Fill => "G",
            Group::Picker => "I",
            Group::Geometry => "U",
            Group::Selection => "M",
            Group::Transform => "T",
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Group::Draw => Icon::Pencil,
            Group::Paint => Icon::Paint,
            Group::Eraser => Icon::Eraser,
            Group::Fill => Icon::Fill,
            Group::Picker => Icon::Picker,
            Group::Geometry => Icon::Shapes,
            Group::Selection => Icon::RectSelect,
            Group::Transform => Icon::Transform,
        }
    }

    /// The tools listed in the Tools docker (the brush groups list their
    /// brushes instead).
    pub fn tools(self) -> &'static [Tool] {
        match self {
            Group::Draw | Group::Paint => &[Tool::Brush],
            Group::Eraser => &[Tool::Eraser],
            Group::Fill => &[Tool::Fill],
            Group::Picker => &[Tool::Picker],
            Group::Geometry => &Tool::SHAPES,
            Group::Selection => &[Tool::RectSelect, Tool::EllipseSelect, Tool::Lasso, Tool::PolySelect, Tool::MagicWand],
            Group::Transform => &[Tool::Transform],
        }
    }

    /// The kind of brush mediums the group lists (None: not a brush group).
    pub fn kind(self) -> Option<Kind> {
        match self {
            Group::Draw => Some(Kind::Draw),
            Group::Paint => Some(Kind::Paint),
            Group::Eraser => Some(Kind::Eraser),
            _ => None,
        }
    }

    pub fn is_brushes(self) -> bool { self.kind().is_some() }

    /// The group of a kind of brushes.
    pub fn of_kind(kind: Kind) -> Group {
        match kind { Kind::Draw => Group::Draw, Kind::Paint => Group::Paint, Kind::Eraser => Group::Eraser }
    }

    /// The group the current tool (and brush) belongs to.
    pub fn current(tool: Tool, preset: usize, presets: &[Preset]) -> Group {
        match tool {
            Tool::Brush => presets.get(preset).map_or(Group::Draw, |p| match Group::of_kind(p.kind) {
                // An eraser brush with the Brush tool: still a brush.
                Group::Eraser => Group::Draw,
                g => g,
            }),
            Tool::Eraser => Group::Eraser,
            Tool::Fill => Group::Fill,
            Tool::Picker => Group::Picker,
            Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc => Group::Geometry,
            Tool::RectSelect | Tool::EllipseSelect | Tool::Lasso | Tool::PolySelect | Tool::MagicWand => Group::Selection,
            Tool::Transform => Group::Transform,
        }
    }

    pub fn from_key(c: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.key().eq_ignore_ascii_case(c))
    }
}

/// The tool and brush last used in each group.
#[derive(Default)]
pub struct ToolMemory {
    tool:   HashMap<Group, Tool>,
    preset: HashMap<Group, usize>,
}

impl ToolMemory {
    /// Note what is in use now (called after every change).
    pub fn remember(&mut self, tool: Tool, preset: usize, presets: &[Preset]) {
        let g = Group::current(tool, preset, presets);
        self.tool.insert(g, tool);
        if g.is_brushes() { self.preset.insert(g, preset); }
    }

    /// Switch to group `g`: its last tool and brush. The same group again
    /// goes to its next tool (U: line, rectangle, ellipse, arc...).
    pub fn select(&mut self, g: Group, tool: &mut Tool, preset: &mut usize, presets: &[Preset]) {
        self.remember(*tool, *preset, presets);
        let tools = g.tools();
        if Group::current(*tool, *preset, presets) == g {
            if let Some(i) = tools.iter().position(|t| t == tool) { *tool = tools[(i + 1) % tools.len()]; }
            return;
        }
        *tool = self.tool.get(&g).copied().unwrap_or(tools[0]);
        if g.is_brushes() {
            let first = || presets.iter().position(|p| Some(p.kind) == g.kind());
            if let Some(p) = self.preset.get(&g).copied().or_else(first) { *preset = p; }
        }
    }
}

/// Settings of the geometry tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOptions {
    /// Arc: each arc starts where the last one ended (one outline).
    pub multipoint: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self { Self { multipoint: true } }
}

/// Width of the toolbox.
pub const TOOLBOX_WIDTH: f32 = 40.0;

/// The toolbox: a small block of the UI surface on the left edge of the
/// canvas, centered, only as tall as its tools (Krita's optimized layout);
/// the canvas goes on to the window edge above and below it. `at`: middle
/// of the canvas's left edge. Returns its rect.
pub fn toolbox(ctx: &egui::Context, at: Pos2, m: &mut Model) -> Rect {
    let r = theme::CANVAS_ROUNDING;
    let id = egui::Id::new("toolbox");
    // Above the start screen, which covers the whole canvas area.
    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, id));
    egui::Area::new(id)
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(at)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(theme::PANEL)
                .rounding(egui::Rounding { ne: r, se: r, ..Default::default() })
                .inner_margin(egui::Margin::symmetric(4.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width(TOOLBOX_WIDTH - 8.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let current = Group::current(*m.tool, *m.preset, m.presets);
                    for g in Group::ALL {
                        if matches!(g, Group::Fill | Group::Geometry | Group::Selection) {
                            ui.add_space(3.0);
                            let (r, _) = ui.allocate_exact_size(vec2(TOOLBOX_WIDTH - 8.0, 1.0), Sense::hover());
                            ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), 0.0, theme::BORDER);
                            ui.add_space(3.0);
                        }
                        if tool_button(ui, g, current == g, !m.home) && current != g {
                            m.tools.select(g, m.tool, m.preset, m.presets);
                        }
                    }
                });
        })
        .response
        .rect
}

fn tool_button(ui: &mut Ui, group: Group, selected: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(TOOLBOX_WIDTH - 8.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(format!("{} ({})", group.name(), group.key()));
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 3.0, theme::ACCENT_DIM);
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 3.0, theme::WIDGET_HOVER);
    }
    let col = match (enabled, selected) {
        (false, _) => theme::TEXT_WEAK.gamma_multiply(0.45),
        (true, true) => theme::TEXT,
        (true, false) => Color32::from_gray(0xc8),
    };
    crate::icons::paint(ui, group.icon(), rect, 20.0, col);
    enabled && resp.clicked()
}


/// Options of the tools that are not brushes (Tool options docker).
pub fn options(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    ui.label(RichText::new(m.tool.name()).strong()).on_hover_text(m.tool.hint());
    ui.add_space(4.0);
    match *m.tool {
        Tool::Fill => fill_options(ui, m, ui.available_width()),
        Tool::Picker => { ui.checkbox(&mut m.picker.loupe, "Loupe").on_hover_text(LOUPE_TIP); }
        t if t.selects() => selection_options(ui, m, a),
        Tool::Transform => transform_options(ui, m, a),
        _ => {}
    }
}

/// Tolerance, reference and expand of the fill tool.
pub fn fill_options(ui: &mut Ui, m: &mut Model, w: f32) {
    let tol = m.fill.tolerance as f32 / 255.0 * 100.0;
    if let Some(v) = fill_slider(ui, "fill_tolerance", crate::widgets::percent("Tolerance"), tol, w) {
        m.fill.tolerance = (v / 100.0 * 255.0).round() as u8;
    }
    let expand = Fill { label: "Expand", range: 0.0..=8.0, mid: None, format: &|v| format!("{v:.0} px") };
    if let Some(v) = fill_slider(ui, "fill_expand", expand, m.fill.expand as f32, w) {
        m.fill.expand = v as u32;
    }
    ui.add_space(2.0);
    ui.label(RichText::new("Refer to").small().color(theme::TEXT_WEAK));
    ui.horizontal_wrapped(|ui| {
        use aurastudio_layers::Refer;
        ui.selectable_value(&mut m.fill.refer, Refer::Layer, "This layer");
        ui.selectable_value(&mut m.fill.refer, Refer::All, "All layers");
        ui.selectable_value(&mut m.fill.refer, Refer::References, "References");
    });
    let active = m.layers.active_id();
    if m.fill.refer == aurastudio_layers::Refer::References && m.layers.reference_for(active).is_none() {
        ui.label(RichText::new("No reference layer").small().color(theme::DROP));
    }
    ui.add_enabled_ui(m.fill.refer != aurastudio_layers::Refer::Layer, |ui| {
        ui.checkbox(&mut m.fill.skip_drafts, "Ignore draft layers");
    });
}

/// Brushes: "stay inside the lines" (CSP's anti-overflow).
pub fn stay_inside_option(ui: &mut Ui, m: &mut Model) {
    ui.add_space(2.0);
    ui.checkbox(m.stay_inside, "Stay inside the lines");
}

/// Select menu commands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectCmd { All, None, Reselect, Invert, Grow(f32), Shrink(f32), Feather(f32), Border(f32), FromLayer }

/// The magic wand's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WandOptions {
    /// Largest difference per channel still "the same color".
    pub tolerance:   u8,
    pub refer:       aurastudio_layers::Refer,
    pub skip_drafts: bool,
    /// Every like pixel touching the one clicked (off: wherever they are).
    pub contiguous:  bool,
    /// Grown by this many pixels.
    pub grow:        u32,
}

impl Default for WandOptions {
    fn default() -> Self {
        Self { tolerance: 32, refer: aurastudio_layers::Refer::Layer, skip_drafts: true, contiguous: true, grow: 0 }
    }
}

/// Settings of the selection tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectOptions {
    pub mode:    aurastudio_layers::SelectMode,
    /// Soft edges one pixel wide (ellipse, lasso, polygon, wand).
    pub aa:      bool,
    /// Soft edges this wide (px).
    pub feather: f32,
    pub wand:    WandOptions,
    /// A magnified view around the cursor; points land on pixel corners.
    pub loupe:   bool,
}

impl Default for SelectOptions {
    fn default() -> Self { Self { mode: Default::default(), aa: true, feather: 0.0, wand: WandOptions::default(), loupe: false } }
}

/// The color picker's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PickerOptions {
    /// A magnified view around the cursor, to take one exact pixel.
    pub loupe: bool,
}

const LOUPE_TIP: &str = "A magnified view around the cursor, to aim at one pixel";

/// The selection tools' options: mode, edges, the wand's settings.
pub fn selection_options(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    use aurastudio_layers::SelectMode as S;
    use crate::icons::Icon;
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (mode, icon, tip) in [
            (S::Replace, Icon::SelReplace, "New selection"),
            (S::Add, Icon::SelAdd, "Add (Shift)"),
            (S::Subtract, Icon::SelSubtract, "Subtract (Alt)"),
            (S::Intersect, Icon::SelIntersect, "Intersect (Shift+Alt)"),
        ] {
            if crate::widgets::icon_toggle(ui, icon, tip, true, m.select.mode == mode).clicked() { m.select.mode = mode; }
        }
    });
    ui.add_space(2.0);
    if *m.tool != Tool::RectSelect { ui.checkbox(&mut m.select.aa, "Anti-aliasing"); }
    ui.checkbox(&mut m.select.loupe, "Loupe").on_hover_text(LOUPE_TIP);
    let feather = Fill { label: "Feather", range: 0.0..=200.0, mid: Some(20.0), format: &|v| format!("{v:.0} px") };
    if let Some(v) = fill_slider(ui, "sel_feather", feather, m.select.feather, w) { m.select.feather = v; }
    if *m.tool == Tool::MagicWand {
        ui.add_space(4.0);
        let wd = &mut m.select.wand;
        let tol = wd.tolerance as f32 / 255.0 * 100.0;
        if let Some(v) = fill_slider(ui, "wand_tolerance", crate::widgets::percent("Tolerance"), tol, w) {
            wd.tolerance = (v / 100.0 * 255.0).round() as u8;
        }
        let grow = Fill { label: "Grow", range: 0.0..=20.0, mid: None, format: &|v| format!("{v:.0} px") };
        if let Some(v) = fill_slider(ui, "wand_grow", grow, wd.grow as f32, w) { wd.grow = v as u32; }
        ui.checkbox(&mut wd.contiguous, "Contiguous");
        ui.add_space(2.0);
        ui.label(RichText::new("Refer to").small().color(theme::TEXT_WEAK));
        ui.horizontal_wrapped(|ui| {
            use aurastudio_layers::Refer;
            ui.selectable_value(&mut wd.refer, Refer::Layer, "This layer");
            ui.selectable_value(&mut wd.refer, Refer::All, "All layers");
            ui.selectable_value(&mut wd.refer, Refer::References, "References");
        });
        let refer = wd.refer;
        ui.add_enabled_ui(refer != aurastudio_layers::Refer::Layer, |ui| { ui.checkbox(&mut m.select.wand.skip_drafts, "Ignore draft layers"); });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Select all").on_hover_text("Ctrl+A").clicked() { a.select = Some(SelectCmd::All); }
        if ui.add_enabled(m.has_selection, egui::Button::new("Deselect")).on_hover_text("Ctrl+D").clicked() {
            a.select = Some(SelectCmd::None);
        }
        if ui.add_enabled(m.has_selection, egui::Button::new("Invert")).on_hover_text("Ctrl+Shift+I").clicked() {
            a.select = Some(SelectCmd::Invert);
        }
    });
}

/// How the transform tool's handles work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransformMode {
    /// Move, scale, rotate, skew; Ctrl+corner distorts.
    #[default]
    Free,
    /// Each corner on its own.
    Perspective,
    /// A smooth grid to bend.
    Warp,
}

/// Settings of the transform tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformOptions {
    pub mode:        TransformMode,
    pub interp:      aurastudio_layers::transform::Interp,
    /// Corners and edges keep the proportions (Shift: free).
    pub keep_aspect: bool,
    /// Warp grid cells a side.
    pub grid:        u32,
}

impl Default for TransformOptions {
    fn default() -> Self { Self { mode: TransformMode::Free, interp: Default::default(), keep_aspect: true, grid: 3 } }
}

/// The transform as numbers, when its box is a parallelogram: center x,
/// y (px), width and height (% of the source), angle and skew (degrees).
pub type TransformNumbers = [f64; 6];

/// The transform under way, as the options show it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransformInfo {
    pub active:  bool,
    pub numbers: Option<TransformNumbers>,
}

/// What the transform tool's options asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransformCmd { Apply, Cancel, Reset, FlipH, FlipV, Rotate(f64), Numbers(TransformNumbers), ResetWarp, Grid(u32) }

/// The transform tool's options.
pub fn transform_options(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    use crate::icons::Icon;
    use crate::widgets::{icon_button, icon_toggle};
    let info = m.transform_info;
    let t = &mut m.transform;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (mode, icon, tip) in [(TransformMode::Free, Icon::Transform, "Free"), (TransformMode::Perspective, Icon::Perspective, "Perspective"), (TransformMode::Warp, Icon::Warp, "Warp")] {
            if icon_toggle(ui, icon, tip, true, t.mode == mode).clicked() { t.mode = mode; }
        }
        ui.add_space(8.0);
        let on = info.active;
        if icon_button(ui, Icon::FlipH, "Flip horizontally", on) { a.transform = Some(TransformCmd::FlipH); }
        if icon_button(ui, Icon::FlipV, "Flip vertically", on) { a.transform = Some(TransformCmd::FlipV); }
        if icon_button(ui, Icon::RotateLeft, "Rotate 90° left", on) { a.transform = Some(TransformCmd::Rotate(-90.0)); }
        if icon_button(ui, Icon::RotateRight, "Rotate 90° right", on) { a.transform = Some(TransformCmd::Rotate(90.0)); }
    });
    ui.add_space(4.0);
    let t = &mut m.transform;
    ui.horizontal(|ui| {
        use aurastudio_layers::transform::Interp;
        ui.label(RichText::new("Resampling").small().color(theme::TEXT_WEAK));
        egui::ComboBox::from_id_salt("transform_interp")
            .selected_text(match t.interp { Interp::Bicubic => "Bicubic", Interp::Bilinear => "Bilinear", Interp::Nearest => "Nearest" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut t.interp, Interp::Bicubic, "Bicubic");
                ui.selectable_value(&mut t.interp, Interp::Bilinear, "Bilinear");
                ui.selectable_value(&mut t.interp, Interp::Nearest, "Nearest");
            });
    });
    if t.mode == TransformMode::Free { ui.checkbox(&mut t.keep_aspect, "Keep proportions"); }
    if t.mode == TransformMode::Warp {
        let w = ui.available_width();
        let grid = Fill { label: "Grid", range: 1.0..=8.0, mid: None, format: &|v| format!("{v:.0} × {v:.0}") };
        if let Some(v) = fill_slider(ui, "warp_grid", grid, t.grid as f32, w) { a.transform = Some(TransformCmd::Grid(v as u32)); }
    }
    // The numbers (a box that is still a parallelogram).
    if let (Some(mut n), TransformMode::Free) = (info.numbers, m.transform.mode) {
        ui.add_space(4.0);
        let before = n;
        egui::Grid::new("transform_numbers").num_columns(4).spacing(vec2(6.0, 4.0)).show(ui, |ui| {
            let field = |ui: &mut Ui, label: &str, v: &mut f64, suffix: &str, speed: f64| {
                ui.label(RichText::new(label).small().color(theme::TEXT_WEAK));
                ui.add(egui::DragValue::new(v).speed(speed).max_decimals(1).suffix(suffix));
            };
            field(ui, "X", &mut n[0], " px", 1.0);
            field(ui, "Y", &mut n[1], " px", 1.0);
            ui.end_row();
            field(ui, "W", &mut n[2], " %", 0.5);
            field(ui, "H", &mut n[3], " %", 0.5);
            ui.end_row();
            field(ui, "Angle", &mut n[4], "°", 0.5);
            field(ui, "Skew", &mut n[5], "°", 0.5);
            ui.end_row();
        });
        if n != before && n.iter().all(|v| v.is_finite()) { a.transform = Some(TransformCmd::Numbers(n)); }
    }
    ui.add_space(6.0);
    let on = info.active;
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply").color(theme::TEXT)).fill(theme::ACCENT);
        if ui.add_enabled(on, apply).on_hover_text("Enter").clicked() { a.transform = Some(TransformCmd::Apply); }
        if ui.add_enabled(on, egui::Button::new("Cancel")).on_hover_text("Esc").clicked() { a.transform = Some(TransformCmd::Cancel); }
        let reset = if m.transform.mode == TransformMode::Warp { TransformCmd::ResetWarp } else { TransformCmd::Reset };
        if ui.add_enabled(on, egui::Button::new("Reset")).clicked() { a.transform = Some(reset); }
    });
}

/// The transform box drawn over the canvas (canvas pixels).
#[derive(Default)]
pub struct TransformOverlay<'a> {
    /// Box edges, grid lines.
    pub lines:   &'a [Vec<(f32, f32)>],
    /// Square handles (corners, edges).
    pub handles: &'a [(f32, f32)],
    /// Round handles (warp points).
    pub points:  &'a [(f32, f32)],
    pub pivot:   Option<(f32, f32)>,
}

/// What the tools draw over the canvas this frame, in canvas pixels.
#[derive(Default)]
pub struct Overlay<'a> {
    /// Outline of the current selection, moved by `ants_offset` (being
    /// dragged).
    pub ants:        &'a [aurastudio_layers::Segment],
    pub ants_offset: (f32, f32),
    /// Lasso, polygon or box being drawn (closed when shown).
    pub drawing:     Option<&'a [(f32, f32)]>,
    /// Shape about to be painted (geometry tools), and its handles.
    pub shape:       Option<&'a [(f32, f32)]>,
    pub handles:     &'a [(f32, f32)],
    /// Dashed construction lines (the arc's start, control point, end).
    pub guide:       &'a [(f32, f32)],
    pub transform:   Option<TransformOverlay<'a>>,
    pub loupe:       Option<Loupe<'a>>,
}

/// A magnified view of the canvas around the cursor.
pub struct Loupe<'a> {
    /// The cursor, canvas pixels (the middle of the view).
    pub cursor: (f32, f32),
    /// What a press takes: a pixel (its top-left corner), else the pixel
    /// corner nearest the cursor.
    pub target: (f32, f32),
    pub pixel:  bool,
    /// The merged image around: a `side` × `side` square of canvas pixels
    /// from `origin` (outside the canvas: transparent).
    pub origin: (i32, i32),
    pub side:   u32,
    pub rgba:   &'a [u8],
    /// The color taken, shown around the view (picker).
    pub ring:   Option<[u8; 3]>,
    /// The selection mode's sign.
    pub mark:   Option<&'static str>,
}

/// Canvas pixels the loupe sends (so the view's widest span fits).
pub const LOUPE_SIDE: u32 = 27;

/// Diameter of the loupe, points.
const LOUPE_SIZE: f32 = 132.0;

fn draw_loupe(ctx: &egui::Context, painter: &egui::Painter, l: &Loupe, pt: &dyn Fn((f32, f32)) -> Pos2) {
    let o = pt(l.cursor);
    let ex = pt((l.cursor.0 + 1.0, l.cursor.1)) - o;
    let ey = pt((l.cursor.0, l.cursor.1 + 1.0)) - o;
    let zoom = ex.length().max(1e-4);
    // Canvas pixels across: three times the view's zoom, 7 to 21.
    let span = (LOUPE_SIZE / (zoom * 3.0)).clamp(7.0, 21.0);
    let k = LOUPE_SIZE / span / zoom;
    let r = LOUPE_SIZE / 2.0;
    // Canvas point <-> point in the loupe.
    let det = ex.x * ey.y - ex.y * ey.x;
    if det.abs() < 1e-9 || l.rgba.len() != (l.side * l.side * 4) as usize { return; }
    let to_loupe = |q: (f32, f32)| o + (ex * (q.0 - l.cursor.0) + ey * (q.1 - l.cursor.1)) * k;
    let to_canvas = |p: Pos2| {
        let d = (p - o) / k;
        (l.cursor.0 + (d.x * ey.y - d.y * ey.x) / det, l.cursor.1 + (ex.x * d.y - ex.y * d.x) / det)
    };
    // The pixels, as a texture kept between frames.
    let image = egui::ColorImage::from_rgba_unmultiplied([l.side as usize, l.side as usize], l.rgba);
    let id = egui::Id::new("aurastudio_loupe");
    let tex = match ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        Some(mut t) => { t.set(image, egui::TextureOptions::NEAREST); t }
        None => {
            let t = ctx.load_texture("loupe", image, egui::TextureOptions::NEAREST);
            ctx.data_mut(|d| d.insert_temp(id, t.clone()));
            t
        }
    };
    painter.circle_filled(o, r, theme::DESK);
    let mut mesh = egui::Mesh::with_texture(tex.id());
    let n = 64;
    let uv = |p: Pos2| {
        let q = to_canvas(p);
        pos2((q.0 - l.origin.0 as f32) / l.side as f32, (q.1 - l.origin.1 as f32) / l.side as f32)
    };
    mesh.vertices.push(egui::epaint::Vertex { pos: o, uv: uv(o), color: Color32::WHITE });
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let p = o + vec2(a.cos(), a.sin()) * r;
        mesh.vertices.push(egui::epaint::Vertex { pos: p, uv: uv(p), color: Color32::WHITE });
        if i > 0 { mesh.add_triangle(0, i, i + 1); }
    }
    painter.add(Shape::mesh(mesh));
    // What a press takes.
    let dark = Stroke::new(3.0_f32, Color32::from_black_alpha(170));
    let light = Stroke::new(1.0_f32, Color32::WHITE);
    let (tx, ty) = l.target;
    if l.pixel {
        let sq = vec![to_loupe((tx, ty)), to_loupe((tx + 1.0, ty)), to_loupe((tx + 1.0, ty + 1.0)), to_loupe((tx, ty + 1.0))];
        painter.add(Shape::closed_line(sq.clone(), dark));
        painter.add(Shape::closed_line(sq, light));
    } else {
        let c = to_loupe((tx, ty));
        let arm = (LOUPE_SIZE / span * 0.9).clamp(6.0, 14.0);
        for d in [vec2(1.0, 0.0), vec2(0.0, 1.0)] {
            for s in [-1.0, 1.0] {
                let seg = [c + d * s * 2.0, c + d * s * arm];
                painter.line_segment(seg, dark);
                painter.line_segment(seg, light);
            }
        }
    }
    // The rim: the color taken (picker), else a plain edge.
    if let Some([cr, cg, cb]) = l.ring {
        painter.circle_stroke(o, r + 4.0, Stroke::new(8.0_f32, Color32::from_rgb(cr, cg, cb)));
        painter.circle_stroke(o, r + 8.5, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
    }
    painter.circle_stroke(o, r, Stroke::new(2.0_f32, Color32::from_black_alpha(170)));
    painter.circle_stroke(o, r - 1.0, Stroke::new(1.0_f32, Color32::from_white_alpha(200)));
    if let Some(m) = l.mark {
        let c = o + vec2(r, r) * std::f32::consts::FRAC_1_SQRT_2;
        painter.circle(c, 9.0, theme::DESK, Stroke::new(1.5_f32, theme::ACCENT));
        painter.text(c, egui::Align2::CENTER_CENTER, m, egui::FontId::proportional(15.0), theme::TEXT);
    }
}

/// One dashed black and white line in `mesh`, screen points; the dashes
/// follow `phase` (points along the line where it starts).
fn ant_line(mesh: &mut egui::Mesh, a: Pos2, b: Pos2, phase: f32) {
    let d = b - a;
    let len = d.length();
    if len < 0.01 { return; }
    let n = d / len;
    let side = vec2(-n.y, n.x) * 0.5;
    let quad = |mesh: &mut egui::Mesh, p: Pos2, q: Pos2, c: Color32| {
        let i = mesh.vertices.len() as u32;
        for v in [p - side, p + side, q + side, q - side] { mesh.colored_vertex(v, c); }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    quad(mesh, a, b, Color32::BLACK);
    const DASH: f32 = 4.0;
    let mut t = -(phase.rem_euclid(DASH * 2.0));
    while t < len {
        let (s, e) = (t.max(0.0), (t + DASH).min(len));
        if e > s { quad(mesh, a + n * s, a + n * e, Color32::WHITE); }
        t += DASH * 2.0;
    }
}

/// Selection outlines as static "marching ants" (black and white dashes,
/// not animated: an animation would redraw the UI all the time), and what
/// the tools show.
pub fn draw_overlay(ctx: &egui::Context, clip: Rect, overlay: &Overlay, to_points: &dyn Fn((f32, f32)) -> (f32, f32)) {
    let painter = ctx.layer_painter(egui::LayerId::background()).with_clip_rect(clip);
    let pt = |p: (f32, f32)| { let (x, y) = to_points(p); pos2(x, y) };
    let mut mesh = egui::Mesh::default();
    let (ox, oy) = overlay.ants_offset;
    let grown = clip.expand(2.0);
    for seg in overlay.ants {
        let (a, b) = (pt((seg[0].0 + ox, seg[0].1 + oy)), pt((seg[1].0 + ox, seg[1].1 + oy)));
        if !grown.intersects(Rect::from_two_pos(a, b)) { continue; }
        ant_line(&mut mesh, a, b, a.x + a.y);
    }
    let outline = |mesh: &mut egui::Mesh, pts: &[(f32, f32)], closed: bool| {
        if pts.len() < 2 { return; }
        let v: Vec<Pos2> = pts.iter().map(|&p| pt(p)).collect();
        let mut phase = 0.0;
        let n = if closed { v.len() } else { v.len() - 1 };
        for i in 0..n {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            ant_line(mesh, a, b, phase);
            phase += (b - a).length();
        }
    };
    if let Some(d) = overlay.drawing { outline(&mut mesh, d, true); }
    if !mesh.is_empty() { painter.add(Shape::mesh(mesh)); }
    if let Some(s) = overlay.shape {
        let v: Vec<Pos2> = s.iter().map(|&p| pt(p)).collect();
        // Dark under light: visible on any color.
        painter.add(Shape::line(v.clone(), Stroke::new(3.0_f32, Color32::from_black_alpha(140))));
        painter.add(Shape::line(v, Stroke::new(1.5_f32, theme::ACCENT)));
    }
    if overlay.guide.len() >= 2 {
        let v: Vec<Pos2> = overlay.guide.iter().map(|&p| pt(p)).collect();
        painter.extend(Shape::dashed_line(&v, Stroke::new(1.0_f32, theme::TEXT_WEAK), 5.0, 4.0));
    }
    for &h in overlay.handles {
        painter.circle(pt(h), 4.0, Color32::WHITE, Stroke::new(1.5_f32, theme::ACCENT));
    }
    if let Some(t) = &overlay.transform {
        for l in t.lines {
            let v: Vec<Pos2> = l.iter().map(|&p| pt(p)).collect();
            painter.add(Shape::line(v.clone(), Stroke::new(3.0_f32, Color32::from_black_alpha(120))));
            painter.add(Shape::line(v, Stroke::new(1.0_f32, theme::ACCENT)));
        }
        for &h in t.handles {
            let r = Rect::from_center_size(pt(h), vec2(8.0, 8.0));
            painter.rect(r, 1.0, Color32::WHITE, Stroke::new(1.5_f32, theme::ACCENT));
        }
        for &h in t.points {
            painter.circle(pt(h), 4.0, Color32::WHITE, Stroke::new(1.5_f32, theme::ACCENT));
        }
        if let Some(p) = t.pivot {
            let c = pt(p);
            painter.circle_stroke(c, 5.0, Stroke::new(3.0_f32, Color32::from_black_alpha(120)));
            painter.circle_stroke(c, 5.0, Stroke::new(1.5_f32, Color32::WHITE));
            for d in [vec2(8.0, 0.0), vec2(0.0, 8.0)] {
                painter.line_segment([c - d, c + d], Stroke::new(1.0_f32, Color32::WHITE));
            }
        }
    }
    if let Some(l) = &overlay.loupe { draw_loupe(ctx, &painter, l, &pt); }
}

/// Options of the geometry tools, under the brush settings.
pub fn shape_options(ui: &mut Ui, m: &mut Model) {
    if *m.tool == Tool::Arc {
        ui.add_space(4.0);
        ui.checkbox(&mut m.shape.multipoint, "Multipoint").on_hover_text("Enter: done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_brushes::{mediums_of, Library, MEDIUMS};

    #[test]
    fn every_medium_has_a_group() {
        let lib = Library::load_bundled();
        let all: Vec<&str> = Group::ALL.iter().filter_map(|g| g.kind()).flat_map(|k| mediums_of(&lib.mediums, k)).collect();
        for m in MEDIUMS { assert_eq!(all.iter().filter(|x| *x == m).count(), 1, "{m}"); }
    }

    #[test]
    fn groups_remember_their_tool_and_brush() {
        let lib = Library::load();
        let p = &lib.presets;
        let find = |n: &str| p.iter().position(|x| x.name == n).unwrap();
        let (mut tool, mut preset) = (Tool::Brush, find("Liner"));
        let mut mem = ToolMemory::default();
        assert_eq!(Group::current(tool, preset, p), Group::Draw);

        mem.select(Group::Paint, &mut tool, &mut preset, p);
        assert_eq!((tool, Group::current(tool, preset, p)), (Tool::Brush, Group::Paint));
        preset = find("Oil");
        mem.select(Group::Eraser, &mut tool, &mut preset, p);
        assert_eq!(tool, Tool::Eraser);
        assert_eq!(p[preset].medium, "Eraser");
        mem.select(Group::Geometry, &mut tool, &mut preset, p);
        assert_eq!(tool, Tool::Line);
        // U again: the next shape.
        mem.select(Group::Geometry, &mut tool, &mut preset, p);
        assert_eq!(tool, Tool::Rectangle);
        mem.select(Group::Draw, &mut tool, &mut preset, p);
        assert_eq!((tool, preset), (Tool::Brush, find("Liner")));
        mem.select(Group::Paint, &mut tool, &mut preset, p);
        assert_eq!(preset, find("Oil"));
        mem.select(Group::Geometry, &mut tool, &mut preset, p);
        assert_eq!(tool, Tool::Rectangle);
    }
}

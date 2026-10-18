//! Tools: the vertical toolbox on the left of the canvas (fused with its
//! edge, like Krita's), each tool's options, and what the tools draw over
//! the canvas (selection outline, lasso being drawn).

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
    Line,
    Rectangle,
    Ellipse,
    Arc,
}

impl Tool {
    /// Toolbox order. No hand or zoom tool: Space / Ctrl+Space drags and
    /// the wheel do that from any tool.
    pub const ALL: [Tool; 10] = [
        Tool::Brush, Tool::Eraser, Tool::Fill, Tool::Picker,
        Tool::Line, Tool::Rectangle, Tool::Ellipse, Tool::Arc,
        Tool::RectSelect, Tool::Lasso,
    ];

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
            Tool::Line       => "Line",
            Tool::Rectangle  => "Rectangle",
            Tool::Ellipse    => "Ellipse",
            Tool::Arc        => "Arc",
        }
    }

    /// Shortcut key (Krita / Photoshop letters).
    pub fn key(self) -> &'static str {
        match self {
            Tool::Brush      => "B",
            Tool::Eraser     => "E",
            Tool::Picker     => "I",
            Tool::Fill       => "G",
            Tool::Lasso      => "L",
            Tool::RectSelect => "M",
            Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc => "U",
        }
    }

    /// The tool of a shortcut key, if any (U: the next geometry tool after
    /// `current`).
    pub fn from_key(c: &str, current: Tool) -> Option<Tool> {
        if c.eq_ignore_ascii_case("u") {
            let i = Tool::SHAPES.iter().position(|&t| t == current).map_or(0, |i| (i + 1) % Tool::SHAPES.len());
            return Some(Tool::SHAPES[i]);
        }
        Tool::ALL.into_iter().find(|t| t.key().eq_ignore_ascii_case(c))
    }

    fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Tool::Brush      => Icon::Brush,
            Tool::Eraser     => Icon::Eraser,
            Tool::Picker     => Icon::Picker,
            Tool::Fill       => Icon::Fill,
            Tool::Lasso      => Icon::Lasso,
            Tool::RectSelect => Icon::RectSelect,
            Tool::Line       => Icon::Line,
            Tool::Rectangle  => Icon::Rectangle,
            Tool::Ellipse    => Icon::Ellipse,
            Tool::Arc        => Icon::Arc,
        }
    }

    /// Paints with the brush (size, opacity... apply; outline cursor).
    pub fn paints(self) -> bool { matches!(self, Tool::Brush | Tool::Eraser) || self.is_shape() }

    pub fn is_shape(self) -> bool { Tool::SHAPES.contains(&self) }

    /// Separator above it in the toolbox.
    fn group_start(self) -> bool { matches!(self, Tool::Line | Tool::RectSelect) }
}

/// Settings of the geometry tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOptions {
    /// Arc: once validated, the next arc starts at its end point (closed,
    /// clean outlines for coloring).
    pub chain_arcs:   bool,
    /// Arc: a piece of circle instead of a free curve.
    pub circular_arc: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self { Self { chain_arcs: true, circular_arc: false } }
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
                    for t in Tool::ALL {
                        if t.group_start() {
                            ui.add_space(3.0);
                            let (r, _) = ui.allocate_exact_size(vec2(TOOLBOX_WIDTH - 8.0, 1.0), Sense::hover());
                            ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), 0.0, theme::BORDER);
                            ui.add_space(3.0);
                        }
                        if tool_button(ui, t, *m.tool == t, !m.home) { *m.tool = t; }
                    }
                });
        })
        .response
        .rect
}

fn tool_button(ui: &mut Ui, tool: Tool, selected: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(TOOLBOX_WIDTH - 8.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(format!("{} ({})", tool.name(), tool.key()));
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 6.0, theme::ACCENT_DIM);
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 6.0, theme::WIDGET_HOVER);
    }
    let col = match (enabled, selected) {
        (false, _) => theme::TEXT_WEAK.gamma_multiply(0.45),
        (true, true) => theme::TEXT,
        (true, false) => Color32::from_gray(0xc8),
    };
    crate::icons::paint(ui, tool.icon(), rect, 20.0, col);
    enabled && resp.clicked()
}


/// Options of the tools that are not brushes (Tool options docker).
pub fn options(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    let hint = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).small().color(theme::TEXT_WEAK)); };
    ui.label(RichText::new(m.tool.name()).strong());
    ui.add_space(4.0);
    let w = ui.available_width();
    match *m.tool {
        Tool::Fill => {
            fill_options(ui, m, w);
            hint(ui, "Fills with the brush color. Alt+click picks a color.");
        }
        Tool::Picker => hint(ui, "Click or drag on the canvas: takes the color of the merged image. With the brush, hold Alt."),
        Tool::Lasso | Tool::RectSelect => {
            selection_buttons(ui, m, a);
            hint(ui, "Drag to select; a click without dragging deselects. Painting, fill and Delete then only change the selection.");
        }
        Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc => {}
    }
}

/// Tolerance, reference and expand of the fill tool.
pub fn fill_options(ui: &mut Ui, m: &mut Model, w: f32) {
    let tol = m.fill.tolerance as f32 / 255.0 * 100.0;
    if let Some(v) = fill_slider(ui, "fill_tolerance", crate::widgets::percent("Tolerance"), tol, w) {
        m.fill.tolerance = (v / 100.0 * 255.0).round() as u8;
    }
    let expand = Fill { label: "Expand", range: 0.0..=8.0, log: false, format: &|v| format!("{v:.0} px") };
    if let Some(v) = fill_slider(ui, "fill_expand", expand, m.fill.expand as f32, w) {
        m.fill.expand = v as u32;
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut m.fill.merged, true, "All layers").on_hover_text("Find the area on the merged image (lines on another layer stop the fill)");
        ui.selectable_value(&mut m.fill.merged, false, "This layer");
    });
}

pub fn selection_buttons(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    ui.horizontal(|ui| {
        if ui.button("Select all").on_hover_text("Ctrl+A").clicked() { a.select = Some(SelectCmd::All); }
        if ui.add_enabled(m.has_selection, egui::Button::new("Deselect")).on_hover_text("Ctrl+D").clicked() {
            a.select = Some(SelectCmd::None);
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectCmd { All, None }

/// What the tools draw over the canvas this frame, in canvas pixels.
#[derive(Default)]
pub struct Overlay<'a> {
    /// Outlines of the current selection.
    pub selection: &'a [Vec<(f32, f32)>],
    /// Lasso or rectangle being drawn (closed when shown).
    pub drawing:   Option<&'a [(f32, f32)]>,
    /// Shape about to be painted (geometry tools), and its handles.
    pub shape:     Option<&'a [(f32, f32)]>,
    pub handles:   &'a [(f32, f32)],
}

/// Selection outlines as static "marching ants" (black and white dashes,
/// not animated: an animation would redraw the UI all the time).
pub fn draw_overlay(ctx: &egui::Context, clip: Rect, overlay: &Overlay, to_points: &dyn Fn((f32, f32)) -> (f32, f32)) {
    let painter = ctx.layer_painter(egui::LayerId::background()).with_clip_rect(clip);
    let outline = |pts: &[(f32, f32)]| {
        if pts.len() < 2 { return; }
        let mut v: Vec<Pos2> = pts.iter().map(|&p| { let (x, y) = to_points(p); pos2(x, y) }).collect();
        v.push(v[0]);
        painter.add(Shape::line(v.clone(), Stroke::new(1.0_f32, Color32::BLACK)));
        painter.extend(Shape::dashed_line(&v, Stroke::new(1.0_f32, Color32::WHITE), 4.0, 4.0));
    };
    for o in overlay.selection { outline(o); }
    if let Some(d) = overlay.drawing { outline(d); }
    let pt = |p: (f32, f32)| { let (x, y) = to_points(p); pos2(x, y) };
    if let Some(s) = overlay.shape {
        let v: Vec<Pos2> = s.iter().map(|&p| pt(p)).collect();
        // Dark under light: visible on any color.
        painter.add(Shape::line(v.clone(), Stroke::new(3.0_f32, Color32::from_black_alpha(140))));
        painter.add(Shape::line(v, Stroke::new(1.5_f32, theme::ACCENT)));
    }
    for &h in overlay.handles {
        painter.circle(pt(h), 4.0, Color32::WHITE, Stroke::new(1.5_f32, theme::ACCENT));
    }
}

/// Options of the geometry tools, under the brush settings.
pub fn shape_options(ui: &mut Ui, m: &mut Model) {
    let hint = |ui: &mut Ui, t: &str| { ui.label(RichText::new(t).small().color(theme::TEXT_WEAK)); };
    ui.add_space(4.0);
    match *m.tool {
        Tool::Line => hint(ui, "Drag to draw. Shift: steps of 15°. Drawn with the current brush."),
        Tool::Rectangle => hint(ui, "Drag to draw. Shift: square, Alt: from the center."),
        Tool::Ellipse => hint(ui, "Drag to draw. Shift: circle, Alt: from the center."),
        Tool::Arc => {
            ui.checkbox(&mut m.shape.chain_arcs, "Chain: next arc starts at the end point")
                .on_hover_text("Each validated arc starts the next one where it ended: outlines stay closed for coloring. Escape stops.");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut m.shape.circular_arc, false, "Free curve");
                ui.selectable_value(&mut m.shape.circular_arc, true, "Circle arc");
            });
            hint(ui, "Drag (or click twice) from start to end, move to bend, click to validate. Escape cancels.");
        }
        _ => {}
    }
}

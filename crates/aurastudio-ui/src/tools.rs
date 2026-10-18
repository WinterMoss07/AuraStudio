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
    Hand,
    Zoom,
}

impl Tool {
    pub const ALL: [Tool; 8] = [
        Tool::Brush, Tool::Eraser, Tool::Fill, Tool::Picker,
        Tool::RectSelect, Tool::Lasso, Tool::Hand, Tool::Zoom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Brush      => "Brush",
            Tool::Eraser     => "Eraser",
            Tool::Picker     => "Color picker",
            Tool::Fill       => "Fill",
            Tool::Lasso      => "Lasso selection",
            Tool::RectSelect => "Rectangle selection",
            Tool::Hand       => "Hand",
            Tool::Zoom       => "Zoom",
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
            Tool::Hand       => "H",
            Tool::Zoom       => "Z",
        }
    }

    /// The tool of a shortcut key, if any.
    pub fn from_key(c: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.key().eq_ignore_ascii_case(c))
    }

    /// Paints with the brush (size, opacity... apply; outline cursor).
    pub fn paints(self) -> bool { matches!(self, Tool::Brush | Tool::Eraser) }

    /// Separator above it in the toolbox.
    fn group_start(self) -> bool { matches!(self, Tool::RectSelect | Tool::Hand) }
}

/// Width of the toolbox column.
pub const TOOLBOX_WIDTH: f32 = 40.0;

pub fn toolbox(ctx: &egui::Context, m: &mut Model) {
    egui::SidePanel::left("toolbox")
        .exact_width(TOOLBOX_WIDTH)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::none().fill(theme::PANEL).inner_margin(egui::Margin { top: 4.0, ..Default::default() }))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.vertical_centered(|ui| {
                for t in Tool::ALL {
                    if t.group_start() {
                        ui.add_space(3.0);
                        let (r, _) = ui.allocate_exact_size(vec2(22.0, 1.0), Sense::hover());
                        ui.painter().rect_filled(r, 0.0, theme::BORDER);
                        ui.add_space(3.0);
                    }
                    if tool_button(ui, t, *m.tool == t, !m.home) { *m.tool = t; }
                }
            });
        });
}

fn tool_button(ui: &mut Ui, tool: Tool, selected: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
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
        (true, false) => Color32::from_gray(0xd0),
    };
    draw_tool_icon(&p, rect.center(), tool, col);
    enabled && resp.clicked()
}

/// Tool icons, drawn with shapes in a ~18 px box.
fn draw_tool_icon(p: &egui::Painter, c: Pos2, tool: Tool, col: Color32) {
    let st = Stroke::new(1.4_f32, col);
    let at = |x: f32, y: f32| c + vec2(x, y);
    match tool {
        Tool::Brush => {
            // Handle, ferrule, bristles.
            p.line_segment([at(7.5, -7.5), at(1.5, -1.5)], Stroke::new(2.0_f32, col));
            p.line_segment([at(1.8, -1.8), at(-0.8, 0.8)], Stroke::new(4.0_f32, col));
            p.add(Shape::convex_polygon(vec![at(-2.4, -0.6), at(0.6, 2.4), at(-3.5, 6.0), at(-7.5, 7.5), at(-6.0, 3.5)], col, Stroke::NONE));
        }
        Tool::Eraser => {
            // A tilted block, its working end filled.
            let q = |x: f32, y: f32| { let (s, k) = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2); at(k * x - s * y, s * x + k * y) };
            p.add(Shape::closed_line(vec![q(-8.0, -3.5), q(8.0, -3.5), q(8.0, 3.5), q(-8.0, 3.5)], st));
            p.add(Shape::convex_polygon(vec![q(-8.0, -3.5), q(-2.0, -3.5), q(-2.0, 3.5), q(-8.0, 3.5)], col, Stroke::NONE));
        }
        Tool::Picker => {
            // Eyedropper: bulb, body, thin tip.
            p.circle_filled(at(5.0, -5.0), 2.8, col);
            p.line_segment([at(3.0, -3.0), at(-3.0, 3.0)], Stroke::new(3.0_f32, col));
            p.line_segment([at(-3.0, 3.0), at(-7.0, 7.0)], st);
        }
        Tool::Fill => {
            // Tilted bucket and a drop.
            let q = |x: f32, y: f32| { let (s, k) = (0.5, 0.866); at(-2.0 + k * x - s * y, 1.0 + s * x + k * y) };
            p.add(Shape::closed_line(vec![q(-5.0, -4.0), q(5.0, -4.0), q(4.0, 5.0), q(-4.0, 5.0)], st));
            p.line_segment([q(-5.0, -4.0), q(0.0, -8.0)], st);
            p.line_segment([q(5.0, -4.0), q(0.0, -8.0)], st);
            p.circle_filled(at(6.5, 4.0), 2.0, col);
            p.add(Shape::convex_polygon(vec![at(5.0, 3.2), at(6.5, -0.5), at(8.0, 3.2)], col, Stroke::NONE));
        }
        Tool::Lasso => {
            // Loop and its tail.
            let pts: Vec<Pos2> = (0..24).map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                at(1.0 + 7.0 * a.cos(), -2.0 + 4.5 * a.sin())
            }).collect();
            p.add(Shape::closed_line(pts, st));
            p.add(Shape::line(vec![at(-4.0, 1.5), at(-5.0, 4.5), at(-3.0, 7.0), at(-5.5, 8.5)], st));
        }
        Tool::RectSelect => {
            let r = Rect::from_center_size(c, vec2(15.0, 12.0));
            let (a, b, cc, d) = (r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom());
            for (u, v) in [(a, b), (b, cc), (cc, d), (d, a)] {
                p.extend(Shape::dashed_line(&[u, v], st, 2.5, 2.0));
            }
        }
        Tool::Hand => {
            // Palm and four fingers, thumb on the side.
            p.rect_filled(Rect::from_min_max(at(-4.5, -1.0), at(5.0, 7.0)), 3.0, col);
            for (i, h) in [5.0, 7.0, 7.0, 5.5].iter().enumerate() {
                let x = -3.5 + i as f32 * 2.6;
                p.line_segment([at(x, 0.0), at(x, -1.0 - h)], Stroke::new(2.0_f32, col));
            }
            p.line_segment([at(-4.0, 3.0), at(-7.5, -0.5)], Stroke::new(2.0_f32, col));
        }
        Tool::Zoom => {
            p.circle_stroke(at(-1.5, -1.5), 5.5, st);
            p.line_segment([at(2.5, 2.5), at(7.0, 7.0)], Stroke::new(2.4_f32, col));
        }
    }
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
        Tool::Hand => hint(ui, "Drag to move the view (Space + drag with any tool)."),
        Tool::Zoom => hint(ui, "Click to zoom in, Alt+click to zoom out, drag left / right to zoom."),
        Tool::Brush | Tool::Eraser => {}
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
}

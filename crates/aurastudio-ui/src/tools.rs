//! Tools: the vertical toolbox on the left of the canvas (fused with its
//! edge, like Krita's), each tool's options, and what the tools draw over
//! the canvas (selection outline, lasso being drawn).
//!
//! The toolbox shows groups of tools (Draw, Paint, Eraser, Fill, Picker,
//! Geometry, Selection); the Tools docker lists the chosen group's tools
//! (or brushes). New tools join a group, the toolbox stays short. Each
//! group remembers the tool (and brush) last used in it.

use std::collections::HashMap;

use aurastudio_brushes::Preset;
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

    pub fn icon(self) -> crate::icons::Icon {
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

    /// A few words shown on hover.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Brush      => "Paint",
            Tool::Eraser     => "Erase",
            Tool::Picker     => "Click: take the color",
            Tool::Fill       => "Click: fill an area",
            Tool::Lasso      => "Drag: freehand selection",
            Tool::RectSelect => "Drag: rectangle selection",
            Tool::Line       => "Drag. Shift: 15° steps",
            Tool::Rectangle  => "Drag. Shift: square, Alt: centered",
            Tool::Ellipse    => "Drag. Shift: circle, Alt: centered",
            Tool::Arc        => "Start, end, bend. Enter: done",
        }
    }

    /// Paints with the brush (size, opacity... apply; outline cursor).
    pub fn paints(self) -> bool { matches!(self, Tool::Brush | Tool::Eraser) || self.is_shape() }

    pub fn is_shape(self) -> bool { Tool::SHAPES.contains(&self) }
}

/// A toolbox entry: a set of tools listed in the Tools docker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group { Draw, Paint, Eraser, Fill, Picker, Geometry, Selection }

impl Group {
    /// Toolbox order. No hand or zoom tool: Space / Ctrl+Space drags and
    /// the wheel do that from any tool.
    pub const ALL: [Group; 7] = [Group::Draw, Group::Paint, Group::Eraser, Group::Fill, Group::Picker, Group::Geometry, Group::Selection];

    pub fn name(self) -> &'static str {
        match self {
            Group::Draw => "Draw",
            Group::Paint => "Paint",
            Group::Eraser => "Eraser",
            Group::Fill => "Fill",
            Group::Picker => "Color picker",
            Group::Geometry => "Geometry",
            Group::Selection => "Selection",
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
            Group::Selection => &[Tool::RectSelect, Tool::Lasso],
        }
    }

    /// Brush mediums of the group: dry techniques to Draw, wet ones to
    /// Paint. Empty: not a brush group.
    pub fn mediums(self) -> &'static [&'static str] {
        match self {
            Group::Draw => &["Pencil", "Ink", "Charcoal & pastel", "Misc"],
            Group::Paint => &["Paint", "Watercolor", "Airbrush", "Blend & smudge", "Texture & FX"],
            Group::Eraser => &["Eraser"],
            _ => &[],
        }
    }

    pub fn is_brushes(self) -> bool { !self.mediums().is_empty() }

    /// The group of a brush medium.
    pub fn of_medium(medium: &str) -> Group {
        [Group::Paint, Group::Eraser].into_iter().find(|g| g.mediums().contains(&medium)).unwrap_or(Group::Draw)
    }

    /// The group the current tool (and brush) belongs to.
    pub fn current(tool: Tool, preset: usize, presets: &[Preset]) -> Group {
        match tool {
            Tool::Brush => presets.get(preset).map_or(Group::Draw, |p| match Group::of_medium(&p.medium) {
                // An eraser brush with the Brush tool: still a brush.
                Group::Eraser => Group::Draw,
                g => g,
            }),
            Tool::Eraser => Group::Eraser,
            Tool::Fill => Group::Fill,
            Tool::Picker => Group::Picker,
            Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc => Group::Geometry,
            Tool::RectSelect | Tool::Lasso => Group::Selection,
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
            let first = || presets.iter().position(|p| g.mediums().contains(&p.medium.as_str()));
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
                        if matches!(g, Group::Fill | Group::Geometry) {
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
        p.rect_filled(rect, 6.0, theme::ACCENT_DIM);
    } else if enabled && resp.hovered() {
        p.rect_filled(rect, 6.0, theme::WIDGET_HOVER);
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
        Tool::Lasso | Tool::RectSelect => selection_buttons(ui, m, a),
        _ => {}
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
        ui.selectable_value(&mut m.fill.merged, true, "All layers").on_hover_text("Lines on any layer stop the fill");
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
    /// Dashed construction lines (the arc's start, control point, end).
    pub guide:     &'a [(f32, f32)],
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
    if overlay.guide.len() >= 2 {
        let v: Vec<Pos2> = overlay.guide.iter().map(|&p| pt(p)).collect();
        painter.extend(Shape::dashed_line(&v, Stroke::new(1.0_f32, theme::TEXT_WEAK), 5.0, 4.0));
    }
    for &h in overlay.handles {
        painter.circle(pt(h), 4.0, Color32::WHITE, Stroke::new(1.5_f32, theme::ACCENT));
    }
}

/// A geometry tool's shape, thin, in `rect` (Tools docker).
pub fn draw_shape(painter: &egui::Painter, tool: Tool, rect: Rect) {
    let stroke = Stroke::new(1.5_f32, theme::TEXT);
    let r = Rect::from_center_size(rect.center(), vec2((rect.height() * 1.6).min(rect.width()), rect.height()) * 0.6);
    match tool {
        Tool::Line => { painter.line_segment([r.left_bottom(), r.right_top()], stroke); }
        Tool::Rectangle => { painter.rect_stroke(r, 0.0, stroke); }
        Tool::Ellipse => {
            let pts = (0..=48).map(|i| {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                pos2(r.center().x + r.width() / 2.0 * a.cos(), r.center().y + r.height() / 2.0 * a.sin())
            }).collect();
            painter.add(Shape::line(pts, stroke));
        }
        Tool::Arc => {
            let (a, b, k) = (r.left_bottom(), r.right_bottom(), pos2(r.center().x, r.top() - r.height()));
            let pts = (0..=32).map(|i| {
                let t = i as f32 / 32.0;
                let u = 1.0 - t;
                pos2(u * u * a.x + 2.0 * u * t * k.x + t * t * b.x, u * u * a.y + 2.0 * u * t * k.y + t * t * b.y)
            }).collect();
            painter.add(Shape::line(pts, stroke));
        }
        _ => {}
    }
}

/// Options of the geometry tools, under the brush settings.
pub fn shape_options(ui: &mut Ui, m: &mut Model) {
    if *m.tool == Tool::Arc {
        ui.add_space(4.0);
        ui.checkbox(&mut m.shape.multipoint, "Multipoint").on_hover_text("Arcs follow each other. Enter: done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_brushes::{Library, MEDIUMS};

    #[test]
    fn every_medium_has_a_group() {
        let all: Vec<&str> = Group::ALL.iter().flat_map(|g| g.mediums().iter().copied()).collect();
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

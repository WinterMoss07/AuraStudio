//! Filter dialog drawings: histograms, bars, handles, sliders, the angle
//! dial (after PhotoCraft, MIT, see NOTICE).

use egui::{epaint, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui};

use crate::theme;

/// Width of the dialogs' content.
pub const WIDTH: f32 = 360.0;

pub const RED: Color32 = Color32::from_rgb(235, 80, 80);
pub const GREEN: Color32 = Color32::from_rgb(80, 205, 95);
pub const BLUE: Color32 = Color32::from_rgb(90, 140, 255);
/// The before histogram / curve.
pub const BEFORE: Color32 = Color32::from_gray(120);
/// The result's histogram / curve.
pub const AFTER: Color32 = theme::ACCENT;
pub const FIELD: Color32 = theme::DESK;

/// A framed graph area `h` points tall, the dialog's width. Returns its
/// inner rect, the response and a painter clipped to it.
pub fn graph(ui: &mut Ui, h: f32, sense: Sense) -> (Rect, Response, Painter) {
    let (r, resp) = ui.allocate_exact_size(vec2(WIDTH, h), sense);
    let p = ui.painter_at(r.expand(1.0));
    p.rect_filled(r, 2.0, FIELD);
    (r.shrink(1.0), resp, p)
}

pub fn frame(p: &Painter, r: Rect) {
    p.rect_stroke(r.expand(1.0), 2.0, Stroke::new(1.0_f32, theme::BORDER), egui::StrokeKind::Middle);
}

/// Light grid lines at quarters (and the diagonal, for a curve).
pub fn grid(p: &Painter, r: Rect, diagonal: bool) {
    let s = Stroke::new(1.0_f32, Color32::from_white_alpha(14));
    for k in 1..4 {
        let t = k as f32 / 4.0;
        p.line_segment([pos2(r.left() + t * r.width(), r.top()), pos2(r.left() + t * r.width(), r.bottom())], s);
        p.line_segment([pos2(r.left(), r.top() + t * r.height()), pos2(r.right(), r.top() + t * r.height())], s);
    }
    if diagonal {
        dashed(p, r.left_bottom(), r.right_top(), Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    }
}

pub fn dashed(p: &Painter, a: Pos2, b: Pos2, s: Stroke) {
    p.extend(Shape::dashed_line(&[a, b], s, 4.0, 4.0));
}

/// The scale of a histogram: its tallest bin plus a margin (linear, as
/// Photoshop draws it). One scale for before and after keeps them
/// comparable.
pub fn hist_top(h: &[u32; 256]) -> f32 {
    h.iter().copied().max().unwrap_or(1).max(1) as f32 * 1.08
}

/// Filled histogram: one bar per level.
pub fn histogram(p: &Painter, r: Rect, h: &[u32; 256], top: f32, color: Color32) {
    let mut mesh = epaint::Mesh::default();
    let bw = r.width() / 256.0;
    for (i, &v) in h.iter().enumerate() {
        if v == 0 { continue; }
        let x0 = r.left() + i as f32 * bw;
        let y = r.bottom() - (v as f32 / top).min(1.0) * r.height();
        // (A shown bar is at least a hairline: one stray pixel shows.)
        let y = y.min(r.bottom() - 1.0);
        let k = mesh.vertices.len() as u32;
        for q in [pos2(x0, r.bottom()), pos2(x0, y), pos2(x0 + bw, y), pos2(x0 + bw, r.bottom())] {
            mesh.colored_vertex(q, color);
        }
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k, k + 2, k + 3);
    }
    p.add(Shape::mesh(mesh));
}

/// A histogram's outline (the result, over the source's).
pub fn histogram_line(p: &Painter, r: Rect, h: &[u32; 256], top: f32, color: Color32) {
    let bw = r.width() / 256.0;
    let mut pts = Vec::with_capacity(514);
    for (i, &v) in h.iter().enumerate() {
        let y = r.bottom() - (v as f32 / top).min(1.0) * r.height();
        pts.push(pos2(r.left() + i as f32 * bw, y));
        pts.push(pos2(r.left() + (i + 1) as f32 * bw, y));
    }
    p.add(Shape::line(pts, Stroke::new(1.2_f32, color)));
}

/// A tone table (input level → output) as a curve over `r`.
pub fn curve(p: &Painter, r: Rect, t: &[u8; 256], color: Color32, width: f32) {
    let pts: Vec<Pos2> = (0..256).map(|i| pos2(r.left() + (i as f32 + 0.5) / 256.0 * r.width(), r.bottom() - t[i] as f32 / 255.0 * r.height())).collect();
    p.add(Shape::line(pts, Stroke::new(width, color)));
}

/// Horizontal gradient through `stops` (evenly spaced).
pub fn gradient(p: &Painter, r: Rect, stops: &[Color32]) {
    let n = stops.len();
    if n == 0 { return; }
    if n == 1 { p.rect_filled(r, 0.0, stops[0]); return; }
    let mut mesh = epaint::Mesh::default();
    for (i, c) in stops.iter().enumerate() {
        let x = r.left() + r.width() * i as f32 / (n - 1) as f32;
        mesh.colored_vertex(pos2(x, r.top()), *c);
        mesh.colored_vertex(pos2(x, r.bottom()), *c);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A level bar: `n` cells, cell i colored `f(i)` (a tone table seen as
/// grays, a spectrum after the filter).
pub fn cells(p: &Painter, r: Rect, n: usize, f: impl Fn(usize) -> Color32) {
    let mut mesh = epaint::Mesh::default();
    let cw = r.width() / n as f32;
    for i in 0..n {
        let c = f(i);
        let x0 = r.left() + i as f32 * cw;
        let k = mesh.vertices.len() as u32;
        for q in [pos2(x0, r.top()), pos2(x0 + cw + 0.3, r.top()), pos2(x0 + cw + 0.3, r.bottom()), pos2(x0, r.bottom())] {
            mesh.colored_vertex(q, c);
        }
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k, k + 2, k + 3);
    }
    p.add(Shape::mesh(mesh));
}

/// Triangular slider handle pointing up at (x, y).
pub fn handle(p: &Painter, x: f32, y: f32, fill: Color32, active: bool) {
    let stroke = if active { Stroke::new(1.5_f32, theme::ACCENT) } else { Stroke::new(1.0_f32, Color32::from_gray(150)) };
    p.add(Shape::convex_polygon(vec![pos2(x, y), pos2(x + 6.0, y + 10.0), pos2(x - 6.0, y + 10.0)], fill, stroke));
}

/// Which of `xs` is nearest to `x`.
pub fn nearest(xs: &[f32], x: f32) -> Option<usize> {
    xs.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|(i, _)| i)
}

pub fn gray(v: u8) -> Color32 { Color32::from_gray(v) }

pub fn rgb(c: [f32; 3]) -> Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(q(c[0]), q(c[1]), q(c[2]))
}

/// Fully saturated hue (degrees).
pub fn hue(deg: f32) -> Color32 {
    rgb(aurastudio_filters::color::hsl_to_rgb(deg.rem_euclid(360.0) / 360.0, 1.0, 0.5))
}

/// A section caption.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).small().color(theme::TEXT_WEAK));
}

/// A line of information under the controls.
pub fn info(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::TEXT_WEAK));
}

/// Small text in a graph's corner.
pub fn corner(p: &Painter, r: Rect, align: Align2, text: &str) {
    let at = match align {
        Align2::LEFT_TOP => r.left_top() + vec2(5.0, 3.0),
        Align2::RIGHT_TOP => r.right_top() + vec2(-5.0, 3.0),
        Align2::LEFT_BOTTOM => r.left_bottom() + vec2(5.0, -3.0),
        _ => r.right_bottom() + vec2(-5.0, -3.0),
    };
    let g = p.layout_no_wrap(text.to_owned(), FontId::proportional(10.5), theme::TEXT_WEAK);
    let rect = align.anchor_size(at, g.size());
    p.rect_filled(rect.expand(2.0), 2.0, Color32::from_black_alpha(140));
    p.galley(rect.min, g, theme::TEXT_WEAK);
}

/// What a slider's track shows.
pub enum Track<'a> {
    /// The accent, filled from `from` (0 or the neutral value) to the
    /// value.
    Fill(f32),
    /// A gradient (black to white for brightness, a spectrum for hue…).
    Colors(&'a [Color32]),
    /// A checkerboard under `color` fading in: opacity.
    Alpha(Color32),
}

/// What a value slider edits.
pub struct Spec<'a> {
    pub label:   &'a str,
    pub lo:      f32,
    pub hi:      f32,
    /// What the value rounds to.
    pub step:    f32,
    /// What a double-click puts back (and the mark under the track).
    pub default: f32,
    pub unit:    &'a str,
    pub track:   Track<'a>,
    /// Track position = ((v - lo) / (hi - lo))^(1 / pow): 2 gives small
    /// values more room (a blur radius of 0.5 to 250 px).
    pub pow:     f32,
}

impl<'a> Spec<'a> {
    pub fn new(label: &'a str, lo: f32, hi: f32, default: f32, unit: &'a str) -> Self {
        Spec { label, lo, hi, step: 1.0, default, unit, track: Track::Fill(lo.max(0.0).min(hi)), pow: 1.0 }
    }
    pub fn step(self, step: f32) -> Self { Spec { step, ..self } }
    pub fn track(self, track: Track<'a>) -> Self { Spec { track, ..self } }
    pub fn pow(self, pow: f32) -> Self { Spec { pow, ..self } }
}

/// A value slider: its name and the value (typed in its field) above, a
/// track below. Click or drag sets it (Shift: finer), a double-click puts
/// back the default. Returns true when changed.
pub fn slider(ui: &mut Ui, id: &str, spec: Spec, v: &mut f32) -> bool {
    let Spec { label, lo, hi, step, default, unit, track, pow } = spec;
    let before = *v;
    let decimals = if step >= 1.0 { 0 } else if step >= 0.1 { 1 } else { 2 };
    ui.horizontal(|ui| {
        ui.set_width(WIDTH);
        ui.label(RichText::new(label).color(theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !unit.is_empty() { ui.label(RichText::new(unit).color(theme::TEXT_WEAK)); }
            ui.add(egui::DragValue::new(v).range(lo..=hi).speed(((hi - lo) / 400.0).max(step)).fixed_decimals(decimals));
        });
    });
    let id = ui.scope_id().with(("filter_slider", id));
    let (r, resp) = ui.allocate_exact_size(vec2(WIDTH, 18.0), Sense::click_and_drag());
    let bar = Rect::from_min_max(pos2(r.left() + 7.0, r.top() + 2.0), pos2(r.right() - 7.0, r.top() + 9.0));
    let to_t = |v: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0).powf(1.0 / pow);
    let to_x = |v: f32| bar.left() + to_t(v) * bar.width();
    if resp.double_clicked() {
        *v = default;
    } else if resp.is_pointer_button_down_on() || resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let fine = ui.input(|i| i.modifiers.shift);
            // Shift: the handle moves a tenth as far as the pointer.
            let last: Option<f32> = ui.data(|d| d.get_temp(id));
            let x = match (fine, last) { (true, Some(l)) => to_x(*v) + (pos.x - l) * 0.1, _ => pos.x };
            ui.data_mut(|d| d.insert_temp(id, pos.x));
            let t = ((x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            *v = ((lo + t.powf(pow) * (hi - lo)) / step).round() * step;
        }
    } else {
        ui.data_mut(|d| d.remove::<f32>(id));
    }
    *v = v.clamp(lo, hi);
    let p = ui.painter_at(r.expand(2.0));
    let mark = !matches!(track, Track::Fill(f) if f == default);
    match track {
        Track::Fill(from) => {
            p.rect_filled(bar, 2.0, theme::WIDGET);
            let (a, b) = (to_x(from.clamp(lo, hi)), to_x(*v));
            p.rect_filled(Rect::from_x_y_ranges(a.min(b)..=a.max(b), bar.y_range()), 2.0, theme::ACCENT);
        }
        Track::Colors(stops) => gradient(&p, bar, stops),
        Track::Alpha(c) => {
            crate::widgets::checker(&p, bar, 3.5);
            gradient(&p, bar, &[c.gamma_multiply(0.0), c]);
        }
    }
    p.rect_stroke(bar, 2.0, Stroke::new(1.0_f32, theme::BORDER), egui::StrokeKind::Middle);
    if mark {
        // The default's mark.
        let x = to_x(default);
        p.line_segment([pos2(x, bar.top() - 2.0), pos2(x, bar.top())], Stroke::new(1.0_f32, theme::TEXT_WEAK));
    }
    let active = resp.hovered() || resp.dragged();
    handle(&p, to_x(*v), bar.bottom() - 2.0, if active { theme::TEXT } else { Color32::from_gray(210) }, resp.dragged());
    resp.on_hover_text("Double-click: reset");
    *v != before
}

/// An angle dial: drag around it to turn the needle (0° to the right,
/// counterclockwise, as Photoshop's light angle). Returns true when
/// changed.
pub fn angle(ui: &mut Ui, label: &str, deg: &mut f32, default: f32) -> bool {
    let before = *deg;
    ui.horizontal(|ui| {
        ui.set_width(WIDTH);
        let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click_and_drag());
        let c = r.center();
        if resp.double_clicked() {
            *deg = default;
        } else if let (true, Some(pos)) = (resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
            let d = pos - c;
            if d.length() > 2.0 {
                let mut a = (-d.y).atan2(d.x).to_degrees().round();
                // Shift: 15° steps.
                if ui.input(|i| i.modifiers.shift) { a = (a / 15.0).round() * 15.0; }
                *deg = if a > 180.0 { a - 360.0 } else { a };
            }
        }
        let p = ui.painter_at(r.expand(1.0));
        p.circle(c, 18.0, FIELD, Stroke::new(1.0_f32, if resp.hovered() { theme::TEXT_WEAK } else { theme::BORDER }));
        let a = deg.to_radians();
        let tip = c + vec2(a.cos(), -a.sin()) * 15.0;
        p.line_segment([c, tip], Stroke::new(1.6_f32, theme::TEXT));
        p.circle_filled(tip, 3.0, theme::ACCENT);
        p.circle_filled(c, 2.0, theme::TEXT_WEAK);
        resp.on_hover_text("Double-click: reset");
        ui.label(RichText::new(label).color(theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("°").color(theme::TEXT_WEAK));
            ui.add(egui::DragValue::new(deg).range(-180.0..=180.0).speed(1.0).fixed_decimals(0));
        });
    });
    *deg != before
}

/// Pick one of `names` (a row of toggle buttons). Returns true when
/// changed.
pub fn choice(ui: &mut Ui, value: &mut usize, names: &[&str]) -> bool {
    let before = *value;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding.x = 4.0;
        for (i, n) in names.iter().enumerate() {
            if ui.selectable_label(*value == i, RichText::new(*n).size(12.5)).clicked() { *value = i; }
        }
    });
    *value != before
}

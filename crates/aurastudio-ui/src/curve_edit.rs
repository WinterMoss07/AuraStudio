//! A response curve to edit (CSP's pen pressure settings, Krita's tablet
//! curve): input across, output up, drawn through its points. Drag a
//! point; click elsewhere to add one; double-click a middle point to take
//! it out. The ends stay at input 0 and 1.

use aurastudio_forge::ResponseCurve;
use egui::{pos2, vec2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui};

use crate::theme;

/// Pixels from a point where the pointer takes it.
const GRAB: f32 = 7.0;

/// The curve through `points` in a `side` square; `x_label`: what the
/// input is. True when the points changed.
pub fn curve_editor(ui: &mut Ui, id_salt: &str, points: &mut Vec<[f32; 2]>, side: f32, x_label: &str) -> bool {
    if points.len() < 2 { *points = vec![[0.0, 0.0], [1.0, 1.0]]; }
    let id = ui.make_persistent_id(("curve_editor", id_salt));
    let (outer, resp) = ui.allocate_exact_size(vec2(side, side + 16.0), Sense::click_and_drag());
    let rect = Rect::from_min_size(outer.min, vec2(side, side)).shrink(4.0);
    let to_screen = |p: [f32; 2]| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height());
    let to_curve = |p: Pos2| [((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0), ((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0)];
    let nearest = |pos: Pos2, pts: &[[f32; 2]]| pts.iter().enumerate().map(|(i, p)| (i, to_screen(*p).distance(pos))).filter(|(_, d)| *d <= GRAB).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    let mut changed = false;
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(id));

    if resp.double_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(|p| nearest(p, points)) {
            if i > 0 && i + 1 < points.len() { points.remove(i); changed = true; }
        }
        dragging = None;
    } else if resp.drag_started() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            dragging = match nearest(pos, points) {
                Some(i) => Some(i),
                // A new point where clicked, between its neighbors.
                None if points.len() < ResponseCurve::MAX_POINTS => {
                    let c = to_curve(pos);
                    let at = points.iter().position(|p| p[0] > c[0]).unwrap_or(points.len() - 1).max(1);
                    points.insert(at, c);
                    changed = true;
                    Some(at)
                }
                None => None,
            };
        }
    }
    if let (Some(i), Some(pos)) = (dragging, resp.interact_pointer_pos()) {
        if resp.dragged() && i < points.len() {
            let mut c = to_curve(pos);
            let last = points.len() - 1;
            // The ends keep their input; the others stay between neighbors.
            c[0] = if i == 0 { 0.0 } else if i == last { 1.0 } else { c[0].clamp(points[i - 1][0] + 0.01, points[i + 1][0] - 0.01) };
            if points[i] != c { points[i] = c; changed = true; }
        }
    }
    if !resp.dragged() && !resp.drag_started() { dragging = None; }
    ui.data_mut(|d| match dragging { Some(i) => { d.insert_temp(id, i); } None => { d.remove::<usize>(id); } });

    // Grid, curve, points.
    let p = ui.painter_at(outer);
    p.rect_filled(rect, 2.0, theme::SURFACE);
    let grid = Stroke::new(1.0, Color32::from_gray(0x44));
    for k in 1..4 {
        let t = k as f32 / 4.0;
        p.line_segment([pos2(rect.left() + t * rect.width(), rect.top()), pos2(rect.left() + t * rect.width(), rect.bottom())], grid);
        p.line_segment([pos2(rect.left(), rect.top() + t * rect.height()), pos2(rect.right(), rect.top() + t * rect.height())], grid);
    }
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Middle);
    let curve = ResponseCurve::new(points);
    let line: Vec<Pos2> = (0..=48).map(|k| { let x = k as f32 / 48.0; to_screen([x, curve.eval(x)]) }).collect();
    p.add(Shape::line(line, Stroke::new(1.5, theme::TEXT)));
    let hover = resp.hover_pos().and_then(|h| nearest(h, points));
    for (i, q) in points.iter().enumerate() {
        let lit = dragging == Some(i) || hover == Some(i);
        p.rect_filled(Rect::from_center_size(to_screen(*q), vec2(7.0, 7.0)), 1.0, if lit { theme::TEXT } else { theme::ACCENT });
    }
    let font = egui::FontId::proportional(10.5);
    p.text(pos2(rect.center().x, outer.bottom() - 2.0), egui::Align2::CENTER_BOTTOM, x_label, font, theme::TEXT_WEAK);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drawn without input: the points are left as they are; fewer than
    /// two become the straight line.
    #[test]
    fn a_curve_is_shown_unchanged() {
        let ctx = egui::Context::default();
        let mut pts = vec![[0.0, 0.0], [0.4, 0.7], [1.0, 1.0]];
        let mut none = Vec::new();
        crate::test_pass(&ctx, egui::RawInput::default(), |ui| {
            assert!(!curve_editor(ui, "a", &mut pts, 120.0, "Pen pressure"));
            curve_editor(ui, "b", &mut none, 120.0, "Tilt");
        });
        assert_eq!(pts, vec![[0.0, 0.0], [0.4, 0.7], [1.0, 1.0]]);
        assert_eq!(none, vec![[0.0, 0.0], [1.0, 1.0]]);
    }
}

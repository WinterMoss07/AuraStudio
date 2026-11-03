//! The tools' presses, drags and releases on the canvas: selections, the
//! polygon, the fill, the picker, the geometry tools and the arc; the
//! toolbox and the brushes.

use aurastudio_core::Point;
use aurastudio_document::{Edit, SelectMode, SelectOp, Shape, Wand};
use aurastudio_forge::PenSample;
use aurastudio_input::{Device, Event};
use aurastudio_layers::FillParams;
use aurastudio_tools::arc::Ctx as ArcCtx;
use aurastudio_tools::shapes;
use aurastudio_ui::{Group, SelectCmd, Tool};

use super::{pixel_in, App, Gesture, Preview};
use super::pointer::pt;
use crate::paint_thread::Cmd;

/// The larger of the two distances on screen between two canvas points.
fn screen_gap(a: Point, b: Point, zoom: f64) -> f64 { (a.x - b.x).abs().max((a.y - b.y).abs()) * zoom }

impl App {
    /// Alt held with the brush or the fill: a click picks a color (Krita,
    /// CSP, Photoshop).
    pub(super) fn picking_with_alt(&self) -> bool {
        self.modifiers.alt_key() && matches!(self.tool, Tool::Brush | Tool::Eraser | Tool::Fill)
    }

    /// The loupe shows around the cursor (picker or selection tools with
    /// it on): a pixel taken (picker, wand) or a pixel corner aimed at.
    pub(super) fn loupe_on(&self) -> Option<bool> {
        if !self.over_canvas || !self.cursor_in_window || self.home || self.space_down || self.nav.is_some() || self.is_drawing { return None; }
        if matches!(self.gesture, Some(Gesture::MoveSel { .. })) { return None; }
        match self.tool {
            Tool::Picker if self.picker.loupe => Some(true),
            Tool::MagicWand if self.select.loupe => Some(true),
            Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect | Tool::PolySelect if self.select.loupe => {
                // Over the selection, a press moves it: no loupe.
                if self.gesture.is_none() && self.poly.is_none() && self.canvas_at().is_some_and(|p| self.on_selection(p)) { return None; }
                Some(false)
            }
            _ => None,
        }
    }

    /// The canvas point a selection tool takes: on the pixel corner
    /// nearest the cursor with the loupe on.
    pub(super) fn select_at(&self) -> Option<Point> {
        let p = self.canvas_at()?;
        Some(if self.select.loupe { Point::new(p.x.round(), p.y.round()) } else { p })
    }

    /// The selection mode's sign by the cursor.
    pub(super) fn select_mark(&self) -> crate::cursors::Mark {
        use crate::cursors::Mark;
        let mode = match &self.gesture {
            Some(Gesture::Box { mode, .. } | Gesture::Lasso { mode, .. }) => *mode,
            _ => match &self.poly { Some((_, m)) => *m, None => self.select_mode() },
        };
        match mode {
            SelectMode::Replace => Mark::None,
            SelectMode::Add => Mark::Add,
            SelectMode::Subtract => Mark::Subtract,
            SelectMode::Intersect => Mark::Intersect,
        }
    }

    /// The selection mode the keys held ask for (Shift: add, Alt:
    /// subtract, both: intersect), else the options'.
    fn select_mode(&self) -> SelectMode {
        match (self.shift_held(), self.modifiers.alt_key()) {
            (true, true) => SelectMode::Intersect,
            (true, false) => SelectMode::Add,
            (false, true) => SelectMode::Subtract,
            _ => self.select.mode,
        }
    }

    /// The press at canvas point `p` is on the selection, with nothing held
    /// that changes the mode: it drags the outline.
    pub(super) fn on_selection(&self, p: Point) -> bool {
        if self.select_mode() != SelectMode::Replace || self.select.mode != SelectMode::Replace { return false; }
        let Ok(d) = self.doc.try_lock() else { return false };
        d.selection().is_some_and(|s| s.inside(p.x.floor() as i32, p.y.floor() as i32))
    }

    /// Press on the canvas: what the current tool does.
    pub(super) fn tool_press(&mut self, e: Event) {
        let tool = if self.picking_with_alt() { Tool::Picker } else { self.tool };
        let at = self.canvas_at();
        match tool {
            Tool::Line | Tool::Rectangle | Tool::Ellipse => {
                self.gesture = at.map(|p| Gesture::Shape { tool, start: p, end: p });
            }
            Tool::Arc => if let Some(p) = at { self.arc_press(p) },
            // The pen turned over erases with any brush.
            Tool::Brush | Tool::Eraser => self.begin_stroke(tool == Tool::Eraser || e.device == Device::Eraser),
            Tool::Picker => {
                self.gesture = Some(Gesture::Pick);
                self.pick_color();
            }
            Tool::Fill => {
                let Some((x, y)) = at.and_then(|p| pixel_in(p, self.canvas_size())) else { return };
                let [r, g, b] = self.brush.color;
                self.send_edit(Edit::Fill { x, y, color: [r, g, b, 255], params: self.fill });
            }
            Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect => {
                let Some(p) = self.select_at() else { return };
                let (mode, held) = (self.select_mode(), (self.shift_held(), self.modifiers.alt_key()));
                self.gesture = Some(if self.on_selection(p) {
                    Gesture::MoveSel { start: p, now: p }
                } else if tool == Tool::Lasso {
                    Gesture::Lasso { pts: vec![p], mode }
                } else {
                    Gesture::Box { ellipse: tool == Tool::EllipseSelect, a: p, b: p, mode, held }
                });
            }
            Tool::PolySelect => if let Some(p) = self.select_at() { self.poly_click(p) },
            Tool::MagicWand => {
                let Some((x, y)) = at.and_then(|p| pixel_in(p, self.canvas_size())) else { return };
                let o = self.select.wand;
                let wand = Wand {
                    fill: FillParams { tolerance: o.tolerance, refer: o.refer, skip_drafts: o.skip_drafts, expand: o.grow },
                    contiguous: o.contiguous,
                };
                let op = SelectOp::Wand { x, y, wand, mode: self.select_mode(), aa: self.select.aa, feather: self.select.feather };
                self.send_select(op);
            }
            Tool::Transform => if let Some(p) = at { self.xform_press(p) },
        }
        self.update_cursor();
        self.request_redraw();
    }

    /// Cursor moved during a tool drag.
    pub(super) fn tool_drag(&mut self) {
        let Some(raw) = self.canvas_at() else { return };
        let zoom = self.zoom() as f64;
        let snapped = self.select_at().unwrap_or(raw);
        let at = if matches!(self.gesture, Some(Gesture::Lasso { .. } | Gesture::Box { .. })) { snapped } else { raw };
        match &mut self.gesture {
            Some(Gesture::Pick) => self.pick_color(),
            Some(Gesture::Lasso { pts, .. }) => {
                // One point per screen pixel or so of travel.
                let last = pts.last().copied().unwrap_or(at);
                if screen_gap(at, last, zoom) >= 1.5 { pts.push(at); }
            }
            Some(Gesture::Box { b: end, .. } | Gesture::Shape { end, .. } | Gesture::MoveSel { now: end, .. }) => *end = at,
            None => return,
        }
        self.request_redraw();
    }

    /// The box a rectangle / ellipse selection covers: Shift pressed after
    /// the press makes it square, Alt centers it.
    fn select_box(&self, a: Point, b: Point, held: (bool, bool)) -> (Point, Point) {
        let square = self.shift_held() && !held.0;
        let centered = self.modifiers.alt_key() && !held.1;
        shapes::drag_box(a, b, square, centered)
    }

    /// What the selection gesture outlines now, canvas pixels.
    pub(super) fn gesture_outline(&self) -> Option<Vec<(f32, f32)>> {
        if self.gesture.is_none() && self.poly.is_some() {
            // The polygon so far, and on to the cursor.
            let (pts, _) = self.poly.as_ref()?;
            let mut v: Vec<(f32, f32)> = pts.iter().copied().map(pt).collect();
            if let Some(c) = self.select_at() { v.push(pt(c)); }
            return Some(v);
        }
        match self.gesture.as_ref()? {
            Gesture::Lasso { pts, .. } => Some(pts.iter().copied().map(pt).collect()),
            Gesture::Box { ellipse, a, b, held, .. } => {
                let (p, q) = self.select_box(*a, *b, *held);
                let v = if *ellipse { shapes::ellipse(p, q) } else { vec![p, Point::new(q.x, p.y), q, Point::new(p.x, q.y)] };
                Some(v.into_iter().map(pt).collect())
            }
            _ => None,
        }
    }

    /// Release: a selection drawn becomes the selection; a click without
    /// dragging deselects (Krita, Photoshop).
    pub(super) fn tool_release(&mut self) {
        let Some(g) = self.gesture.take() else { return };
        let zoom = self.zoom() as f64;
        let far = |a: Point, b: Point, min: f64| screen_gap(a, b, zoom) >= min;
        let (aa, feather) = (self.select.aa, self.select.feather);
        let op = match g {
            Gesture::Pick => { self.update_cursor(); return; }
            Gesture::MoveSel { start, now } => {
                let (dx, dy) = ((now.x - start.x).round() as i32, (now.y - start.y).round() as i32);
                if dx == 0 && dy == 0 {
                    // A click on the selection: deselect.
                    if !far(start, now, 3.0) { Some(SelectOp::None) } else { None }
                } else {
                    Some(SelectOp::Move(dx, dy))
                }
            }
            Gesture::Lasso { pts, mode } => {
                let moved = pts.iter().any(|p| far(*p, pts[0], 3.0));
                if moved && pts.len() >= 3 { Some(SelectOp::Shape { shape: Shape::Polygon(pts), mode, aa, feather }) }
                else if mode == SelectMode::Replace { Some(SelectOp::None) } else { None }
            }
            Gesture::Box { ellipse, a, b, mode, held } => {
                let (p, q) = self.select_box(a, b, held);
                if !far(p, q, 3.0) {
                    if mode == SelectMode::Replace { Some(SelectOp::None) } else { None }
                } else if ellipse {
                    Some(SelectOp::Shape { shape: Shape::Ellipse([p.x, p.y, q.x, q.y]), mode, aa, feather })
                } else {
                    let r = |v: f64| v.round() as i32;
                    let rect = aurastudio_core::Rect::new(r(p.x.min(q.x)), r(p.y.min(q.y)), r(p.x.max(q.x)), r(p.y.max(q.y)));
                    Some(SelectOp::Shape { shape: Shape::Rect(rect), mode, aa, feather })
                }
            }
            Gesture::Shape { tool, start, end } => {
                // A click without dragging draws nothing.
                if far(start, end, 2.0) {
                    let path = self.shape_path(tool, start, end);
                    self.paint_path(path);
                }
                self.update_cursor();
                self.request_redraw();
                return;
            }
        };
        if let Some(op) = op { self.send_select(op); }
        self.update_cursor();
        self.request_redraw();
    }

    /// Polygon selection: a click adds a point; on the first point (or a
    /// double click) it closes.
    fn poly_click(&mut self, p: Point) {
        let zoom = self.zoom() as f64;
        match &mut self.poly {
            None => {
                let mode = self.select_mode();
                self.poly = Some((vec![p], mode));
            }
            Some((pts, _)) => {
                let close = |a: Point, b: Point| (a.x - b.x).hypot(a.y - b.y) * zoom <= 8.0;
                if pts.len() >= 3 && close(p, pts[0]) { self.finish_poly(); return; }
                // A second click on the last point: done.
                if pts.last().is_some_and(|l| close(p, *l)) { if pts.len() >= 3 { self.finish_poly(); } return; }
                pts.push(p);
            }
        }
        self.request_redraw();
    }

    pub(super) fn finish_poly(&mut self) {
        let Some((pts, mode)) = self.poly.take() else { return };
        if pts.len() >= 3 {
            let op = SelectOp::Shape { shape: Shape::Polygon(pts), mode, aa: self.select.aa, feather: self.select.feather };
            self.send_select(op);
        }
        self.request_redraw();
    }

    fn send_select(&mut self, op: SelectOp) {
        self.send_edit(Edit::Select(op));
        self.request_redraw();
    }

    /// Select menu and keys.
    pub(super) fn select(&mut self, cmd: SelectCmd) {
        if self.home || self.is_drawing { return; }
        if self.xf.is_some() { self.end_transform(true); }
        let op = match cmd {
            SelectCmd::All => SelectOp::All,
            SelectCmd::None => SelectOp::None,
            SelectCmd::Reselect => SelectOp::Reselect,
            SelectCmd::Invert => SelectOp::Invert,
            SelectCmd::Grow(r) => SelectOp::Grow(r),
            SelectCmd::Shrink(r) => SelectOp::Shrink(r),
            SelectCmd::Feather(r) => SelectOp::Feather(r),
            SelectCmd::Border(r) => SelectOp::Border(r),
            SelectCmd::FromLayer => SelectOp::FromLayer(self.select.mode),
        };
        self.send_select(op);
    }

    /// Path of a line / rectangle / ellipse dragged from `start` to `end`,
    /// with the modifiers held (Shift: 15° steps, square, circle; Alt:
    /// from the center).
    fn shape_path(&self, tool: Tool, start: Point, end: Point) -> Vec<Point> {
        let (shift, alt) = (self.shift_held(), self.modifiers.alt_key());
        match tool {
            Tool::Line => shapes::line(start, if shift { shapes::snap_angle(start, end) } else { end }),
            Tool::Rectangle => { let (a, b) = shapes::drag_box(start, end, shift, alt); shapes::rectangle(a, b) }
            Tool::Ellipse => { let (a, b) = shapes::drag_box(start, end, shift, alt); shapes::ellipse(a, b) }
            _ => Vec::new(),
        }
    }

    /// Geometry tool preview: the path, its handles and control lines.
    pub(super) fn shape_preview(&self) -> Option<Preview> {
        let v = |p: &[Point]| p.iter().copied().map(pt).collect::<Vec<_>>();
        if let Some(Gesture::Shape { tool, start, end }) = &self.gesture {
            return Some((v(&self.shape_path(*tool, *start, *end)), Vec::new(), Vec::new()));
        }
        let a = self.arc.preview()?;
        let mut path = v(a.chain);
        path.extend(v(&a.current));
        let mut handles = vec![pt(a.start), pt(a.end)];
        if let Some(f) = a.first { handles.push(pt(f)); }
        let guide = a.guide.map_or_else(Vec::new, |g| v(&g));
        Some((path, handles, guide))
    }

    fn arc_ctx(&self) -> ArcCtx {
        ArcCtx { zoom: self.zoom() as f64, shift: self.shift_held(), multipoint: self.shape.multipoint }
    }

    fn arc_press(&mut self, at: Point) {
        let cx = self.arc_ctx();
        if let Some(path) = self.arc.press(at, cx) { self.paint_path(path); }
        self.request_redraw();
    }

    pub(super) fn arc_release(&mut self, at: Point) {
        let cx = self.arc_ctx();
        self.arc.release(at, cx);
        self.request_redraw();
    }

    /// Cursor moved (button up or down) with an arc on the way.
    pub(super) fn arc_move(&mut self, at: Point) {
        let cx = self.arc_ctx();
        if self.arc.moved(at, cx) { self.request_redraw(); }
    }

    /// The outline is done (Enter, leaving the tool...): paint it.
    pub(super) fn finish_arcs(&mut self) {
        if let Some(path) = self.arc.finish() { self.paint_path(path); }
        self.request_redraw();
    }

    /// Escape: drop the outline in progress, nothing painted.
    pub(super) fn cancel_arcs(&mut self) {
        self.arc.cancel();
        self.request_redraw();
    }

    /// Ctrl+Z while building an outline: take back its last arc.
    pub(super) fn undo_arc(&mut self) {
        let at = self.canvas_at().unwrap_or(Point::new(0.0, 0.0));
        self.arc.undo(at);
        self.request_redraw();
    }

    /// Paint a geometry tool's path as one stroke with the current brush
    /// (full pressure, an even pen speed for speed-sensitive brushes).
    fn paint_path(&mut self, path: Vec<Point>) {
        let Some(&first) = path.first() else { return };
        if let Some(msg) = self.paint_refusal() {
            self.ui.notify(msg);
            self.request_redraw();
            return;
        }
        self.send_begin(false);
        let mut time = aurastudio_input::clock::now().0 as f64 / 1e6;
        let mut last = first;
        for p in path {
            // 2000 canvas px per second.
            time += ((p.x - last.x).hypot(p.y - last.y) / 2000.0).max(0.0005);
            last = p;
            self.paint.send(Cmd::Sample(PenSample::new(p.x as f32, p.y as f32, 1.0, time)));
        }
        self.paint.send(Cmd::End);
    }

    /// Why the active layer can't be painted (paint thread busy: don't
    /// wait, it checks too).
    pub(super) fn paint_refusal(&self) -> Option<&'static str> {
        match self.doc.try_lock() {
            Ok(doc) if doc.paint_id().is_some() => None,
            Ok(doc) if doc.stack().active_node().is_some_and(|n| n.is_folder()) => Some("Select a layer to paint (a folder is selected)"),
            Ok(_) => Some("Layer is locked"),
            Err(_) => None,
        }
    }

    /// Color picker: the merged image's color under the cursor becomes the
    /// brush color.
    pub(super) fn pick_color(&mut self) {
        let Some(p) = self.canvas_at() else { return };
        // Never wait for a stroke being painted: skip this sample.
        let Ok(d) = self.doc.try_lock() else { return };
        let Some((x, y)) = pixel_in(p, (d.stack().width, d.stack().height)) else { return };
        let px = d.merged(aurastudio_core::Rect::new(x, y, x + 1, y + 1), aurastudio_layers::Pick::ALL);
        drop(d);
        let Some(px) = px.first() else { return };
        if self.brush.color != [px[0], px[1], px[2]] {
            self.brush.color = [px[0], px[1], px[2]];
            self.request_redraw();
        }
    }

    /// A toolbox group's shortcut: its last tool and brush (pressed again:
    /// its next tool).
    pub(super) fn select_group(&mut self, g: Group) {
        if self.is_drawing || self.gesture.is_some() || self.nav.is_some() { return; }
        let (mut tool, mut preset) = (self.tool, self.preset);
        self.tool_memory.select(g, &mut tool, &mut preset, &self.presets);
        self.select_preset(preset);
        self.set_tool(tool);
        self.tool_memory.remember(self.tool, self.preset, &self.presets);
    }

    pub(super) fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool || self.is_drawing || self.gesture.is_some() || self.nav.is_some() { return; }
        if self.xf.as_ref().is_some_and(|x| x.drag.is_some()) { return; }
        self.finish_arcs();
        self.arc.reset_release();
        // Leaving the transform keeps it (Krita); a polygon not closed is
        // dropped.
        if self.tool == Tool::Transform { self.end_transform(true); }
        self.poly = None;
        self.tool = tool;
        if tool == Tool::Transform { self.start_transform(); }
        self.update_cursor();
        self.request_redraw();
    }

    /// Switch brush: the current one keeps the settings it has now (each
    /// brush has its own size, opacity, blend...), the new one brings its
    /// own. The color stays.
    pub(super) fn select_preset(&mut self, index: usize) {
        if index == self.preset || index >= self.presets.len() { return; }
        let color = self.brush.color;
        self.presets[self.preset].settings = self.brush.clone();
        self.brush = self.presets[index].settings.clone();
        self.brush.color = color;
        self.preset = index;
    }
}

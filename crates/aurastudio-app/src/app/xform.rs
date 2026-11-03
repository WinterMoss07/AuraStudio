//! The transform tool: its box and handles here, the pixels on the paint
//! thread (the document's transform session), a live preview on the GPU.

use std::sync::Arc;

use aurastudio_core::Point;
use aurastudio_document::{Edit, Interp};
use aurastudio_tools::xform::{Hit, Mods, Polylines, Xform};
use aurastudio_ui::{Tool, TransformCmd, TransformMode};
use winit::window::CursorIcon;

use super::{lift_mesh, App, XformDrawing};
use super::pointer::pt;
use crate::paint_thread::Cmd;

impl App {
    /// The layers the transform tool takes: those chosen in the Layers
    /// docker, the paint layers of a chosen folder.
    fn transform_targets(&self) -> Vec<u64> {
        let Ok(d) = self.doc.try_lock() else { return Vec::new() };
        let mut out = Vec::new();
        for id in self.ui.chosen_layers(d.stack()) {
            if let Some(n) = d.stack().node(id) { n.push_layer_ids(&mut out); }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Ask the paint thread to start a transform (its box shows once it
    /// answers).
    pub(super) fn start_transform(&mut self) {
        if self.home || self.xf.is_some() || self.xf_pending { return; }
        let targets = self.transform_targets();
        if targets.is_empty() {
            self.ui.notify("Select a layer to transform");
            self.request_redraw();
            return;
        }
        self.xf_pending = true;
        self.xf_targets = targets.clone();
        self.send_edit(Edit::TransformBegin(targets));
    }

    /// The paint thread began the transform: its box, or why not.
    pub(super) fn transform_started(&mut self, r: Result<aurastudio_core::Rect, &'static str>) {
        let wanted = std::mem::take(&mut self.xf_pending) && self.tool == Tool::Transform;
        match r {
            Ok(src) if wanted => {
                let mut xf = Xform::new(src);
                if self.transform_opts.mode == TransformMode::Warp { xf.set_grid(self.transform_opts.grid); xf.undo.clear(); }
                self.xf = Some(xf);
            }
            // The tool was left meanwhile.
            Ok(_) => self.send_edit(Edit::TransformCancel),
            Err(msg) => self.ui.notify(msg),
        }
        self.update_cursor();
        self.request_redraw();
    }

    /// Its live preview's pieces: to the GPU.
    pub(super) fn transform_lift(&mut self, l: &aurastudio_document::Lift) {
        let Some(gpu) = &mut self.gpu else { return };
        if !self.xf.as_ref().is_some_and(|x| x.src == l.src) { return; }
        let layers: Vec<(&[aurastudio_core::Rgba8], f32)> = l.layers.iter().map(|x| (&x.premul[..], x.opacity)).collect();
        self.lift = gpu.lift_begin(aurastudio_gpu::Pieces { src: l.src, below: &l.below, above: l.above.as_deref(), layers: &layers });
        if self.lift.is_none() { log::warn!("transform: no live preview (sizes)"); }
        // Changed before it came (Warp mode's grid...): show it.
        if self.xf.as_ref().is_some_and(Xform::changed) { self.lift_show(); self.request_redraw(); }
    }

    /// Keep (or drop) the transform under way.
    pub(super) fn end_transform(&mut self, apply: bool) {
        self.xf_pending = false;
        let Some(xf) = self.xf.take() else { return };
        // (The canvas keeps the GPU's last picture until the result comes.)
        self.lift = None;
        let edit = if apply && xf.changed() { Edit::TransformApply { warp: xf.warp, interp: self.transform_opts.interp } } else { Edit::TransformCancel };
        self.send_edit(edit);
        self.update_cursor();
        self.request_redraw();
    }

    /// Show the transform as it is now: drawn by the GPU at once; `fast`
    /// (a handle dragged): only that, else the paint thread's exact result
    /// follows (bilinear while dragging when the GPU can't).
    pub(super) fn send_preview(&mut self, fast: bool) {
        let on_gpu = self.lift_show();
        self.xf_seq += 1;
        self.request_redraw();
        if fast && on_gpu { return; }
        let Some(xf) = &self.xf else { return };
        let interp = match self.transform_opts.interp { Interp::Nearest => Interp::Nearest, i if !fast => i, _ => Interp::Bilinear };
        self.paint.send(Cmd::Edit { doc: Arc::clone(&self.doc), edit: Edit::TransformPreview { warp: xf.warp.clone(), interp }, seq: Some(self.xf_seq) });
    }

    /// Draw the transform on the GPU (lift.rs), drawn over the canvas
    /// until the paint thread's matching preview is in. False: not ready.
    fn lift_show(&mut self) -> bool {
        let (Some(xf), Some(gpu), Some(lift)) = (&self.xf, &self.gpu, &mut self.lift) else { return false };
        if lift.src != xf.src { return false; }
        let Some(verts) = lift_mesh(&xf.warp, xf.src) else { return false };
        gpu.lift_show(lift, &verts, self.transform_opts.interp == Interp::Nearest, true);
        true
    }

    /// Handles' reach, canvas pixels.
    fn handle_tolerance(&self) -> f64 { 9.0 / self.zoom().max(1e-3) as f64 }

    pub(super) fn xform_press(&mut self, p: Point) {
        let mode = self.transform_opts.mode;
        let tol = self.handle_tolerance();
        let Some(xf) = &mut self.xf else {
            // None yet (empty layer, cancelled): try again.
            self.start_transform();
            return;
        };
        if mode == TransformMode::Warp && xf.warp.mesh.is_none() { xf.set_grid(self.transform_opts.grid); }
        let hit = xf.hit(p, tol, mode);
        xf.press(hit, p);
    }

    fn xform_mods(&self) -> Mods {
        Mods { shift: self.shift_held(), alt: self.modifiers.alt_key(), ctrl: self.ctrl_held() }
    }

    /// The pointer moved with the transform's handle held.
    pub(super) fn xform_drag(&mut self) {
        let Some(at) = self.canvas_at() else { return };
        let (m, mode, keep) = (self.xform_mods(), self.transform_opts.mode, self.transform_opts.keep_aspect);
        let Some(xf) = &mut self.xf else { return };
        let Some(hit) = xf.drag.as_ref().map(|d| d.hit) else { return };
        if !xf.drag_to(at, m, mode, keep) { return; }
        // The pivot moved: the picture didn't, only the box is redrawn.
        if hit == Hit::Pivot { self.request_redraw(); } else { self.send_preview(true); }
    }

    pub(super) fn xform_release(&mut self) {
        let Some(xf) = &mut self.xf else { return };
        let pivot = xf.drag.as_ref().is_some_and(|d| d.hit == Hit::Pivot);
        if xf.release() && !pivot { self.send_preview(false); }
        self.update_cursor();
    }

    /// Other layers chosen while transforming: the transform so far is
    /// kept, a new one starts on them.
    pub(super) fn follow_transform_targets(&mut self) {
        if self.tool != Tool::Transform || self.is_drawing || self.xf.as_ref().is_some_and(|x| x.drag.is_some()) { return; }
        if self.xf.is_none() && !self.xf_pending { return; }
        let targets = self.transform_targets();
        if !targets.is_empty() && targets != self.xf_targets {
            self.end_transform(true);
            self.start_transform();
        }
    }

    /// The options' buttons and fields.
    pub(super) fn transform_cmd(&mut self, cmd: TransformCmd) {
        match cmd {
            TransformCmd::Apply => return self.end_transform(true),
            TransformCmd::Cancel => return self.end_transform(false),
            _ => {}
        }
        let Some(xf) = &mut self.xf else { return };
        match cmd {
            TransformCmd::Reset => xf.reset(),
            TransformCmd::FlipH => xf.flip(true),
            TransformCmd::FlipV => xf.flip(false),
            TransformCmd::Rotate(d) => xf.rotate(d),
            TransformCmd::Numbers(n) => xf.set_numbers(n),
            TransformCmd::ResetWarp => { xf.remember(); xf.warp.mesh = None; }
            TransformCmd::Grid(n) => { self.transform_opts.grid = n; xf.set_grid(n); }
            TransformCmd::Apply | TransformCmd::Cancel => {}
        }
        self.send_preview(false);
    }

    /// The box and its handles, canvas pixels.
    pub(super) fn transform_overlay_data(&self) -> Option<XformDrawing> {
        let xf = self.xf.as_ref()?;
        let mode = self.transform_opts.mode;
        let mut polys = Polylines::default();
        xf.lines(mode, &mut polys);
        let lines = polys.iter().map(|l| l.iter().copied().map(pt).collect()).collect();
        // A small box on screen shows its corners only (see Xform::hit).
        let q = xf.warp.quad;
        let small = (0..4).map(|i| (q[(i + 1) % 4].x - q[i].x).hypot(q[(i + 1) % 4].y - q[i].y)).fold(f64::MAX, f64::min) < self.handle_tolerance() * 5.0;
        let (handles, points) = match mode {
            TransformMode::Warp => (Vec::new(), xf.mesh_points().into_iter().map(pt).collect()),
            _ if small => (q.iter().copied().map(pt).collect(), Vec::new()),
            _ => (q.iter().copied().chain(xf.edge_handles()).map(pt).collect(), Vec::new()),
        };
        let pivot = (mode == TransformMode::Free && !small).then(|| pt(xf.pivot));
        Some((lines, handles, points, pivot))
    }

    /// The cursor over the transform box: what a press there would do.
    pub(super) fn transform_cursor(&self) -> Option<crate::cursors::Wanted> {
        use crate::cursors::Wanted;
        let xf = self.xf.as_ref()?;
        let at = self.canvas_at()?;
        let mode = self.transform_opts.mode;
        let hit = match &xf.drag { Some(d) => d.hit, None => xf.hit(at, self.handle_tolerance(), mode) };
        let dragging = xf.drag.is_some();
        // Resize arrows along the handle's direction on screen.
        let gpu = self.gpu.as_ref()?;
        let q = xf.warp.quad;
        let c = Point::new((q[0].x + q[2].x) / 2.0, (q[0].y + q[2].y) / 2.0);
        let dir = |p: Point| {
            let (a, b) = (gpu.canvas_to_screen(pt(c)), gpu.canvas_to_screen(pt(p)));
            let ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees().rem_euclid(180.0);
            match ang { a if !(22.5..157.5).contains(&a) => CursorIcon::EwResize, a if a < 67.5 => CursorIcon::NwseResize, a if a < 112.5 => CursorIcon::NsResize, _ => CursorIcon::NeswResize }
        };
        // A skew slides along the edge.
        let skew = |i: usize| dir(Point::new(c.x + q[(i + 1) % 4].x - q[i].x, c.y + q[(i + 1) % 4].y - q[i].y));
        Some(match hit {
            Hit::Inside => Wanted::System(CursorIcon::Move),
            Hit::Outside => Wanted::Rotate,
            Hit::Pivot => Wanted::System(CursorIcon::Crosshair),
            Hit::Corner(_) if mode == TransformMode::Perspective || self.ctrl_held() => Wanted::System(CursorIcon::Crosshair),
            Hit::Corner(i) => Wanted::System(dir(q[i])),
            Hit::Side(i) => Wanted::System(skew(i)),
            Hit::Edge(i) if self.ctrl_held() => Wanted::System(skew(i)),
            Hit::Edge(i) => Wanted::System(dir(xf.edge_handles()[i])),
            Hit::Point(_) => if dragging { Wanted::HandClosed } else { Wanted::Hand },
        })
    }
}

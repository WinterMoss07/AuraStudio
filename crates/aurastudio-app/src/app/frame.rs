//! Frames: what the paint thread merged to the GPU, the interface run
//! (or its last frame reused), the settings applied after it; playback;
//! the monitor's colors.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurastudio_color::{monitor_profile, DisplaySetup, Intent, MonitorChoice, ViewMode};
use aurastudio_core::DocType;
use aurastudio_document::{Edit, Region};
use aurastudio_tools::xform::Xform;
use aurastudio_ui::{Actions, Loupe, Model, Overlay, Tool, TransformInfo, TransformMode, TransformOverlay};

use super::{gpu_quality, lock_soon, save_prefs, save_recent, Ants, App, Gesture, Pending};
use crate::paint_thread;
use crate::window::Screen;

impl App {
    /// Shortest time between two frames showing new paint: the paint
    /// thread merges once per canvas frame, a little early is let through
    /// (two clocks: waiting for the next one would show it a frame late).
    pub(super) fn paint_frame(&self) -> Duration { paint_thread::canvas_frame() * 3 / 4 }

    /// Interface size and 100 % zoom meaning, from the settings.
    pub(super) fn apply_settings(&mut self) {
        paint_thread::set_canvas_frame(Duration::from_secs_f64(1.0 / self.prefs.canvas_fps.max(1) as f64));
        self.apply_onion_look();
        let Some(gpu) = &mut self.gpu else { return };
        gpu.set_pixel_exact(!self.prefs.hidpi_scaling);
        gpu.set_ui_scale(self.prefs.ui_scale);
        gpu.set_quality(gpu_quality(self.prefs.canvas_quality));
        gpu.set_smooth_zoom_in(self.prefs.smooth_zoom_in);
        gpu.set_vsync(self.prefs.vsync);
        self.input.set_debounce(Duration::from_millis(self.prefs.debounce_ms as u64));
        self.update_cursor();
        self.request_redraw();
    }

    /// The onion skin's colors and fade from the preferences, on the
    /// document (shown again if they changed).
    pub(super) fn apply_onion_look(&mut self) {
        let p = &self.prefs;
        let look = aurastudio_animation::OnionLook { before: p.onion_before, after: p.onion_after, thickness: p.onion_thickness, by_frames: p.onion_by_frames };
        let Ok(mut d) = self.doc.try_lock() else { return };
        if d.timeline().look == look { return; }
        let done = d.apply(Edit::SetOnionLook(look));
        drop(d);
        self.show(done.region);
    }



    /// Play the animation from the current frame (from the first if at
    /// the end), at the document's frame rate.
    pub(super) fn start_playback(&mut self) {
        if self.is_drawing || self.xf.is_some() || self.xf_pending { return; }
        let Ok(mut doc) = self.doc.try_lock() else { return };
        if doc.kind() != DocType::Animation { return; }
        use aurastudio_animation::Sheets;
        let mut region = aurastudio_document::Region::Nothing;
        if doc.timeline().frame() >= doc.stack().last_frame() { region.add(doc.apply(Edit::SetFrame(1)).region); }
        let from = doc.timeline().frame();
        // The onion skin during playback only if asked.
        if !self.prefs.onion_playback { region.add(doc.apply(Edit::PauseOnion(true)).region); }
        drop(doc);
        self.show(region);
        let now = Instant::now();
        self.play = Some((now, from));
        self.play_next = Some(now);
        self.request_redraw();
        crate::memory::event("play start");
    }

    pub(super) fn stop_playback(&mut self) {
        if self.play.take().is_some() {
            let done = self.doc.lock().unwrap_or_else(|e| e.into_inner()).apply(Edit::PauseOnion(false));
            self.show(done.region);
            self.request_redraw();
            crate::memory::event("play stop");
        }
        self.play_next = None;
    }

    /// Playing: show the frame of now (frames are skipped if the merge
    /// can't keep up, the time stays right); loop or stop at the last
    /// frame drawn.
    pub(super) fn tick_playback(&mut self) {
        use aurastudio_animation::Sheets;
        let Some((start, from)) = self.play else { return };
        let now = Instant::now();
        if self.play_next.is_some_and(|t| now < t) { return; }
        let Ok(mut doc) = self.doc.try_lock() else {
            self.play_next = Some(now + Duration::from_millis(2));
            return;
        };
        let fps = doc.timeline().fps;
        let fps = (fps.num as f64 / fps.den.max(1) as f64).clamp(1.0, 120.0);
        let step = (now.duration_since(start).as_secs_f64() * fps) as u64;
        let last = doc.stack().last_frame() as u64;
        let mut t = from as u64 + step;
        let mut stop = false;
        if t > last {
            if self.looping && last > 1 { t = 1 + (t - 1) % last; } else { t = last; stop = true; }
        }
        let done = doc.apply(Edit::SetFrame(t as u32));
        drop(doc);
        self.show(done.region);
        self.request_redraw();
        if stop { self.stop_playback(); return; }
        self.play_next = Some(start + Duration::from_secs_f64((step + 1) as f64 / fps));
    }

    /// Go `d` frames back or forward (Animation documents; not during a
    /// stroke or a transform).
    pub(super) fn step_frame(&mut self, d: i64) {
        self.stop_playback();
        if self.is_drawing || self.xf.is_some() || self.xf_pending || self.ui.has_dialog() { return; }
        let done = {
            let Ok(mut doc) = self.doc.try_lock() else { return };
            if doc.kind() != DocType::Animation { return; }
            let now = doc.timeline().frame();
            let t = (now as i64 + d).max(1) as u32;
            if t == now { return; }
            doc.apply(Edit::SetFrame(t))
        };
        self.show(done.region);
        self.request_redraw();
    }

    /// The paint thread updated the projection: show it.
    pub(super) fn on_painted(&mut self) {
        self.painted = true;
        self.request_canvas_frame();
        self.render_if_due();
    }

    /// Render now when a frame is due (Windows sends WM_PAINT only once the
    /// queue is empty: pen packets kept it from ever coming). New paint:
    /// as soon as it is merged; else one frame per refresh.
    pub(super) fn render_if_due(&mut self) {
        let min = if self.painted { self.paint_frame() } else { self.frame_interval };
        if !self.painted && !self.redraw_wanted { return; }
        if self.last_frame.is_some_and(|t| t.elapsed() < min) { return; }
        self.redraw_wanted = false;
        self.deferred_frame = None;
        self.render_frame();
    }

    /// Window > Memory report open: measure once a second (the document
    /// is skipped while the paint thread has it) and show it.
    pub(super) fn tick_memory(&mut self) {
        if !self.ui.memory_wanted() {
            self.memory_at = None;
            return;
        }
        let now = Instant::now();
        if self.memory_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) { return; }
        let gpu = self.gpu.as_ref().map(Screen::memory);
        let pending = self.projection.try_lock().map_or(0, |p| p.pending_bytes());
        let report = match self.doc.try_lock() {
            Ok(d) => crate::memory::build(Some(&d), pending, gpu.as_ref()),
            Err(_) => return,
        };
        crate::memory::log(&report);
        self.ui.set_memory(report);
        self.memory_at = Some(now);
        self.ui_dirty = true;
        self.redraw_wanted = true;
        let next = now + Duration::from_secs(1);
        self.repaint_at = Some(self.repaint_at.map_or(next, |t| t.min(next)));
    }

    #[profiling::function]
    pub(super) fn render_frame(&mut self) {
        self.wakes.frames += 1;
        self.update_outline();
        let preview = self.shape_preview();
        let drawing = self.gesture_outline();
        let ants_offset = match &self.gesture { Some(Gesture::MoveSel { start, now }) => ((now.x - start.x).round() as f32, (now.y - start.y).round() as f32), _ => (0.0, 0.0) };
        let xf_data = self.transform_overlay_data();
        let transform_info = TransformInfo { active: self.xf.is_some(), numbers: self.xf.as_ref().and_then(Xform::numbers) };
        let transforming = self.xf.is_some() || self.xf_pending;
        // The loupe: where, what it marks, its rim and sign.
        let loupe_req = self.loupe_on().and_then(|pixel| {
            let c = self.window_to_canvas(self.cursor.0, self.cursor.1)?;
            let mark = match (self.tool == Tool::Picker, self.select_mark()) {
                (true, _) | (false, crate::cursors::Mark::None) => None,
                (false, crate::cursors::Mark::Add) => Some("+"),
                (false, crate::cursors::Mark::Subtract) => Some("\u{2212}"),
                (false, crate::cursors::Mark::Intersect) => Some("\u{00d7}"),
            };
            Some((c, pixel, self.tool == Tool::Picker, mark))
        });
        self.loupe_shown = loupe_req.is_some();
        // Alt + right drag: the brush's hardness and the value set.
        let brush_drag = self.size_drag.as_ref().and_then(|d| Some(aurastudio_ui::BrushDrag {
            center:   self.window_to_canvas(d.anchor.0, d.anchor.1)?,
            radius:   self.brush.size / 2.0,
            hardness: self.brush.hardness,
            pointer:  self.window_to_canvas(self.cursor.0, self.cursor.1)?,
            label:    d.label(),
        }));
        let Some(gpu) = &mut self.gpu else { return };
        let t0 = Instant::now();

        // Upload what the paint thread merged (taken out under the
        // projection's own lock, copied to the GPU after).
        if std::mem::take(&mut self.painted) {
            let patches = self.projection.lock().unwrap().take_patches();
            for (r, rgba) in &patches {
                gpu.upload(*r, rgba);
            }
        }

        let max_canvas = gpu.max_canvas_size();
        let (view_zoom, view_angle) = (gpu.display_zoom(), gpu.view.angle.to_degrees());
        // (Compared with `self.prefs` after the frame: one copy.)
        let mut prefs = self.prefs.clone();
        let display_profile = self.display_profile.as_ref().map(|p| p.name());
        let ui_state   = &mut self.ui;
        let doc        = &self.doc;
        let brush      = &mut self.brush;
        let presets    = &self.library.presets[..];
        let preset_before = self.preset;
        let mut preset = self.preset;
        let canvas_edit = &mut self.canvas_edit;
        let home = self.home;
        let maximized = self.window.as_ref().is_some_and(|w| w.is_maximized());
        let tool_before = self.tool;
        let tool = &mut self.tool;
        let tool_memory = &mut self.tool_memory;
        let fill = &mut self.fill;
        let stay_inside = &mut self.stay_inside;
        // Zoomed out, the outline is traced coarser (its detail can't show).
        let ants_step = (1.0 / gpu.zoom().max(1e-3)).floor().max(1.0) as u32;
        let ants = &mut self.ants;
        let select = &mut self.select;
        let picker = &mut self.picker;
        let transform_opts = &mut self.transform_opts;
        let playing = self.play.is_some();
        let looping = &mut self.looping;
        let renderer = self.renderer.as_str();
        let shape = &mut self.shape;
        let snap = gpu.view_snapshot();
        let to_points = move |p: (f32, f32)| snap.to_points(p);
        let mut actions = Actions::default();
        let mut done = aurastudio_document::Done::default();
        // Never wait for the brush: document busy, redraw with the last UI
        // and try again in a moment.
        let mut doc_busy = false;
        let run_ui = std::mem::take(&mut self.ui_dirty) || !gpu.has_cached_ui();
        let ui_saw_canvas = &mut self.ui_saw_canvas;
        let over_canvas = self.over_canvas;
        profiling::scope!("frame", if run_ui { "UI run" } else { "UI cached" });
        // Outside a stroke, wait a little for it: a merge on the paint
        // thread takes it band by band and lets go between two. During a
        // stroke, a short wait: the paint thread lets the UI in between two
        // samples.
        let wait = if self.is_drawing { Duration::from_millis(3) } else { Duration::from_millis(15) };
        match if run_ui { lock_soon(doc, wait) } else { None } {
            Some(mut doc) => gpu.render(self.lift.as_mut(), |root| {
            *ui_saw_canvas = over_canvas;
            let d = &mut *doc;
            // The outline, traced again when the selection or the zoom changed.
            match d.selection() {
                Some(sel) if sel.stamp() != ants.stamp || ants_step != ants.step => {
                    let mut step = ants_step.next_power_of_two();
                    let mut segs = sel.outline(step);
                    // Very detailed (noise picked by the wand): coarser.
                    while segs.len() > 120_000 && step < 64 { step *= 2; segs = sel.outline(step); }
                    *ants = Ants { stamp: sel.stamp(), step: ants_step, segs };
                }
                Some(_) => {}
                None => *ants = Ants::default(),
            }
            // The pixels around the cursor, for the loupe.
            let loupe = loupe_req.map(|(c, pixel, ring, mark)| {
                let side = aurastudio_ui::LOUPE_SIDE as i32;
                let (fx, fy) = (c.0.floor() as i32, c.1.floor() as i32);
                let origin = (fx - side / 2, fy - side / 2);
                let px = d.merged(aurastudio_core::Rect::new(origin.0, origin.1, origin.0 + side, origin.1 + side), aurastudio_layers::Pick::ALL);
                let rgba: Vec<u8> = px.into_iter().flatten().collect();
                let mid = ((side / 2 * side + side / 2) * 4) as usize;
                let ring = ring.then(|| [rgba[mid], rgba[mid + 1], rgba[mid + 2]]);
                let target = if pixel { (fx as f32, fy as f32) } else { (c.0.round(), c.1.round()) };
                (c, target, pixel, origin, rgba, ring, mark)
            });
            let mut model = Model {
                brush,
                presets,
                preset: &mut preset,
                doc: d,
                done: aurastudio_document::Done::default(),
                playing,
                looping,
                tool,
                tools: tool_memory,
                fill,
                stay_inside,
                select,
                picker,
                transform: transform_opts,
                transform_info,
                renderer,
                overlay: Overlay {
                    // (The selection moves with a transform: its box shows.)
                    ants: if transforming { &[] } else { &ants.segs },
                    ants_offset,
                    drawing: drawing.as_deref(),
                    shape: preview.as_ref().map(|p| &p.0[..]),
                    handles: preview.as_ref().map_or(&[], |p| &p.1[..]),
                    guide: preview.as_ref().map_or(&[], |p| &p.2[..]),
                    transform: xf_data.as_ref().map(|(lines, handles, points, pivot)| TransformOverlay { lines, handles, points, pivot: *pivot }),
                    loupe: loupe.as_ref().map(|(cursor, target, pixel, origin, rgba, ring, mark)| Loupe {
                        cursor: *cursor, target: *target, pixel: *pixel, origin: *origin, side: aurastudio_ui::LOUPE_SIDE, rgba, ring: *ring, mark: *mark,
                    }),
                    brush_drag,
                },
                shape,
                to_points: &to_points,
                view_zoom,
                view_angle,
                prefs: &mut prefs,
                display_profile,
                canvas_edit,
                max_canvas,
                home,
                maximized,
            };
            aurastudio_ui::show(root, ui_state, &mut model, &mut actions);
            done = std::mem::take(&mut model.done);
            }),
            None => {
                doc_busy = run_ui;
                gpu.render_cached_ui(self.lift.as_mut());
            }
        }
        let mut want_frame = false;
        if doc_busy {
            // Run the UI again in a moment.
            self.ui_dirty = true;
            let at = Instant::now() + Duration::from_millis(4);
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
        }
        match gpu.take_repaint_delay() {
            Some(d) if d.is_zero() => { want_frame = true; self.wakes.egui += 1; }
            // egui says "never" with a huge delay.
            Some(d) if d < Duration::from_secs(3600) => {
                let at = Instant::now() + d;
                self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            }
            _ => {}
        }
        if t0.elapsed() > Duration::from_millis(100) {
            log::warn!("slow frame: {:.0} ms (UI {})", t0.elapsed().as_secs_f64() * 1000.0, if run_ui { "run" } else { "cached" });
        }
        // The UI waits while the paint thread holds the document: how long.
        match (doc_busy, self.ui_blocked_since) {
            (true, None) => self.ui_blocked_since = Some(t0),
            (false, Some(t)) if run_ui => {
                self.ui_blocked_since = None;
                if t.elapsed() > Duration::from_millis(100) {
                    // What the paint thread was most likely busy with.
                    let work = if self.is_drawing {
                        format!("stroke, brush {} {:.0} px", self.library.presets.get(self.preset).map_or("?", |p| p.name.as_str()), self.brush.size)
                    } else if self.xf.is_some() || self.xf_pending {
                        "transform preview".to_owned()
                    } else {
                        "an edit".to_owned()
                    };
                    log::warn!("UI waited {:.0} ms for the document (paint thread busy: {work})", t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            _ => {}
        }
        self.last_frame = Some(Instant::now());
        // egui's cursor wish over its widgets is known now.
        self.apply_cursor();

        if prefs != self.prefs {
            let color = (prefs.color_managed, prefs.perceptual) != (self.prefs.color_managed, self.prefs.perceptual);
            self.prefs = prefs;
            save_prefs(&self.prefs);
            self.apply_settings();
            if color { self.apply_color_management(); }
            // The autosave timer restarts with the new delay.
            self.autosave_at = None;
        }
        if preset != preset_before { self.select_preset(preset); }
        if self.tool != tool_before {
            let tool = std::mem::replace(&mut self.tool, tool_before);
            self.set_tool(tool);
        }
        self.tool_memory.remember(self.tool, self.preset, &self.library.presets);
        if let Some(cmd) = actions.select { self.select(cmd); }
        if let Some(cmd) = actions.transform { self.transform_cmd(cmd); }
        // Warp: its grid shows as soon as the mode is chosen.
        if let (TransformMode::Warp, Some(xf)) = (self.transform_opts.mode, &mut self.xf) {
            if xf.warp.mesh.is_none() {
                let n = self.transform_opts.grid.clamp(1, 8) as usize;
                xf.warp.mesh = Some(aurastudio_transform::Mesh::new(n, n));
                want_frame = true;
            }
        }
        self.follow_transform_targets();
        if let Some(cmd) = actions.edit { self.edit_cmd(cmd); }
        if actions.import_brushes { self.import_brushes(); }
        if let Some(action) = actions.filter {
            use aurastudio_ui::FilterAction as F;
            let edit = match action { F::Preview(f) => Edit::FilterPreview(f), F::Apply(f) => Edit::FilterApply(f), F::Cancel => Edit::FilterCancel };
            self.send_edit(edit);
        }
        if let Some(choice) = actions.dialog { self.on_dialog(choice); }
        if let Some(cmd) = actions.file { self.file_cmd(cmd); }
        if let Some(spec) = actions.new_doc { self.request(Pending::Create(spec)); }
        if let Some(path) = actions.open_path { self.request(Pending::Open(path)); }
        if actions.clear_recent {
            self.recent.clear();
            save_recent(&self.recent);
            self.refresh_home();
        }
        if let Some(path) = actions.forget_recent {
            self.recent.retain(|p| *p != path);
            save_recent(&self.recent);
            self.refresh_home();
        }
        // What the interface did to the document: shown, thumbnails.
        if done.changed() {
            for &l in &done.layers { self.ui.invalidate_thumbnail(l, done.region.bounds().flatten()); }
            if !self.layers_dirty { self.show(done.region.clone()); }
        }
        if let Some(cmd) = actions.window { self.window_cmd(cmd); }
        if let Some(cmd) = actions.view { self.view_cmd(cmd); }
        match actions.play {
            Some(true) => self.start_playback(),
            Some(false) => self.stop_playback(),
            None => {}
        }
        if actions.undo { self.undo(); }
        if actions.redo { self.redo(); }
        if actions.clear_layer { self.clear_active_layer(); }
        // A new size: on the paint thread, after the strokes sent before
        // (`resized` follows).
        if let Some((w, h)) = actions.resize_to { self.send_edit(Edit::Resize { width: w, height: h }); }
        if self.layers_dirty && self.gpu.is_some() {
            // A document just shown: the paint thread merges it all, by
            // bands, off this thread.
            self.show(Region::Whole);
            self.layers_dirty = false;
            want_frame = true;
        }
        if want_frame {
            self.request_redraw();
        }
    }

    /// Read the profile of the monitor the window is on; rebuild the
    /// display table if it changed (startup, window moved to another
    /// monitor, profile changed in the system settings).
    pub(super) fn refresh_display_profile(&mut self) {
        let (profile, err) = monitor_profile(&MonitorChoice::System, self.hwnd());
        if let Some(e) = err { log::warn!("monitor profile: {e}"); }
        if self.display_profile.as_ref() == Some(&profile) { return; }
        log::info!("monitor profile: {}", profile.name());
        self.display_profile = Some(profile);
        self.apply_color_management();
    }

    /// The document's colors through the monitor's profile: one table for
    /// the canvas shader and the color docker, or none (sRGB-like monitor,
    /// or off in Preferences).
    pub(super) fn apply_color_management(&mut self) {
        let document = match self.doc.try_lock() { Ok(d) => d.profile().clone(), Err(_) => return };
        let lut = match (&self.display_profile, self.prefs.color_managed) {
            (Some(monitor), true) => {
                let intent = if self.prefs.perceptual { Intent::Perceptual } else { Intent::RelativeColorimetric };
                let setup = DisplaySetup { document, monitor: monitor.clone(), intent, mode: ViewMode::Normal };
                match setup.build() {
                    Ok(t) if t.is_identity() => { log::info!("color management: nothing to convert (sRGB-like monitor)"); None }
                    Ok(t) => { log::info!("color management: on ({} → {})", setup.document.name(), setup.monitor.name()); Some(Arc::new(t)) }
                    Err(e) => { log::warn!("color management off: {e}"); None }
                }
            }
            _ => None,
        };
        if let Some(gpu) = &mut self.gpu { gpu.set_display_lut(lut.as_deref()); }
        aurastudio_ui::set_display_transform(lut.map(|t| {
            Arc::new(move |c: [u8; 3]| t.apply_u8(c)) as Arc<dyn Fn([u8; 3]) -> [u8; 3] + Send + Sync>
        }));
        self.request_redraw();
    }
}

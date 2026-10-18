use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

use aurastudio_brush::Brush;
use aurastudio_brushes::Library;
use aurastudio_gpu::GpuState;
use aurastudio_input::Stylus;
use aurastudio_color::{monitor_profile, DisplayProfile, DisplayTransform, Intent, LUT_SIZE};
use aurastudio_document::{SharedDoc, Workspace};
use aurastudio_ui::{Actions, ImportBrushes, Model, UiState, ViewCmd};

use crate::paint_thread::{self, Cmd, PaintThread, Timing};
use crate::projection::{Projection, SharedProjection};

/// Shortest time between two frames that only update the UI.
const UI_FRAME: Duration = Duration::from_micros(16_667);

/// Canvas size at startup; changed afterwards from the Canvas docker.
const DEFAULT_CANVAS: (u32, u32) = (2048, 2048);
/// Zoom drag speed: the zoom is multiplied by e^(ZOOM_DRAG * dx px).
const ZOOM_DRAG: f32 = 0.007;

/// View navigation drag in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav { Pan, Rotate, Zoom }

/// Message posted to the event loop (by the app itself or the paint thread).
#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
    /// Wake the loop so about_to_wait runs and requests the wanted frame.
    Wake,
    /// The paint thread changed some canvas area.
    Painted,
    /// A layer's pixels changed for good (stroke done, undo, clear).
    LayerChanged(u64),
    /// Message for the options bar, from the paint thread.
    Notice(&'static str),
}

pub struct App {
    proxy:  EventLoopProxy<AppEvent>,
    window: Option<Arc<Window>>,
    gpu:    Option<GpuState>,
    stylus: Option<Stylus>,

    /// Brush presets (built-in, MyPaint collection, imported).
    library:      Library,
    /// Selected preset.
    preset:       usize,
    /// Working settings of the selected preset (copied back into it when
    /// another one is selected) + the global color.
    brush:        Brush,

    /// Every open document. One for now (no document tabs yet): it owns
    /// them, `doc` is the one shown.
    #[allow(dead_code)]
    workspace: Workspace,
    /// The document shown, shared with the paint thread.
    doc:    SharedDoc,
    paint:  PaintThread,
    /// Merged image shown on screen, filled by the paint thread.
    projection: SharedProjection,
    /// Width / height typed in the Canvas dock, applied by "Resize".
    canvas_edit: (u32, u32),
    ui:          UiState,

    is_drawing: bool,
    pressure:   f32,
    /// Pen samples sent for the current stroke (0 at release = a click
    /// without moving: paint one dot).
    samples_sent: u32,
    /// Time origin for mouse samples (pen samples carry their own time).
    start:      Instant,
    modifiers:  ModifiersState,
    /// Monitor ICC profile (None: not found, treated as sRGB).
    display_profile: Option<DisplayProfile>,
    /// Display color management of the canvas (View menu).
    color_managed: bool,
    perceptual:    bool,
    /// Space held: a left drag moves the view instead of painting.
    space_down: bool,
    /// A Shift key is held (tracked from the keys too, see shift_held).
    shift_down: bool,
    /// A Ctrl key is held (same reason).
    ctrl_down:  bool,
    /// View drag in progress, with the last cursor position, and the
    /// button that started it (only its release ends the drag).
    nav:        Option<(Nav, (f32, f32))>,
    nav_button: MouseButton,
    /// Fixed point of the zoom / rotation drag (window pixels).
    nav_pivot:  (f32, f32),
    /// Set once the native pen sent samples during the current stroke:
    /// the CursorMoved events Windows synthesizes for the same pen are then
    /// ignored, otherwise the stroke gets two slightly offset position
    /// streams (wavy / zigzag edges).
    pen_active: bool,
    /// Last known cursor position, physical window pixels.
    cursor:     (f32, f32),
    /// The cursor is over the canvas area (not a docker), from the last
    /// event; and inside the window at all.
    over_canvas: bool,
    cursor_in_window: bool,
    /// The brush outline is drawn (it replaces the OS cursor).
    outline_shown: bool,
    /// Our cursors (hands, rotate), made once the event loop runs.
    cursors: Option<crate::cursors::Cursors>,

    /// The paint thread updated the projection: upload it next frame.
    painted:      bool,
    /// A change outside strokes (visibility, opacity, add/remove/clear
    /// layer, resize) needs a full-canvas recomposite.
    layers_dirty: bool,
    last_frame:   Option<Instant>,
    /// Shortest time between two frames outside strokes: one display
    /// refresh. Without it, anything asking for "a frame right away" on
    /// every frame (egui does, for animations and tooltips) runs as fast as
    /// the GPU goes with the Immediate present mode.
    frame_interval: Duration,
    /// A frame wanted before `frame_interval` was over: drawn at this time.
    deferred_frame: Option<Instant>,
    /// A frame is needed. The actual window redraw request is made in
    /// about_to_wait: winit drops requests made while handling a redraw.
    redraw_wanted: bool,
    /// The next frame must run the UI. Frames that only move the brush
    /// outline or show new paint reuse the last UI as it was (no egui
    /// layout): a pen hovering over the canvas costs a GPU pass, not a
    /// full UI frame.
    ui_dirty:     bool,
    /// The cursor was over the canvas at the last full UI frame.
    ui_saw_canvas: bool,
    /// What woke the app lately, logged when it keeps drawing while
    /// nothing is being painted (idle CPU hunt).
    activity:     Activity,
    /// Delayed frame asked by egui (tooltip, notice fade-out).
    repaint_at:   Option<Instant>,
    /// Main-thread frame cost during the current stroke (logged at its end).
    frame_stats:  Timing,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let library = Library::load();
        log::info!("brushes: {} presets ({} groups)", library.presets.len(), library.groups().len());
        let mut workspace = Workspace::new();
        let doc = workspace.new_document("Untitled", DEFAULT_CANVAS.0, DEFAULT_CANVAS.1);
        let projection = Arc::new(std::sync::Mutex::new(Projection::new(DEFAULT_CANVAS.0, DEFAULT_CANVAS.1)));
        let worker_proxy = proxy.clone();
        let paint = PaintThread::spawn(Arc::clone(&projection), move |e| {
            let _ = worker_proxy.send_event(match e {
                paint_thread::Event::Painted => AppEvent::Painted,
                paint_thread::Event::LayerChanged { layer_id } => AppEvent::LayerChanged(layer_id),
                paint_thread::Event::Notice(text) => AppEvent::Notice(text),
            });
        });
        Self {
            proxy,
            window: None,
            gpu:    None,
            stylus: None,

            brush:       library.presets[0].settings.clone(),
            library,
            preset:      0,

            workspace,
            doc,
            paint,
            projection,
            canvas_edit: DEFAULT_CANVAS,
            ui:          UiState::new(),

            is_drawing: false,
            pressure:   1.0,
            samples_sent: 0,
            start:      Instant::now(),
            modifiers:  ModifiersState::empty(),
            display_profile: None,
            color_managed: true,
            perceptual:    true,
            space_down: false,
            shift_down: false,
            ctrl_down:  false,
            nav_pivot:  (0.0, 0.0),
            nav:        None,
            nav_button: MouseButton::Left,
            pen_active: false,
            cursor:     (0.0, 0.0),
            over_canvas: false,
            cursor_in_window: false,
            outline_shown: false,
            cursors: None,

            painted:      false,
            // Upload the first composite.
            layers_dirty: true,
            last_frame:   None,
            frame_interval: Duration::from_micros(16_667),
            deferred_frame: None,
            redraw_wanted: false,
            ui_dirty:     true,
            ui_saw_canvas: false,
            activity:     Activity::new(),
            repaint_at:   None,
            frame_stats:  Timing::default(),
        }
    }

    /// Window pixels -> canvas pixels, through the view (zoom, rotation,
    /// pan): the exact inverse of what the GPU draws.
    fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        Some(self.gpu.as_ref()?.screen_to_canvas((wx, wy)))
    }

    /// Which view drag the held keys ask for: Ctrl zooms, Shift rotates,
    /// nothing pans (Krita / CSP).
    fn nav_mode(&self) -> Nav {
        if self.ctrl_held() { Nav::Zoom } else if self.shift_held() { Nav::Rotate } else { Nav::Pan }
    }

    /// Start panning / rotating / zooming the view with a drag from the
    /// cursor.
    fn begin_nav(&mut self, button: MouseButton) {
        let mode = self.nav_mode();
        log::info!("view drag: {mode:?}");
        self.nav = Some((mode, self.cursor));
        self.nav_button = button;
        self.set_nav_pivot(mode);
        self.update_cursor();
    }

    /// Fixed point of a zoom or rotation drag, chosen when it starts (or
    /// switches mode): zoom around where the drag began (CSP); rotate
    /// around the picture's center while all of it is in view, else around
    /// the middle of the canvas area.
    fn set_nav_pivot(&mut self, mode: Nav) {
        let Some(gpu) = &self.gpu else { return };
        self.nav_pivot = match mode {
            Nav::Zoom => self.cursor,
            _ => gpu.rotation_pivot(),
        };
    }

    fn end_nav(&mut self) {
        self.nav = None;
        self.update_cursor();
    }

    /// Shift is down, from the modifier state or the Shift keys
    /// themselves (whichever Windows reported last: with the pen, the
    /// modifier state alone was seen missing Shift).
    fn shift_held(&self) -> bool {
        self.modifiers.shift_key() || self.shift_down
    }

    fn ctrl_held(&self) -> bool {
        self.modifiers.control_key() || self.ctrl_down
    }

    /// Cursor moved during a navigation drag.
    fn drag_nav(&mut self) {
        let Some((prev_mode, last)) = self.nav else { return };
        // The mode follows Ctrl / Shift during the drag (Krita): pressing or
        // releasing one mid-drag switches.
        let mode = self.nav_mode();
        if mode != prev_mode {
            self.set_nav_pivot(mode);
            self.update_cursor();
        }
        let pivot = self.nav_pivot;
        let Some(gpu) = &mut self.gpu else { return };
        let now = self.cursor;
        match mode {
            Nav::Pan => gpu.view.pan_by(now.0 - last.0, now.1 - last.1),
            Nav::Rotate => {
                // Turn by the angle the cursor swept around the pivot.
                let a0 = (last.1 - pivot.1).atan2(last.0 - pivot.0);
                let a1 = (now.1 - pivot.1).atan2(now.0 - pivot.0);
                gpu.view_rotate_at(aurastudio_gpu::wrap_angle(a1 - a0), pivot);
            }
            // Horizontal drag (CSP): right zooms in, left zooms out; 100 px
            // is about x2.
            Nav::Zoom => gpu.view_zoom_at((ZOOM_DRAG * (now.0 - last.0)).exp(), pivot),
        }
        self.nav = Some((mode, now));
        self.request_redraw();
    }

    /// Brush outline under the cursor over the canvas (Krita style), in
    /// place of the OS arrow; the arrow / hand comes back over the dockers
    /// and while navigating.
    fn update_outline(&mut self) {
        let show = self.over_canvas && self.cursor_in_window && !self.space_down && self.nav.is_none();
        self.outline_shown = show;
        if let Some(gpu) = &mut self.gpu {
            let outline = show.then(|| (self.cursor, self.brush.radius as f32 * gpu.zoom()));
            gpu.set_brush_cursor(outline);
        }
    }

    /// Pointer, keys or view drag changed: outline and cursor follow.
    fn update_cursor(&mut self) {
        self.update_outline();
        self.apply_cursor();
    }

    /// The one place the window cursor is set (egui only suggests one).
    fn apply_cursor(&mut self) {
        use crate::cursors::Wanted;
        let wanted = match self.nav {
            Some((Nav::Pan, _)) => Wanted::HandClosed,
            Some((Nav::Rotate, _)) => Wanted::Rotate,
            Some((Nav::Zoom, _)) => Wanted::Zoom,
            None if self.space_down && self.over_canvas => match self.nav_mode() {
                Nav::Zoom => Wanted::Zoom,
                Nav::Rotate => Wanted::Rotate,
                Nav::Pan => Wanted::Hand,
            },
            None if self.over_canvas || self.is_drawing => Wanted::Hidden,
            None => self.gpu.as_ref().map_or(Wanted::System(CursorIcon::Default),
                |g| crate::cursors::from_egui(g.egui_cursor())),
        };
        let (Some(window), Some(cursors)) = (&self.window, &mut self.cursors) else { return };
        cursors.apply(window, wanted);
    }

    /// Win32 window handle, for the monitor profile lookup.
    fn hwnd(&self) -> Option<isize> {
        let handle = self.window.as_ref()?.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    /// Read the profile of the monitor the window is on; rebuild the
    /// display transform if it changed (startup, window moved to another
    /// monitor, profile changed in Windows settings).
    fn refresh_display_profile(&mut self) {
        let profile = monitor_profile(self.hwnd());
        if profile == self.display_profile { return; }
        match &profile {
            Some(p) => log::info!("monitor profile: {} ({} bytes)", p.name, p.icc.len()),
            None => log::info!("monitor profile: none found, canvas shown as sRGB"),
        }
        self.display_profile = profile;
        self.apply_color_management();
    }

    /// Push the sRGB -> monitor transform to the canvas shader and the
    /// color docker, or switch it off.
    fn apply_color_management(&mut self) {
        let intent = if self.perceptual { Intent::Perceptual } else { Intent::RelativeColorimetric };
        let transform = match (&self.display_profile, self.color_managed) {
            (Some(p), true) => match DisplayTransform::new(&p.icc, intent) {
                Ok(t) if t.is_identity() => {
                    log::info!("color management: monitor profile is sRGB-like, nothing to convert");
                    None
                }
                Ok(t) => {
                    log::info!("color management: on, {} intent ({} profile)", intent.name(),
                        if t.matrix_shaper { "matrix/curves" } else { "LUT" });
                    Some(Arc::new(t))
                }
                Err(e) => {
                    log::warn!("color management off: {e}");
                    None
                }
            },
            _ => None,
        };
        if let Some(gpu) = &mut self.gpu {
            gpu.set_display_lut(transform.as_ref().map(|t| (t.lut(), LUT_SIZE)));
        }
        aurastudio_ui::set_display_transform(transform.map(|t| {
            Arc::new(move |c: [u8; 3]| t.apply(c)) as Arc<dyn Fn([u8; 3]) -> [u8; 3] + Send + Sync>
        }));
        self.request_redraw();
    }

    /// Switch brush: the current one keeps the settings it has now (each
    /// brush has its own size, opacity, blend...), the new one brings its
    /// own. The color stays.
    fn select_preset(&mut self, index: usize) {
        if index == self.preset || index >= self.library.presets.len() { return; }
        let color = self.brush.color;
        let old = &mut self.library.presets[self.preset];
        old.settings = self.brush.clone();
        let new = &self.library.presets[index];
        self.brush = new.settings.clone();
        self.brush.color = color;
        self.preset = index;
    }

    /// File dialog, then copy the chosen MyPaint brushes into the library.
    fn import_brushes(&mut self, kind: ImportBrushes) {
        let dialog = rfd::FileDialog::new().set_title("Import MyPaint brushes");
        let paths: Vec<std::path::PathBuf> = match kind {
            ImportBrushes::Files => dialog.add_filter("MyPaint brush", &["myb"]).pick_files().unwrap_or_default(),
            ImportBrushes::Folder => dialog.pick_folder().into_iter().collect(),
        };
        if paths.is_empty() { return; }
        let mut added = 0;
        let mut error = None;
        for p in &paths {
            match self.library.import(p) {
                Ok(n) => added += n,
                Err(e) => error = Some(e),
            }
        }
        match (added, error) {
            (0, Some(e)) => self.ui.notify(&format!("Import failed: {e}")),
            (n, _) => self.ui.notify(&format!("{n} brush(es) imported")),
        }
        log::info!("brush import: {added} added from {paths:?}");
        self.request_redraw();
    }

    /// Keyboard / menu view commands.
    fn view_cmd(&mut self, cmd: ViewCmd) {
        let Some(gpu) = &mut self.gpu else { return };
        let c = gpu.view_center();
        let turn = gpu.rotation_pivot();
        let step = 15f32.to_radians();
        match cmd {
            ViewCmd::ZoomIn        => gpu.view_zoom_at(1.25, c),
            ViewCmd::ZoomOut       => gpu.view_zoom_at(1.0 / 1.25, c),
            ViewCmd::ActualSize    => gpu.view_set_zoom(1.0, c),
            ViewCmd::Fit           => gpu.view.fit_to_area(),
            ViewCmd::RotateLeft    => gpu.view_rotate_at(-step, turn),
            ViewCmd::RotateRight   => gpu.view_rotate_at(step, turn),
            ViewCmd::ResetRotation => gpu.view_reset_rotation(),
        }
        self.request_redraw();
    }

    /// Ask for a frame. The window redraw request itself is made in
    /// about_to_wait: one made while handling a redraw is dropped (Windows
    /// validates the window after WM_PAINT; X11 has already chosen how long
    /// to sleep). Posting Wake makes sure the loop gets there right away.
    fn request_redraw(&mut self) {
        self.ui_dirty = true;
        self.request_canvas_frame();
    }

    /// Ask for a frame that only needs the canvas and the brush outline
    /// redrawn; the UI is reused from the last frame.
    fn request_canvas_frame(&mut self) {
        if !self.redraw_wanted {
            self.redraw_wanted = true;
            let _ = self.proxy.send_event(AppEvent::Wake);
        }
    }

    /// Sleep until the next OS event, or until egui's delayed repaint.
    fn schedule(&self, event_loop: &ActiveEventLoop) {
        let wake = match (self.repaint_at, self.deferred_frame) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None    => ControlFlow::Wait,
        });
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    fn begin_stroke(&mut self) {
        // Instant feedback when the layer can't be painted. try_lock: if
        // the paint thread is still busy with the previous stroke, don't
        // wait for it; it checks again itself and reports a refusal.
        let refusal = match self.doc.try_lock() {
            Ok(doc) if doc.layers.can_paint() => None,
            Ok(doc) if doc.layers.active_node().is_folder() => Some("Select a layer to paint (a folder is selected)"),
            Ok(_) => Some("Layer is locked"),
            Err(_) => None,
        };
        if let Some(msg) = refusal {
            self.ui.notify(msg);
            self.is_drawing = false;
            self.request_redraw();
            return;
        }
        self.is_drawing = true;
        self.pen_active = false;
        self.samples_sent = 0;
        self.frame_stats = Timing::default();
        self.paint.send(Cmd::Begin {
            doc:       Arc::clone(&self.doc),
            brush:     self.brush.clone(),
            engine:    self.library.presets[self.preset].engine.clone(),
        });
    }

    fn end_stroke(&mut self) {
        // A click without moving sent nothing yet: paint one dot there.
        if self.samples_sent == 0 {
            self.draw_at(self.cursor.0, self.cursor.1, None);
        }
        self.is_drawing = false;
        self.paint.send(Cmd::End);
        self.pen_active = false;
    }

    /// Forward one sample to the paint thread (never paints here).
    /// `pen_time_ms`: the pen's own timestamp; mouse samples use the app
    /// clock.
    fn draw_at(&mut self, wx: f32, wy: f32, pen_time_ms: Option<u32>) {
        let Some((x, y)) = self.window_to_canvas(wx, wy) else { return };
        let time = match pen_time_ms {
            Some(ms) => ms as f64 / 1000.0,
            None => self.start.elapsed().as_secs_f64(),
        };
        self.samples_sent += 1;
        self.paint.send(Cmd::Sample { x, y, pressure: self.pressure, time });
    }

    /// The window lost focus (Alt+Tab...): key releases now go elsewhere,
    /// so drop held Space, any view drag and any stroke, instead of
    /// leaving them stuck.
    fn reset_input(&mut self) {
        self.space_down = false;
        self.shift_down = false;
        self.ctrl_down = false;
        self.nav = None;
        if self.is_drawing {
            self.end_stroke();
        }
        self.update_cursor();
        self.request_redraw();
    }

    /// Native pen samples (WinTab or Windows Ink). Press / release still
    /// comes from MouseInput, which Windows always synthesizes for the pen;
    /// only position and pressure come from here.
    fn drain_stylus(&mut self) {
        let Some(stylus) = &self.stylus else { return };
        for s in stylus.drain() {
            self.activity.pen_packets += 1;
            if self.is_drawing {
                self.pen_active = true;
                self.pressure   = s.pressure;
                // The outline follows the pen's own samples (newer than
                // the mouse events Windows derives from them).
                self.cursor     = (s.x, s.y);
                self.draw_at(s.x, s.y, Some(s.time_ms));
                self.request_canvas_frame();
            }
        }
    }

    /// The paint thread updated the projection: show it.
    fn on_painted(&mut self) {
        self.painted = true;
        self.request_canvas_frame();
        self.render_if_due();
    }

    /// Render now if a frame is pending and the last one is old enough.
    ///
    /// We can't rely on RedrawRequested alone while drawing: on Windows it
    /// comes from WM_PAINT, which the OS only sends once the message queue
    /// is empty. With big brushes each pen packet takes a few ms, packets
    /// keep arriving, the queue never empties and nothing was displayed
    /// until the pen was lifted. Krita likewise refreshes the canvas on its
    /// own timer rather than waiting for the window system.
    fn render_if_due(&mut self) {
        const MIN_FRAME: Duration = Duration::from_millis(4);
        if !self.painted { return; }
        if self.last_frame.is_some_and(|t| t.elapsed() < MIN_FRAME) { return; }
        self.render_frame();
    }

    fn render_frame(&mut self) {
        self.update_outline();
        let Some(gpu) = &mut self.gpu else { return };
        let t0 = Instant::now();

        // Upload what the paint thread merged. Only a copy into wgpu's
        // staging memory, under the projection's own lock.
        if std::mem::take(&mut self.painted) {
            let mut p = self.projection.lock().unwrap();
            for r in p.take_dirty() {
                gpu.upload_rect(&p.pixels, r);
            }
        }

        let max_canvas = gpu.max_canvas_size();
        let (view_zoom, view_angle) = (gpu.view.zoom, gpu.view.angle.to_degrees());
        let (cm_before, perceptual_before) = (self.color_managed, self.perceptual);
        let color_managed = &mut self.color_managed;
        let perceptual = &mut self.perceptual;
        let display_profile = self.display_profile.as_ref().map(|p| p.name.as_str());
        let ui_state   = &mut self.ui;
        let doc        = &self.doc;
        let brush      = &mut self.brush;
        let presets    = &self.library.presets[..];
        let preset_before = self.preset;
        let mut preset = self.preset;
        let canvas_edit = &mut self.canvas_edit;
        let mut actions = Actions::default();
        // Never wait for the brush: if the paint thread holds the document
        // (mid-stroke), redraw the canvas with the last UI as it was, and
        // try the UI again a moment later.
        let mut doc_busy = false;
        let run_ui = std::mem::take(&mut self.ui_dirty) || !gpu.has_cached_ui();
        let ui_saw_canvas = &mut self.ui_saw_canvas;
        let over_canvas = self.over_canvas;
        if run_ui { self.activity.ui_frames += 1 } else { self.activity.canvas_frames += 1 }
        match if run_ui { doc.try_lock().ok() } else { None } {
            Some(mut doc) => gpu.render(|ctx| {
            *ui_saw_canvas = over_canvas;
            let (can_undo, can_redo) = (doc.history().undo_len() > 0, doc.history().redo_len() > 0);
            let mut model = Model {
                brush,
                presets,
                preset: &mut preset,
                layers: &mut doc.layers,
                can_undo,
                can_redo,
                view_zoom,
                view_angle,
                color_managed,
                perceptual,
                display_profile,
                canvas_edit,
                max_canvas,
            };
            aurastudio_ui::show(ctx, ui_state, &mut model, &mut actions);
            }),
            None => {
                doc_busy = run_ui;
                gpu.render_cached_ui();
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
            Some(d) if d.is_zero() => want_frame = true,
            // egui says "never" with a huge delay.
            Some(d) if d < Duration::from_secs(3600) => {
                let at = Instant::now() + d;
                self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            }
            _ => {}
        }
        if self.is_drawing {
            self.frame_stats.add(t0.elapsed());
        }
        self.activity.main_time += t0.elapsed();
        self.last_frame = Some(Instant::now());
        // egui's cursor wish over its widgets is known now.
        self.apply_cursor();

        if (self.color_managed, self.perceptual) != (cm_before, perceptual_before) {
            self.apply_color_management();
        }
        if preset != preset_before { self.select_preset(preset); }
        if let Some(kind) = actions.import_brushes { self.import_brushes(kind); }
        if let Some(cmd) = actions.view { self.view_cmd(cmd); }
        if actions.undo { self.undo(); }
        if actions.redo { self.redo(); }
        if actions.clear_layer { self.clear_active_layer(); }
        let Some(gpu) = &mut self.gpu else { return };
        if actions.resize_to.is_some() || actions.recomposite || self.layers_dirty {
            // Layer visibility / opacity / order changed, or new size: merge
            // everything again (rare, never during a stroke).
            let mut doc = self.doc.lock().unwrap();
            let mut p = self.projection.lock().unwrap();
            if let Some((w, h)) = actions.resize_to {
                log::info!("canvas resize: {}x{} -> {w}x{h} (undo history cleared)", doc.layers.width, doc.layers.height);
                doc.resize(w, h);
                p.resize(w, h);
                gpu.resize_canvas(w, h);
                self.ui.invalidate_all_thumbnails();
            }
            let (w, h) = (doc.layers.width, doc.layers.height);
            doc.layers.composite_into(&mut p.pixels);
            p.add_dirty((0, 0, w, h));
            self.layers_dirty = false;
            self.painted = true;
            // Uploaded with the next frame.
            want_frame = true;
        }
        if want_frame {
            self.request_redraw();
        }
    }

    /// Delete key / Layer > Clear: erase the active layer (white for the
    /// background, transparent for the others), undoable. Done by the paint
    /// thread, after any stroke still being painted.
    fn clear_active_layer(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Clear(Arc::clone(&self.doc)));
    }

    fn undo(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Undo(Arc::clone(&self.doc)));
    }

    fn redo(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Redo(Arc::clone(&self.doc)));
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            // Only there to wake the loop; about_to_wait does the work.
            AppEvent::Wake => {}
            AppEvent::Painted => self.on_painted(),
            AppEvent::LayerChanged(layer_id) => {
                if self.frame_stats.count > 0 {
                    log::info!("stroke frames (composite+render, main thread): {}", self.frame_stats);
                    self.frame_stats = Timing::default();
                }
                self.on_painted();
                self.ui.invalidate_thumbnail(layer_id);
                self.request_redraw();
            }
            AppEvent::Notice(text) => {
                self.ui.notify(text);
                self.request_redraw();
            }
        }
    }

    /// Called once winit has dispatched every pending OS message, right
    /// before it waits for new ones. WinTab packets arrive as their own
    /// window messages, which produce no winit event: draining here paints
    /// them immediately instead of waiting for the next mouse event.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_stylus();
        self.activity.wakes += 1;
        self.activity.report(self.is_drawing || self.nav.is_some());

        if self.repaint_at.is_some_and(|t| Instant::now() >= t) {
            self.repaint_at = None;
            self.activity.timers += 1;
            self.ui_dirty = true;
            self.redraw_wanted = true;
        }
        let now = Instant::now();
        if self.deferred_frame.is_some_and(|t| now >= t) {
            self.deferred_frame = None;
            self.redraw_wanted = true;
        }
        if std::mem::take(&mut self.redraw_wanted) {
            // At most one frame per display refresh (strokes have their own
            // pacing, see render_if_due).
            // UI-only frames (a pen hovering over the docks moves the
            // pointer all the time) at most 60 per second: hover effects
            // don't need more. The canvas and the brush outline follow the
            // display refresh, and so does the UI while something is
            // dragged.
            let dragging = self.gpu.as_ref().is_some_and(|g| g.ui_is_dragging());
            let interval = if self.ui_dirty && !dragging && self.nav.is_none() {
                self.frame_interval.max(UI_FRAME)
            } else {
                self.frame_interval
            };
            let due = self.last_frame.map_or(now, |t| t + interval);
            if now >= due || self.is_drawing {
                if let Some(w) = &self.window { w.request_redraw(); }
            } else {
                self.deferred_frame = Some(self.deferred_frame.map_or(due, |t| t.min(due)));
            }
        }
        self.schedule(event_loop);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("AuraStudio v0.1")
            .with_inner_size(winit::dpi::LogicalSize::new(1440u32, 900u32));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size   = {
            let d = self.doc.lock().unwrap();
            (d.layers.width, d.layers.height)
        };
        let mut gpu = pollster::block_on(GpuState::new(Arc::clone(&window), size));
        gpu.clear_color = aurastudio_ui::theme::backdrop_rgb();
        log::info!("max canvas size: {} px", gpu.max_canvas_size());

        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let stylus = Stylus::install(h.hwnd.get());
                log::info!("pen input backend: {}", stylus.backend());
                self.stylus = Some(stylus);
            }
        }

        // One frame per refresh of the monitor the window opens on.
        if let Some(mhz) = window.current_monitor().and_then(|m| m.refresh_rate_millihertz()) {
            let hz = (mhz as f64 / 1000.0).clamp(30.0, 360.0);
            self.frame_interval = Duration::from_secs_f64(1.0 / hz);
            log::info!("display refresh: {hz:.0} Hz");
        }
        window.request_redraw();
        self.window = Some(window);
        self.cursors = Some(crate::cursors::Cursors::new(event_loop));
        self.gpu    = Some(gpu);
        self.refresh_display_profile();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id:        WindowId,
        event:      WindowEvent,
    ) {
        // egui sees every event and says when it needs a new frame (hover,
        // slider drag...). Whether a press starts a stroke is decided by
        // GpuState::is_canvas_at, see there for why egui can't decide it.
        //
        // RedrawRequested itself must never ask for another redraw here:
        // that made an endless loop (1.6 CPU cores while idle with the
        // cursor over a docker).
        let ui_needs_frame = self.gpu.as_mut()
            .map(|gpu| gpu.egui_handle_event(&event))
            .unwrap_or(false);
        // The hit test below uses the new position.
        if let WindowEvent::CursorMoved { position, .. } = &event {
            self.cursor = (position.x as f32, position.y as f32);
        }
        let over_canvas = self.gpu.as_ref()
            .map(|gpu| gpu.is_canvas_at(self.cursor))
            .unwrap_or(false);
        let ui_dragging = self.gpu.as_ref().is_some_and(|g| g.ui_is_dragging());
        let is_redraw = matches!(event, WindowEvent::RedrawRequested);
        // Moving over the canvas, when the last UI frame already had the
        // cursor there, changes nothing in the UI (no hover can change):
        // only the outline moves.
        let canvas_move = matches!(event, WindowEvent::CursorMoved { .. })
            && over_canvas && self.ui_saw_canvas && !ui_dragging;
        if !is_redraw && (ui_needs_frame || ui_dragging) && !canvas_move {
            self.request_redraw();
        }
        if !is_redraw { self.activity.events += 1; }
        self.over_canvas = over_canvas;

        self.drain_stylus();

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                self.request_redraw();
            }

            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),

            WindowEvent::Focused(false) => self.reset_input(),

            // The window may now be on another monitor (other profile).
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => self.refresh_display_profile(),

            WindowEvent::KeyboardInput { event: key, .. } => {
                // Space is tracked on release too (and even while a field
                // has focus, so it can't get stuck down).
                if key.logical_key == Key::Named(NamedKey::Shift) {
                    self.shift_down = key.state == ElementState::Pressed;
                    if self.space_down { self.update_cursor(); }
                }
                if key.logical_key == Key::Named(NamedKey::Control) {
                    self.ctrl_down = key.state == ElementState::Pressed;
                    if self.space_down { self.update_cursor(); }
                }
                if key.logical_key == Key::Named(NamedKey::Space) {
                    let down = key.state == ElementState::Pressed;
                    if down != self.space_down {
                        self.space_down = down;
                        self.update_cursor();
                    }
                }
                // Typing in a docker field (e.g. canvas width) must not
                // erase the layer, undo or move the view.
                let ui_typing = self.gpu.as_ref().is_some_and(|g| g.ui_wants_keyboard());
                if key.state != ElementState::Pressed || ui_typing { return; }
                let ctrl  = self.modifiers.control_key() || self.modifiers.super_key();
                let shift = self.modifiers.shift_key();
                match key.logical_key.as_ref() {
                    Key::Named(NamedKey::Delete) if !key.repeat => self.clear_active_layer(),
                    // Ctrl+Z undo, Ctrl+Shift+Z / Ctrl+Y redo (Krita, CSP,
                    // Photoshop). Holding the keys repeats, like everywhere.
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
                        if shift { self.redo() } else { self.undo() }
                    }
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => self.redo(),
                    // View keys (Krita): 1 = 100 %, 2 = fit, 4 / 6 = rotate
                    // by 15 degrees, 5 = reset rotation, + / - = zoom.
                    Key::Character(c) if !ctrl => match c {
                        "1" => self.view_cmd(ViewCmd::ActualSize),
                        "2" => self.view_cmd(ViewCmd::Fit),
                        "4" => self.view_cmd(ViewCmd::RotateLeft),
                        "6" => self.view_cmd(ViewCmd::RotateRight),
                        "5" => self.view_cmd(ViewCmd::ResetRotation),
                        "+" | "=" => self.view_cmd(ViewCmd::ZoomIn),
                        "-" => self.view_cmd(ViewCmd::ZoomOut),
                        _ => {}
                    },
                    _ => {}
                }
            }

            // A release always ends the stroke, even over a dock; otherwise
            // releasing above a panel left the app stuck in drawing mode.
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                if state.is_pressed() {
                    if self.nav.is_some() || self.is_drawing {
                        // Already dragging the view (middle button) or
                        // drawing: a second press starts nothing.
                    } else if over_canvas && self.space_down {
                        // Space + drag: pan; Shift + Space + drag: rotate;
                        // Ctrl + Space + drag: zoom (Krita / CSP).
                        self.begin_nav(MouseButton::Left);
                    } else if over_canvas {
                        self.pressure   = 1.0;
                        self.begin_stroke();
                    }
                } else if self.nav.is_some() {
                    if self.nav_button == MouseButton::Left { self.end_nav(); }
                } else if self.is_drawing {
                    self.end_stroke();
                }
            }

            // Middle drag: pan, Shift + middle drag: rotate, Ctrl + middle
            // drag: zoom.
            WindowEvent::MouseInput { button: MouseButton::Middle, state, .. } => {
                if state.is_pressed() && over_canvas && !self.is_drawing && self.nav.is_none() {
                    self.begin_nav(MouseButton::Middle);
                } else if !state.is_pressed() && self.nav.is_some() && self.nav_button == MouseButton::Middle {
                    self.end_nav();
                }
            }

            // Wheel: zoom toward the cursor.
            WindowEvent::MouseWheel { delta, .. } if over_canvas && !self.is_drawing => {
                use winit::event::MouseScrollDelta;
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                if let Some(gpu) = &mut self.gpu {
                    gpu.view_zoom_at(1.2f32.powf(notches), self.cursor);
                    self.request_redraw();
                }
            }

            WindowEvent::CursorEntered { .. } => {
                self.cursor_in_window = true;
                if let Some(c) = &mut self.cursors { c.invalidate(); }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_in_window = false;
                if let Some(c) = &mut self.cursors { c.invalidate(); }
                self.update_cursor();
                self.request_redraw();
            }

            WindowEvent::CursorMoved { .. } => {
                self.cursor_in_window = true;
                self.activity.cursor_moves += 1;
                // The outline moves with the cursor: a frame is needed.
                if self.over_canvas || self.outline_shown {
                    self.update_cursor();
                    self.request_canvas_frame();
                }
                if self.nav.is_some() {
                    self.drag_nav();
                } else if self.is_drawing && !self.pen_active {
                    self.draw_at(self.cursor.0, self.cursor.1, None);
                }
            }

            // macOS / Linux fallback only: on Windows, drain_stylus already
            // handles the pen, so Touch is ignored there to avoid drawing twice.
            WindowEvent::Touch(touch) if cfg!(not(target_os = "windows")) => {
                use winit::event::{Force, TouchPhase};
                self.pressure = match touch.force {
                    Some(Force::Normalized(f)) => f as f32,
                    Some(Force::Calibrated { force, max_possible_force, .. }) => {
                        (force / max_possible_force) as f32
                    }
                    None => 1.0,
                };
                let pos = (touch.location.x as f32, touch.location.y as f32);
                match touch.phase {
                    TouchPhase::Started => {
                        if self.is_drawing || !self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos)) {
                            return;
                        }
                        self.cursor = pos;
                        self.begin_stroke();
                        if self.is_drawing {
                            self.draw_at(pos.0, pos.1, None);
                        }
                    }
                    TouchPhase::Moved => {
                        if self.is_drawing {
                            self.cursor = pos;
                            self.draw_at(pos.0, pos.1, None);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.is_drawing {
                            self.end_stroke();
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => self.render_frame(),

            _ => {}
        }
    }
}

/// Counts what makes the app work, over a few seconds. Logged when the app
/// keeps drawing frames while no stroke or view drag is going on, to find
/// what wakes it (pen hovering, egui timers, OS events...).
struct Activity {
    since:         Instant,
    ui_frames:     u32,
    canvas_frames: u32,
    events:        u32,
    cursor_moves:  u32,
    pen_packets:   u32,
    timers:        u32,
    wakes:         u32,
    main_time:     Duration,
}

impl Activity {
    const WINDOW: Duration = Duration::from_secs(5);
    /// Main-thread time per window above which the activity is logged
    /// (10 % of a core: a real problem, not the pointer moving).
    const BUSY_TIME: Duration = Duration::from_millis(500);

    fn new() -> Self {
        Self {
            since: Instant::now(), ui_frames: 0, canvas_frames: 0, events: 0, cursor_moves: 0,
            pen_packets: 0, timers: 0, wakes: 0, main_time: Duration::ZERO,
        }
    }

    fn report(&mut self, working: bool) {
        if self.since.elapsed() < Self::WINDOW { return; }
        if !working && self.main_time >= Self::BUSY_TIME {
            log::info!(
                "activity (5 s, no stroke): {} UI frames, {} canvas-only frames, main thread {:.0} ms | \
                 {} window events ({} cursor moves), {} pen packets, {} UI timers, {} wake-ups",
                self.ui_frames, self.canvas_frames, self.main_time.as_secs_f64() * 1000.0,
                self.events, self.cursor_moves, self.pen_packets, self.timers, self.wakes,
            );
        }
        *self = Self::new();
    }
}

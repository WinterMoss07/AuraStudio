use std::path::{Path, PathBuf};
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
use aurastudio_document::{Document, SharedDoc, Workspace};
use aurastudio_file::{EXTENSION, IMAGE_EXTENSIONS};
use aurastudio_layers::{FillParams, Selection};
use aurastudio_ui::{Actions, Dialog, DialogChoice, EditCmd, FileCmd, ImportBrushes, Model, Overlay, SelectCmd, ShapeOptions, Tool, UiState, ViewCmd};

use crate::arc_tool::{ArcTool, Ctx as ArcCtx};
use crate::paint_thread::{self, Cmd, PaintThread, Timing};
use crate::projection::{Projection, SharedProjection};

/// Window title: the name and version.
const APP_TITLE: &str = concat!("AuraStudio ", env!("CARGO_PKG_VERSION"));

/// Shortest time between two frames that only update the UI.
const UI_FRAME: Duration = Duration::from_micros(16_667);

/// Size of the empty stand-in document while the start screen shows.
const PLACEHOLDER: (u32, u32) = (1, 1);
/// Files listed on the start screen.
const MAX_RECENT: usize = 12;
/// Zoom drag speed: the zoom is multiplied by e^(ZOOM_DRAG * dx px).
const ZOOM_DRAG: f32 = 0.007;
/// Unsaved work is saved to the recovery file this long after it changed
/// (AURASTUDIO_AUTOSAVE_SECS overrides it, for testing).
fn autosave_after() -> Duration {
    let secs = std::env::var("AURASTUDIO_AUTOSAVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(5 * 60);
    Duration::from_secs(secs)
}

/// Something that would close the current document: done right away, or
/// after asking to save it.
#[derive(Clone, Debug)]
enum Pending { Create(aurastudio_ui::NewDoc), Open(PathBuf), Close, Quit }

/// Where unsaved work is saved automatically (one file: AuraStudio has one
/// document open at a time for now). One per version, so two versions
/// running side by side (to compare them) don't overwrite or delete each
/// other's autosave.
fn recovery_path() -> Option<PathBuf> {
    let name = format!("autosave-{}.{EXTENSION}", env!("CARGO_PKG_VERSION"));
    dirs::data_dir().map(|d| d.join("AuraStudio").join("recovery").join(name))
}

/// The list of recently used files, one path per line.
fn recent_list_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("AuraStudio").join("recent.txt"))
}

fn load_recent() -> Vec<PathBuf> {
    recent_list_path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Vec::new, |s| {
        s.lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from).take(MAX_RECENT).collect()
    })
}

fn save_recent(list: &[PathBuf]) {
    let Some(path) = recent_list_path() else { return };
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    if let Err(e) = std::fs::write(&path, text) { log::warn!("recent files: {e}"); }
}

/// Settings kept between sessions (key=value lines).
#[derive(Clone, Copy, PartialEq)]
struct Settings {
    /// 100 % zoom follows the display scaling (off: one image pixel per
    /// physical screen pixel).
    hidpi_scaling: bool,
    ui_scale:    f32,
}

impl Default for Settings {
    fn default() -> Self { Self { hidpi_scaling: true, ui_scale: 1.0 } }
}

fn settings_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("AuraStudio").join("settings.txt"))
}

fn load_settings() -> Settings {
    let mut s = Settings::default();
    let text = settings_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    for line in text.lines() {
        match line.split_once('=') {
            Some(("hidpi_image_scaling", v)) => s.hidpi_scaling = v.trim() != "false",
            Some(("ui_scale", v)) => if let Ok(v) = v.trim().parse::<f32>() { s.ui_scale = v.clamp(0.5, 2.0) },
            _ => {}
        }
    }
    s
}

fn save_settings(s: Settings) {
    let Some(path) = settings_path() else { return };
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    let text = format!("hidpi_image_scaling={}\nui_scale={}\n", s.hidpi_scaling, s.ui_scale);
    if let Err(e) = std::fs::write(&path, text) { log::warn!("settings: {e}"); }
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// "5 minutes ago"...
fn ago(t: std::time::SystemTime) -> String {
    let s = t.elapsed().map_or(0, |d| d.as_secs());
    let (n, unit) = match s {
        0..=89 => return "a moment ago".into(),
        90..=5399 => (s / 60, "minute"),
        5400..=129_599 => (s / 3600, "hour"),
        _ => (s / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n > 1 { "s" } else { "" })
}

/// A drag of a tool other than the brush, in progress.
enum Gesture {
    /// Color picker: picks under the cursor as it moves.
    Pick,
    /// Lasso: its outline so far, canvas pixels.
    Lasso(Vec<(f32, f32)>),
    /// Rectangle selection: corner where it started, and the other one.
    Rect((f32, f32), (f32, f32)),
    /// Line, rectangle or ellipse being dragged.
    Shape { tool: Tool, start: (f32, f32), end: (f32, f32) },
}

/// A geometry tool's preview: its path, the handles to show, and the
/// arc's control lines (start, control point, end).
type Preview = (Vec<(f32, f32)>, Vec<(f32, f32)>, Vec<(f32, f32)>);


impl Gesture {
    /// The outline being drawn, canvas pixels.
    fn outline(&self) -> Option<Vec<(f32, f32)>> {
        match self {
            Gesture::Pick | Gesture::Shape { .. } => None,
            Gesture::Lasso(p) => Some(p.clone()),
            Gesture::Rect(a, b) => Some(vec![*a, (b.0, a.1), *b, (a.0, b.1)]),
        }
    }
}

/// View navigation drag in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav { Pan, Rotate, Zoom }

/// Message posted to the event loop (by the app itself or the paint thread).
#[derive(Debug)]
pub enum AppEvent {
    /// Wake the loop so about_to_wait runs and requests the wanted frame.
    Wake,
    /// The paint thread changed some canvas area.
    Painted,
    /// A layer's pixels changed for good (stroke done, undo, clear).
    LayerChanged(u64),
    /// Message for the options bar, from the paint thread.
    Notice(&'static str),
    Message(String),
    /// A start screen thumbnail was decoded.
    Thumbnail { path: PathBuf, w: u32, h: u32, rgba: Vec<u8> },
    /// A save finished (see paint_thread::Event::Saved).
    Saved { doc: aurastudio_document::DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String> },
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
    workspace: Workspace,
    /// Waiting for the answer to "save changes?".
    pending:    Option<Pending>,
    /// Done once the save in progress succeeds.
    after_save: Option<Pending>,
    /// The recovery question is shown (its Discard deletes the autosave).
    recovery_offered: bool,
    /// When the next autosave is due (unsaved work since the last one).
    autosave_at: Option<Instant>,
    /// Document revision in the recovery file.
    autosaved_revision: u64,
    /// Window title shown ("name* — AuraStudio").
    title: String,
    /// Close once about_to_wait runs.
    quit: bool,
    /// No document open: the start screen shows (`doc` is an empty
    /// stand-in).
    home: bool,
    /// Recently opened / saved files, newest first.
    recent: Vec<PathBuf>,
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
    /// Where the button went down, painted as the stroke's first point
    /// with the first mouse move (otherwise a mouse stroke started at its
    /// first move, a few pixels off). Dropped if the pen sends its own.
    press_point: Option<(f32, f32)>,
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
    settings:   Settings,
    /// Last pixels copied here (with where they came from: pasted back in
    /// place while they are still what the system clipboard holds).
    clip:       Option<aurastudio_document::Clip>,
    /// Selected tool, the fill tool's settings.
    tool:       Tool,
    fill:       FillParams,
    shape:      ShapeOptions,
    gesture:    Option<Gesture>,
    /// The arc tool between clicks (arc_tool.rs).
    arc:        ArcTool,

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
        let doc = workspace.new_document("", PLACEHOLDER.0, PLACEHOLDER.1);
        let projection = Arc::new(std::sync::Mutex::new(Projection::new(PLACEHOLDER.0, PLACEHOLDER.1)));
        let worker_proxy = proxy.clone();
        let paint = PaintThread::spawn(Arc::clone(&projection), move |e| {
            let _ = worker_proxy.send_event(match e {
                paint_thread::Event::Painted => AppEvent::Painted,
                paint_thread::Event::LayerChanged { layer_id } => AppEvent::LayerChanged(layer_id),
                paint_thread::Event::Notice(text) => AppEvent::Notice(text),
                paint_thread::Event::Saved { doc, path, revision, auto, result } => AppEvent::Saved { doc, path, revision, auto, result },
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
            pending: None,
            after_save: None,
            recovery_offered: false,
            autosave_at: None,
            autosaved_revision: 0,
            title: String::new(),
            quit: false,
            home: true,
            recent: load_recent(),
            doc,
            paint,
            projection,
            canvas_edit: PLACEHOLDER,
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
            press_point: None,
            cursor:     (0.0, 0.0),
            over_canvas: false,
            cursor_in_window: false,
            outline_shown: false,
            cursors: None,
            settings:   load_settings(),
            clip:       None,
            tool:       Tool::default(),
            fill:       FillParams::default(),
            shape:      ShapeOptions::default(),
            gesture:    None,
            arc:        ArcTool::default(),

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
        let now = self.cursor;
        let Some(gpu) = &mut self.gpu else { return };
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
        let show = self.over_canvas && self.cursor_in_window && !self.space_down && self.nav.is_none()
            && self.tool.paints() && !self.picking_with_alt() && self.gesture.is_none();
        self.outline_shown = show;
        if let Some(gpu) = &mut self.gpu {
            let outline = show.then(|| (self.cursor, self.brush.radius * gpu.zoom()));
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
            None if self.is_drawing => Wanted::Hidden,
            None if self.over_canvas && !self.home => match self.tool {
                _ if self.gesture.is_some() || self.picking_with_alt() => Wanted::System(CursorIcon::Crosshair),
                Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc => Wanted::Hidden,
                Tool::Picker | Tool::Fill | Tool::Lasso | Tool::RectSelect => Wanted::System(CursorIcon::Crosshair),
            },
            None if self.over_canvas => Wanted::System(CursorIcon::Default),
            None => self.gpu.as_ref().map_or(Wanted::System(CursorIcon::Default),
                |g| crate::cursors::from_egui(g.egui_cursor())),
        };
        let (Some(window), Some(cursors)) = (&self.window, &mut self.cursors) else { return };
        cursors.apply(window, wanted);
    }

    /// Follow the scaling of the monitor the window is on. Asked from
    /// Windows directly on every move / resize / focus: winit only updates
    /// its own value on WM_DPICHANGED, which did not come when switching
    /// from the 200 % laptop screen to a 100 % HD screen.
    fn refresh_dpi(&mut self, why: &str) {
        // AURASTUDIO_DPI=96...: pretend Windows reports this DPI (testing).
        let forced = std::env::var("AURASTUDIO_DPI").ok().and_then(|v| v.parse::<u32>().ok());
        let real = forced.or_else(|| self.hwnd().and_then(aurastudio_input::window_dpi)).map(|d| d as f32 / 96.0);
        let Some(gpu) = &mut self.gpu else { return };
        if let Some(scale) = gpu.set_dpi_scale(real) {
            log::info!("display scaling: {:.0} % ({why}; winit: {:.0} %)", scale * 100.0, gpu.winit_scale() * 100.0);
            self.update_cursor();
            self.request_redraw();
        }
    }

    /// Copy / paste pixels, also with other apps through the system
    /// clipboard.
    fn edit_cmd(&mut self, cmd: EditCmd) {
        if self.home || self.is_drawing { return; }
        match cmd {
            EditCmd::Copy => {
                let clip = self.doc.lock().unwrap().copy();
                let Some(clip) = clip else {
                    self.ui.notify("Nothing to copy here (empty, or a folder is selected)");
                    self.request_redraw();
                    return;
                };
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let img = arboard::ImageData {
                        width: clip.width as usize, height: clip.height as usize,
                        bytes: std::borrow::Cow::Borrowed(&clip.rgba),
                    };
                    if let Err(e) = cb.set_image(img) { log::warn!("clipboard: {e}"); }
                }
                self.ui.notify("Copied");
                self.clip = Some(clip);
            }
            EditCmd::Paste => match self.paste_source() {
                Some(clip) => self.paint.send(Cmd::Paste { doc: Arc::clone(&self.doc), clip }),
                None => self.ui.notify("Nothing to paste"),
            },
        }
        self.request_redraw();
    }

    /// What Paste pastes: the system clipboard's image (centered on the
    /// canvas, or in place if it is what we copied last), else our copy.
    fn paste_source(&self) -> Option<aurastudio_document::Clip> {
        let image = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_image().ok());
        let Some(img) = image else { return self.clip.clone() };
        if let Some(c) = &self.clip {
            if (c.width as usize, c.height as usize) == (img.width, img.height) && c.rgba[..] == img.bytes[..] {
                return Some(c.clone());
            }
        }
        let (w, h) = self.canvas_size();
        Some(aurastudio_document::Clip {
            x: (w as i64 - img.width as i64) / 2,
            y: (h as i64 - img.height as i64) / 2,
            width: img.width as u32,
            height: img.height as u32,
            rgba: img.bytes.into_owned(),
        })
    }

    /// Interface size and 100 % zoom meaning, from the settings.
    fn apply_settings(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        gpu.set_pixel_exact(!self.settings.hidpi_scaling);
        gpu.set_ui_scale(self.settings.ui_scale);
        self.update_cursor();
        self.request_redraw();
    }

    /// Alt held with the brush or the fill: a click picks a color (Krita,
    /// CSP, Photoshop).
    fn picking_with_alt(&self) -> bool {
        self.modifiers.alt_key() && matches!(self.tool, Tool::Brush | Tool::Eraser | Tool::Fill)
    }

    /// Left press on the canvas: what the current tool does.
    fn tool_press(&mut self) {
        let tool = if self.picking_with_alt() { Tool::Picker } else { self.tool };
        let at = self.window_to_canvas(self.cursor.0, self.cursor.1);
        match tool {
            Tool::Line | Tool::Rectangle | Tool::Ellipse => {
                self.gesture = at.map(|p| Gesture::Shape { tool, start: p, end: p });
            }
            Tool::Arc => if let Some(p) = at { self.arc_press(p) },
            Tool::Brush | Tool::Eraser => {
                self.pressure = 1.0;
                self.begin_stroke();
            }
            Tool::Picker => {
                self.gesture = Some(Gesture::Pick);
                self.pick_color();
            }
            Tool::Fill => {
                let Some((x, y)) = at else { return };
                let (w, h) = self.canvas_size();
                if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 { return; }
                let [r, g, b] = self.brush.color;
                self.paint.send(Cmd::Fill {
                    doc: Arc::clone(&self.doc), x: x as u32, y: y as u32, color: [r, g, b, 255], params: self.fill,
                });
            }
            Tool::Lasso => self.gesture = at.map(|p| Gesture::Lasso(vec![p])),
            Tool::RectSelect => self.gesture = at.map(|p| Gesture::Rect(p, p)),
        }
        self.update_cursor();
        self.request_redraw();
    }

    /// Cursor moved during a tool drag.
    fn tool_drag(&mut self) {
        let Some(at) = self.window_to_canvas(self.cursor.0, self.cursor.1) else { return };
        let zoom = self.gpu.as_ref().map_or(1.0, |g| g.zoom());
        match &mut self.gesture {
            Some(Gesture::Pick) => self.pick_color(),
            Some(Gesture::Lasso(points)) => {
                // One point per screen pixel or so of travel.
                let last = *points.last().unwrap();
                if (at.0 - last.0).abs().max((at.1 - last.1).abs()) * zoom >= 1.5 { points.push(at); }
            }
            Some(Gesture::Rect(_, end)) | Some(Gesture::Shape { end, .. }) => *end = at,
            None => return,
        }
        self.request_redraw();
    }

    /// Left release: a selection drawn becomes the selection; a click
    /// without dragging deselects (Krita, Photoshop).
    fn tool_release(&mut self) {
        let Some(g) = self.gesture.take() else { return };
        let (w, h) = self.canvas_size();
        let zoom = self.gpu.as_ref().map_or(1.0, |g| g.zoom());
        let sel = match g {
            Gesture::Pick => { self.update_cursor(); return; }
            Gesture::Lasso(points) => Selection::polygon(w, h, &points),
            Gesture::Rect(a, b) if (a.0 - b.0).abs().max((a.1 - b.1).abs()) * zoom >= 3.0 => {
                let r = (a.0.min(b.0).round().max(0.0) as u32, a.1.min(b.1).round().max(0.0) as u32,
                         a.0.max(b.0).round().max(0.0) as u32, a.1.max(b.1).round().max(0.0) as u32);
                Selection::rect(w, h, r)
            }
            Gesture::Rect(..) => None,
            Gesture::Shape { tool, start, end } => {
                // A click without dragging draws nothing.
                if (start.0 - end.0).abs().max((start.1 - end.1).abs()) * zoom >= 2.0 {
                    let path = self.shape_path(tool, start, end);
                    self.paint_path(path);
                }
                self.update_cursor();
                self.request_redraw();
                return;
            }
        };
        self.set_selection(sel);
        self.update_cursor();
    }

    /// Path of a line / rectangle / ellipse dragged from `start` to `end`,
    /// with the modifiers held (Shift: 15° steps, square, circle; Alt:
    /// from the center).
    fn shape_path(&self, tool: Tool, start: (f32, f32), end: (f32, f32)) -> Vec<(f32, f32)> {
        let (shift, alt) = (self.shift_held(), self.modifiers.alt_key());
        match tool {
            Tool::Line => crate::shapes::line(start, if shift { crate::shapes::snap_angle(start, end) } else { end }),
            Tool::Rectangle => { let (a, b) = crate::shapes::drag_box(start, end, shift, alt); crate::shapes::rectangle(a, b) }
            Tool::Ellipse => { let (a, b) = crate::shapes::drag_box(start, end, shift, alt); crate::shapes::ellipse(a, b) }
            _ => Vec::new(),
        }
    }

    /// Geometry tool preview: the path, its handles and control lines.
    fn shape_preview(&self) -> Option<Preview> {
        if let Some(Gesture::Shape { tool, start, end }) = self.gesture {
            return Some((self.shape_path(tool, start, end), Vec::new(), Vec::new()));
        }
        self.arc.preview()
    }

    fn arc_ctx(&self) -> ArcCtx {
        let zoom = self.gpu.as_ref().map_or(1.0, |g| g.zoom());
        ArcCtx { zoom, shift: self.shift_held(), chain_arcs: self.shape.chain_arcs }
    }

    fn arc_press(&mut self, at: (f32, f32)) {
        let cx = self.arc_ctx();
        if let Some(path) = self.arc.press(at, cx, Instant::now()) { self.paint_path(path); }
        self.request_redraw();
    }

    fn arc_release(&mut self, at: (f32, f32)) {
        let cx = self.arc_ctx();
        self.arc.release(at, cx, Instant::now());
        self.request_redraw();
    }

    /// Cursor moved (button up or down) with an arc on the way.
    fn arc_move(&mut self, at: (f32, f32)) {
        let cx = self.arc_ctx();
        if self.arc.moved(at, cx) { self.request_redraw(); }
    }

    /// The outline is done (Enter, leaving the tool...): paint it.
    fn finish_arcs(&mut self) {
        if let Some(path) = self.arc.finish() { self.paint_path(path); }
        self.request_redraw();
    }

    /// Escape: drop the outline in progress, nothing painted.
    fn cancel_arcs(&mut self) {
        self.arc.cancel();
        self.request_redraw();
    }

    /// Ctrl+Z while building an outline: take back its last arc.
    fn undo_arc(&mut self) {
        let at = self.window_to_canvas(self.cursor.0, self.cursor.1).unwrap_or((0.0, 0.0));
        self.arc.undo(at);
        self.request_redraw();
    }

    /// Paint a geometry tool's path as one stroke with the current brush
    /// (full pressure, an even pen speed for speed-sensitive brushes).
    fn paint_path(&mut self, path: Vec<(f32, f32)>) {
        if path.is_empty() { return; }
        if let Some(msg) = self.paint_refusal() {
            self.ui.notify(msg);
            self.request_redraw();
            return;
        }
        let mut brush = self.brush.clone();
        brush.erase = false;
        self.paint.send(Cmd::Begin { doc: Arc::clone(&self.doc), brush, engine: self.library.presets[self.preset].engine.clone() });
        let mut time = self.start.elapsed().as_secs_f64();
        let mut last = path[0];
        for (x, y) in path {
            // 2000 canvas px per second.
            time += (((x - last.0).powi(2) + (y - last.1).powi(2)).sqrt() as f64 / 2000.0).max(0.0005);
            last = (x, y);
            self.paint.send(Cmd::Sample { x, y, pressure: 1.0, time });
        }
        self.paint.send(Cmd::End);
    }

    /// Why the active layer can't be painted, if it can't. try_lock: if
    /// the paint thread is busy, don't wait; it checks again itself.
    fn paint_refusal(&self) -> Option<&'static str> {
        match self.doc.try_lock() {
            Ok(doc) if doc.layers.can_paint() => None,
            Ok(doc) if doc.layers.active_node().is_folder() => Some("Select a layer to paint (a folder is selected)"),
            Ok(_) => Some("Layer is locked"),
            Err(_) => None,
        }
    }

    fn set_selection(&mut self, sel: Option<Selection>) {
        // The paint thread reads it at its next stroke: taken under the
        // document lock (a stroke still painting keeps the lock briefly).
        self.doc.lock().unwrap().selection = sel;
        self.request_redraw();
    }

    fn select(&mut self, cmd: SelectCmd) {
        if self.home || self.is_drawing { return; }
        let (w, h) = self.canvas_size();
        self.set_selection(match cmd {
            SelectCmd::All => Some(Selection::all(w, h)),
            SelectCmd::None => None,
        });
    }

    /// Color picker: the merged image's color under the cursor becomes the
    /// brush color.
    fn pick_color(&mut self) {
        let Some((x, y)) = self.window_to_canvas(self.cursor.0, self.cursor.1) else { return };
        // Never wait for a stroke being painted: skip this sample.
        let Ok(d) = self.doc.try_lock() else { return };
        let (w, h) = (d.layers.width, d.layers.height);
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 { return; }
        let (x, y) = (x as u32, y as u32);
        let px = d.layers.composite_rect((x, y, x + 1, y + 1));
        drop(d);
        if px.len() == 4 && self.brush.color != [px[0], px[1], px[2]] {
            self.brush.color = [px[0], px[1], px[2]];
            self.request_redraw();
        }
    }

    fn canvas_size(&self) -> (u32, u32) {
        let p = self.projection.lock().unwrap();
        (p.width, p.height)
    }

    fn set_tool(&mut self, tool: Tool) {
        if self.is_drawing || self.gesture.is_some() || self.nav.is_some() { return; }
        self.finish_arcs();
        self.arc.reset_release();
        self.tool = tool;
        self.update_cursor();
        self.request_redraw();
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
            ViewCmd::ActualSize    => gpu.view_set_zoom(gpu.zoom_unit(), c),
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
        let wake = [self.repaint_at, self.deferred_frame, self.autosave_at].into_iter().flatten().min();
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None    => ControlFlow::Wait,
        });
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    fn begin_stroke(&mut self) {
        // Instant feedback when the layer can't be painted.
        if let Some(msg) = self.paint_refusal() {
            self.ui.notify(msg);
            self.is_drawing = false;
            self.request_redraw();
            return;
        }
        self.is_drawing = true;
        self.pen_active = false;
        self.samples_sent = 0;
        self.press_point = Some(self.cursor);
        self.frame_stats = Timing::default();
        let mut brush = self.brush.clone();
        brush.erase = self.tool == Tool::Eraser;
        self.paint.send(Cmd::Begin {
            doc:       Arc::clone(&self.doc),
            brush,
            engine:    self.library.presets[self.preset].engine.clone(),
        });
    }

    fn end_stroke(&mut self) {
        // A click without moving sent nothing yet: paint one dot there.
        self.press_point = None;
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
        self.gesture = None;
        if self.arc.active() { self.finish_arcs(); }
        self.arc.reset_release();
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
                self.press_point = None;
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
        let preview = self.shape_preview();
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
        let settings_before = self.settings;
        let mut hidpi_scaling = self.settings.hidpi_scaling;
        let mut ui_scale = self.settings.ui_scale;
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
        let home = self.home;
        let tool_before = self.tool;
        let tool = &mut self.tool;
        let fill = &mut self.fill;
        let drawing = self.gesture.as_ref().and_then(Gesture::outline);
        let shape = &mut self.shape;
        let snap = gpu.view_snapshot();
        let to_points = move |p: (f32, f32)| snap.to_points(p);
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
            let d = &mut *doc;
            let mut model = Model {
                brush,
                presets,
                preset: &mut preset,
                layers: &mut d.layers,
                tool,
                fill,
                has_selection: d.selection.is_some(),
                overlay: Overlay {
                    selection: d.selection.as_ref().map_or(&[], |s| s.outlines()),
                    drawing: drawing.as_deref(),
                    shape: preview.as_ref().map(|p| &p.0[..]),
                    handles: preview.as_ref().map_or(&[], |p| &p.1[..]),
                    guide: preview.as_ref().map_or(&[], |p| &p.2[..]),
                },
                shape,
                to_points: &to_points,
                hidpi_scaling: &mut hidpi_scaling,
                ui_scale: &mut ui_scale,
                can_undo,
                can_redo,
                view_zoom,
                view_angle,
                color_managed,
                perceptual,
                display_profile,
                canvas_edit,
                max_canvas,
                home,
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
        self.settings.hidpi_scaling = hidpi_scaling;
        self.settings.ui_scale = ui_scale;
        if self.settings != settings_before {
            self.apply_settings();
            save_settings(self.settings);
        }
        if self.tool != tool_before {
            self.finish_arcs();
            self.update_cursor();
        }
        if let Some(cmd) = actions.select { self.select(cmd); }
        if let Some(cmd) = actions.edit { self.edit_cmd(cmd); }
        if let Some(kind) = actions.import_brushes { self.import_brushes(kind); }
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
        if actions.modified || actions.recomposite {
            if let Ok(mut d) = self.doc.try_lock() { d.touch(); }
        }
        if let Some(cmd) = actions.view { self.view_cmd(cmd); }
        if actions.undo { self.undo(); }
        if actions.redo { self.redo(); }
        if actions.clear_layer { self.clear_active_layer(); }
        let Some(gpu) = &mut self.gpu else { return };
        if actions.resize_to.is_some() || actions.recomposite || self.layers_dirty {
            // Layer visibility / opacity / order changed, or new size: merge
            // everything again (rare, never during a stroke).
            let mut doc = self.doc.lock().unwrap();
            if let Some((w, h)) = actions.resize_to {
                log::info!("canvas resize: {}x{} -> {w}x{h} (undo history cleared)", doc.layers.width, doc.layers.height);
                doc.resize(w, h);
                *self.projection.lock().unwrap() = Projection::new(w, h);
                gpu.resize_canvas(w, h);
                self.ui.invalidate_all_thumbnails();
            }
            // A stroke being painted keeps merges of the other layers:
            // they just changed.
            doc.layers.drop_stroke_cache();
            // Pieces the paint thread merged before are superseded (it
            // can't merge more meanwhile: the document is locked here).
            self.projection.lock().unwrap().clear();
            upload_all(&doc.layers, gpu);
            self.layers_dirty = false;
            want_frame = true;
        }
        if want_frame {
            self.request_redraw();
        }
    }

    // --- Files -------------------------------------------------------------

    /// Biggest canvas side the GPU takes (before the window exists: a
    /// common limit).
    fn max_canvas(&self) -> u32 {
        self.gpu.as_ref().map_or(16384, |g| g.max_canvas_size())
    }

    fn file_cmd(&mut self, cmd: FileCmd) {
        if self.is_drawing || self.ui.has_dialog() { return; }
        match cmd {
            FileCmd::New => self.ui.open_new_dialog(),
            FileCmd::Open => self.open_dialog(),
            // Nothing to save or close on the start screen.
            _ if self.home => {}
            FileCmd::Save => self.save(false, None),
            FileCmd::SaveAs => self.save(true, None),
            FileCmd::Export => self.export(),
            FileCmd::Close => self.request(Pending::Close),
        }
        self.request_redraw();
    }

    /// Do `action`, first asking to save unsaved changes.
    fn request(&mut self, action: Pending) {
        let (modified, name) = {
            let d = self.doc.lock().unwrap();
            (d.is_modified(), d.name.clone())
        };
        if modified {
            self.pending = Some(action);
            self.ui.show_dialog(Dialog::UnsavedChanges { name });
            self.request_redraw();
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::Create(spec) => {
                let id = self.workspace.reserve_id();
                let mut doc = Document::new(id, &spec.name, spec.width, spec.height);
                doc.dpi = spec.dpi;
                log::info!("new document: {} × {} px, {} dpi", spec.width, spec.height, spec.dpi);
                self.install(doc);
                self.home = false;
            }
            Pending::Open(path) => self.open_path(&path),
            Pending::Close => {
                let id = self.workspace.reserve_id();
                self.install(Document::new(id, "", PLACEHOLDER.0, PLACEHOLDER.1));
                self.home = true;
                self.refresh_home();
            }
            Pending::Quit => {
                // Leaving on purpose: no work to recover next time.
                if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); }
                self.quit = true;
                let _ = self.proxy.send_event(AppEvent::Wake);
            }
        }
    }

    fn on_dialog(&mut self, choice: DialogChoice) {
        if std::mem::take(&mut self.recovery_offered) {
            match choice {
                DialogChoice::Recover => self.recover(),
                _ => if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); },
            }
            return;
        }
        let Some(action) = self.pending.take() else { return };
        match choice {
            DialogChoice::Save => self.save(false, Some(action)),
            DialogChoice::Discard => self.perform(action),
            DialogChoice::Cancel | DialogChoice::Recover => {}
        }
    }

    fn open_dialog(&mut self) {
        let mut all = vec![EXTENSION];
        all.extend(IMAGE_EXTENSIONS);
        let picked = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("AuraStudio documents and images", &all)
            .add_filter("AuraStudio document", &[EXTENSION])
            .add_filter("Images", &IMAGE_EXTENSIONS)
            .pick_file();
        if let Some(path) = picked { self.request(Pending::Open(path)); }
    }

    /// Open an .aurs document, or an image as a new document.
    fn open_path(&mut self, path: &Path) {
        let (id, max) = (self.workspace.reserve_id(), self.max_canvas());
        let t = Instant::now();
        let opened = if has_extension(path, EXTENSION) {
            aurastudio_file::open(path, id, max).map(|o| o.doc)
        } else {
            aurastudio_file::import_image(path, id, max)
        };
        match opened {
            Ok(doc) => {
                log::info!("opened {} in {:.0} ms", path.display(), t.elapsed().as_secs_f64() * 1000.0);
                self.install(doc);
                self.home = false;
                self.add_recent(path);
            }
            Err(e) => {
                log::warn!("open {}: {e}", path.display());
                self.ui.notify(&format!("Can't open {}: {e}", aurastudio_file::doc_name(path)));
                self.request_redraw();
            }
        }
    }

    /// Show `doc` in place of the current document.
    fn install(&mut self, doc: Document) {
        let (w, h) = (doc.layers.width, doc.layers.height);
        let old = self.doc.lock().unwrap().id();
        self.autosaved_revision = doc.revision();
        self.autosave_at = None;
        self.doc = self.workspace.add(doc);
        self.workspace.close(old);
        *self.projection.lock().unwrap() = Projection::new(w, h);
        if let Some(gpu) = &mut self.gpu { gpu.resize_canvas(w, h); }
        self.canvas_edit = (w, h);
        self.ui.invalidate_all_thumbnails();
        self.layers_dirty = true;
        self.request_redraw();
    }

    /// Ctrl+S (`save_as`: Ctrl+Shift+S, or no .aurs file yet: ask where).
    /// `then`: done once saved.
    fn save(&mut self, save_as: bool, then: Option<Pending>) {
        let (path, name) = {
            let d = self.doc.lock().unwrap();
            (d.path.clone(), d.name.clone())
        };
        let path = match path.filter(|p| !save_as && has_extension(p, EXTENSION)) {
            Some(p) => p,
            None => {
                let picked = rfd::FileDialog::new()
                    .set_title("Save")
                    .add_filter("AuraStudio document", &[EXTENSION])
                    .set_file_name(format!("{name}.{EXTENSION}"))
                    .save_file();
                match picked {
                    Some(p) if has_extension(&p, EXTENSION) => p,
                    Some(p) => p.with_extension(EXTENSION),
                    // Cancelled: whatever was to follow doesn't happen.
                    None => return,
                }
            }
        };
        self.after_save = then;
        self.paint.send(Cmd::Save { doc: Arc::clone(&self.doc), path, origin: None, auto: false });
        self.ui.notify("Saving…");
        self.request_redraw();
    }

    fn on_saved(&mut self, doc: aurastudio_document::DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String>) {
        let current = Arc::clone(&self.doc);
        let mut d = current.lock().unwrap();
        // A save of a document closed since: nothing left to update.
        if d.id() != doc { return; }
        match (auto, result) {
            (true, Ok(())) => drop(d),
            (true, Err(e)) => {
                drop(d);
                log::warn!("autosave failed: {e}");
            }
            (false, Ok(())) => {
                if d.path.as_deref() != Some(&path) {
                    d.name = aurastudio_file::doc_name(&path);
                    d.path = Some(path.clone());
                }
                d.mark_saved(revision);
                drop(d);
                // The work is safe in its own file now.
                if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); }
                self.autosaved_revision = revision;
                self.add_recent(&path);
                self.ui.notify(&format!("Saved {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
                if let Some(action) = self.after_save.take() { self.perform(action); }
            }
            (false, Err(e)) => {
                drop(d);
                self.after_save = None;
                self.ui.notify(&format!("Save failed: {e}"));
            }
        }
        self.request_redraw();
    }

    /// The merged image to PNG / JPG (on a thread: big images take a
    /// moment to encode).
    fn export(&mut self) {
        let name = self.doc.lock().unwrap().name.clone();
        let picked = rfd::FileDialog::new()
            .set_title("Export")
            .add_filter("PNG image", &["png"])
            .add_filter("JPEG image", &["jpg", "jpeg"])
            .set_file_name(format!("{name}.png"))
            .save_file();
        let Some(mut path) = picked else { return };
        if !aurastudio_file::EXPORT_EXTENSIONS.iter().any(|e| has_extension(&path, e)) {
            path.set_extension("png");
        }
        // The whole merged image is only made now, for the file, on the
        // export thread (the screen shows it from the GPU).
        let (doc, proxy) = (Arc::clone(&self.doc), self.proxy.clone());
        self.ui.notify("Exporting…");
        std::thread::spawn(move || {
            let (pixels, w, h) = {
                let d = doc.lock().unwrap();
                let (w, h) = (d.layers.width, d.layers.height);
                (d.layers.composite_rect((0, 0, w, h)), w, h)
            };
            let file = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let msg = match aurastudio_file::export_image(&pixels, w, h, &path) {
                Ok(()) => format!("Exported {file}"),
                Err(e) => format!("Export failed: {e}"),
            };
            let _ = proxy.send_event(AppEvent::Message(msg));
        });
        self.request_redraw();
    }

    /// Save unsaved work to the recovery file some time after it changed
    /// (on the paint thread's queue, like any save).
    fn check_autosave(&mut self) {
        let Ok(d) = self.doc.try_lock() else { return };
        let rev = d.revision();
        if !d.is_modified() || rev == self.autosaved_revision {
            self.autosave_at = None;
            return;
        }
        let now = Instant::now();
        match self.autosave_at {
            None => self.autosave_at = Some(now + autosave_after()),
            Some(t) if now >= t && !self.is_drawing => {
                if let Some(path) = recovery_path() {
                    let origin = d.path.clone();
                    self.paint.send(Cmd::Save { doc: Arc::clone(&self.doc), path, origin, auto: true });
                }
                self.autosaved_revision = rev;
                self.autosave_at = None;
            }
            _ => {}
        }
    }

    /// At startup: an autosave left by a session that didn't close
    /// normally? Ask whether to recover it.
    fn offer_recovery(&mut self) {
        let Some(path) = recovery_path().filter(|p| p.is_file()) else { return };
        let origin = aurastudio_file::peek_origin(&path);
        let name = origin.as_deref().map_or_else(|| "Untitled".to_string(), aurastudio_file::doc_name);
        let saved = std::fs::metadata(&path).and_then(|m| m.modified()).map_or_else(|_| "earlier".into(), ago);
        self.recovery_offered = true;
        self.ui.show_dialog(Dialog::Recover { name, saved });
        self.request_redraw();
    }

    fn recover(&mut self) {
        let Some(path) = recovery_path() else { return };
        let (id, max) = (self.workspace.reserve_id(), self.max_canvas());
        match aurastudio_file::open(&path, id, max) {
            Ok(o) => {
                let mut doc = o.doc;
                doc.name = o.origin.as_deref().map_or_else(|| "Untitled".into(), aurastudio_file::doc_name);
                doc.path = o.origin;
                // Unsaved: it isn't in its own file yet.
                doc.touch();
                self.install(doc);
                self.ui.notify("Recovered unsaved work");
            }
            Err(e) => {
                log::warn!("recovery: {e}");
                self.ui.notify(&format!("Can't recover: {e}"));
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Put `path` first in the recent files.
    fn add_recent(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        save_recent(&self.recent);
    }

    /// Recent files on the start screen; their thumbnails are decoded on a
    /// thread and arrive one by one.
    fn refresh_home(&mut self) {
        self.recent.retain(|p| p.is_file());
        let list = self.recent.iter().map(|p| {
            let ext = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            let when = std::fs::metadata(p).and_then(|m| m.modified()).map_or_else(|_| String::new(), ago);
            aurastudio_ui::RecentFile::new(p.clone(), aurastudio_file::doc_name(p), format!("{ext} · {when}"))
        }).collect();
        self.ui.set_recent(list);
        let (paths, proxy) = (self.recent.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            for path in paths {
                if let Some((w, h, rgba)) = aurastudio_file::thumbnail(&path, 280) {
                    let _ = proxy.send_event(AppEvent::Thumbnail { path, w, h, rgba });
                }
            }
        });
        self.request_redraw();
    }

    /// "name* — AuraStudio" (the star: unsaved changes).
    fn update_title(&mut self) {
        let Ok(d) = self.doc.try_lock() else { return };
        let title = if self.home {
            APP_TITLE.to_string()
        } else {
            format!("{}{} — {APP_TITLE}", d.name, if d.is_modified() { "*" } else { "" })
        };
        drop(d);
        if title != self.title {
            if let Some(w) = &self.window { w.set_title(&title); }
            self.title = title;
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
            AppEvent::Message(text) => {
                self.ui.notify(&text);
                self.request_redraw();
            }
            AppEvent::Saved { doc, path, revision, auto, result } => self.on_saved(doc, path, revision, auto, result),
            AppEvent::Thumbnail { path, w, h, rgba } => {
                self.ui.set_recent_thumbnail(&path, w, h, &rgba);
                self.request_redraw();
            }
        }
    }

    /// Called once winit has dispatched every pending OS message, right
    /// before it waits for new ones. WinTab packets arrive as their own
    /// window messages, which produce no winit event: draining here paints
    /// them immediately instead of waiting for the next mouse event.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit {
            event_loop.exit();
            return;
        }
        self.drain_stylus();
        self.check_autosave();
        self.update_title();
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
            .with_title(APP_TITLE)
            .with_window_icon(window_icon())
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
        let window_scale = window.scale_factor();
        window.request_redraw();
        self.window = Some(window);
        self.cursors = Some(crate::cursors::Cursors::new(event_loop));
        self.gpu    = Some(gpu);
        log::info!("display scaling: {:.0} % ({})", window_scale * 100.0,
            self.hwnd().map_or_else(String::new, aurastudio_input::dpi_report));
        self.apply_settings();
        self.refresh_dpi("startup");
        self.refresh_display_profile();
        self.refresh_home();
        self.offer_recovery();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
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
            // Asks to save unsaved changes first.
            WindowEvent::CloseRequested => {
                if !self.ui.has_dialog() { self.request(Pending::Quit); }
            }

            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                self.refresh_dpi("resized");
                self.request_redraw();
            }

            WindowEvent::ModifiersChanged(m) => {
                let alt = self.modifiers.alt_key();
                self.modifiers = m.state();
                // Alt switches the brush to the picker: cursor follows.
                if alt != self.modifiers.alt_key() && self.over_canvas {
                    self.update_cursor();
                    self.request_canvas_frame();
                }
            }

            // A file dropped on the window: opened like with Open.
            WindowEvent::DroppedFile(path) => {
                if self.ui.has_dialog() || self.is_drawing { return; }
                if aurastudio_file::can_open(&path) {
                    self.request(Pending::Open(path));
                } else {
                    self.ui.notify(&format!("Can't open {}: not an image or .aurs file", aurastudio_file::doc_name(&path)));
                    self.request_redraw();
                }
            }

            WindowEvent::Focused(false) => self.reset_input(),
            WindowEvent::Focused(true) => self.refresh_dpi("focused"),

            // The window may now be on another monitor (other profile).
            // The window may be on another monitor now (other profile,
            // other scaling: Windows sends WM_DPICHANGED, which winit turns
            // into ScaleFactorChanged; the picture keeps its size on screen).
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.refresh_dpi("moved");
                self.refresh_display_profile();
            }

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
                if self.ui.has_dialog() { return; }
                match key.logical_key.as_ref() {
                    Key::Character(c) if ctrl && !key.repeat && ["n", "o", "s", "e", "w"].iter().any(|k| c.eq_ignore_ascii_case(k)) => {
                        let cmd = match c.to_ascii_lowercase().as_str() {
                            "n" => Some(FileCmd::New),
                            "w" => Some(FileCmd::Close),
                            "o" => Some(FileCmd::Open),
                            "s" if shift => Some(FileCmd::SaveAs),
                            "s" => Some(FileCmd::Save),
                            "e" if shift => Some(FileCmd::Export),
                            _ => None,
                        };
                        if let Some(cmd) = cmd { self.file_cmd(cmd); }
                    }
                    // The start screen has nothing to edit or move.
                    _ if self.home => {}
                    Key::Named(NamedKey::Delete) if !key.repeat => self.clear_active_layer(),
                    // Ctrl+Z undo, Ctrl+Shift+Z / Ctrl+Y redo (Krita, CSP,
                    // Photoshop). Holding the keys repeats, like everywhere.
                    // While building an arc outline, Ctrl+Z takes back its
                    // last arc (nothing is on the layer yet).
                    Key::Character(c) if ctrl && !shift && c.eq_ignore_ascii_case("z") && self.arc.active() => self.undo_arc(),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
                        if shift { self.redo() } else { self.undo() }
                    }
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => self.redo(),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => self.select(SelectCmd::All),
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("c") => self.edit_cmd(EditCmd::Copy),
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("v") => self.edit_cmd(EditCmd::Paste),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("d") => self.select(SelectCmd::None),
                    Key::Character(c) if !ctrl && !key.repeat && Tool::from_key(c, self.tool).is_some() => {
                        if let Some(t) = Tool::from_key(c, self.tool) { self.set_tool(t); }
                    }
                    // Arc outline: Enter paints it, Escape drops it.
                    Key::Named(NamedKey::Enter) if self.arc.active() => self.finish_arcs(),
                    Key::Named(NamedKey::Escape) if self.arc.active() => self.cancel_arcs(),
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
                    } else if self.ui.has_dialog() {
                        // A question is waiting: the canvas doesn't paint.
                    } else if over_canvas && self.space_down {
                        // Space + drag: pan; Shift + Space + drag: rotate;
                        // Ctrl + Space + drag: zoom (Krita / CSP).
                        self.begin_nav(MouseButton::Left);
                    } else if over_canvas && !self.home && self.gesture.is_none() {
                        self.tool_press();
                    }
                } else if self.nav.is_some() {
                    if self.nav_button == MouseButton::Left { self.end_nav(); }
                } else if self.is_drawing {
                    self.end_stroke();
                } else if self.gesture.is_some() {
                    self.tool_release();
                } else if self.arc.wants_release() {
                    if let Some(at) = self.window_to_canvas(self.cursor.0, self.cursor.1) { self.arc_release(at); }
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
                } else if self.gesture.is_some() {
                    self.tool_drag();
                } else if self.arc.active() {
                    if let Some(at) = self.window_to_canvas(self.cursor.0, self.cursor.1) { self.arc_move(at); }
                } else if self.is_drawing && !self.pen_active {
                    if let Some((x, y)) = self.press_point.take() { self.draw_at(x, y, None); }
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

/// The logo, for the title bar and the taskbar.
fn window_icon() -> Option<winit::window::Icon> {
    let png = include_bytes!("../../../assets/logo/aurastudio-256.png");
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Send the whole merged image to the GPU, in bands of rows: the full
/// image never exists in RAM (a B4 page at 600 dpi is 200 MB).
fn upload_all(layers: &aurastudio_layers::LayerStack, gpu: &mut GpuState) {
    const BAND: u32 = 64;
    let (w, h) = (layers.width, layers.height);
    let mut band = vec![0u8; w as usize * BAND as usize * 4];
    for y in (0..h).step_by(BAND as usize) {
        let rect = (0, y, w, (y + BAND).min(h));
        let rgba = &mut band[..w as usize * (rect.3 - y) as usize * 4];
        layers.composite_rect_to(rect, rgba);
        gpu.upload(rect, rgba);
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

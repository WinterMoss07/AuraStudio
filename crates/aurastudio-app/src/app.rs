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

use aurastudio_forge::{Brush, PenSample};
use aurastudio_forge_library::Library;
use aurastudio_gpu::GpuState;
use aurastudio_input::Stylus;
use aurastudio_color::{monitor_profile, DisplayProfile, DisplayTransform, Intent, LUT_SIZE};
use aurastudio_document::{Document, SelectOp, Shape, SharedDoc, Wand, Workspace};
use aurastudio_file::{EXTENSION, IMAGE_EXTENSIONS};
use aurastudio_layers::{FillParams};
use aurastudio_selection::{SelectMode, Segment};
use aurastudio_transform::Interp;
use aurastudio_ui::{LayerCmd, Actions, Dialog, DialogChoice, EditCmd, FileCmd, Group, ImportBrushes, Loupe, Model, Overlay, PickerOptions, Prefs, SelectCmd, SelectOptions, ShapeOptions, Tool, ToolMemory, TransformCmd, TransformInfo, TransformMode, TransformOptions, TransformOverlay, UiState, ViewCmd};

use crate::arc_tool::{ArcTool, Ctx as ArcCtx};
use crate::debounce::{Debounce, Kind as DebounceKind};
use crate::paint_thread::{self, Cmd, PaintThread, Timing, TransformAction};
use crate::xform::{Hit, Mods, Xform};
use crate::projection::{Projection, SharedProjection};

/// Window title: the name and version.
const APP_TITLE: &str = concat!("AuraStudio ", env!("CARGO_PKG_VERSION"));

/// Shortest time between two frames that only update the UI, and while
/// painting (new paint shows sooner than one display refresh).
const UI_FRAME: Duration = Duration::from_micros(16_667);
const PAINT_FRAME: Duration = Duration::from_millis(4);

/// Size of the empty stand-in document while the start screen shows.
const PLACEHOLDER: (u32, u32) = (1, 1);
/// Files listed on the start screen.
const MAX_RECENT: usize = 12;
/// Zoom drag speed: the zoom is multiplied by e^(ZOOM_DRAG * dx px).
const ZOOM_DRAG: f32 = 0.007;
/// Unsaved work is saved to the recovery file this long after it changed
/// (Preferences; AURASTUDIO_AUTOSAVE_SECS overrides it, for testing).
/// None: never.
fn autosave_after(minutes: u32) -> Option<Duration> {
    let secs = std::env::var("AURASTUDIO_AUTOSAVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(minutes as u64 * 60);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Something that would close the current document: done right away, or
/// after asking to save it.
#[derive(Clone, Debug)]
enum Pending { Create(aurastudio_ui::NewDoc), Open(PathBuf), Close, Quit }

/// Where unsaved work is autosaved: one file per version, so two versions
/// side by side don't overwrite each other's.
pub fn recovery_path() -> Option<PathBuf> {
    let name = format!("autosave-{}.{EXTENSION}", env!("CARGO_PKG_VERSION"));
    dirs::data_dir().map(|d| d.join("AuraStudio").join("recovery").join(name))
}

/// Where imported brushes are kept (AURASTUDIO_BRUSH_DIR, else
/// %APPDATA%\AuraStudio\brushes and the like).
pub fn brush_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AURASTUDIO_BRUSH_DIR") { return Some(PathBuf::from(dir)); }
    dirs::data_dir().map(|d| d.join("AuraStudio").join("brushes"))
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
fn settings_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("AuraStudio").join("settings.txt"))
}

fn load_prefs() -> Prefs {
    Prefs::from_text(&settings_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default())
}

fn save_prefs(p: &Prefs) {
    let Some(path) = settings_path() else { return };
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Err(e) = std::fs::write(&path, p.to_text()) { log::warn!("settings: {e}"); }
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
    Lasso { pts: Vec<(f32, f32)>, mode: SelectMode },
    /// Rectangle / ellipse selection: corner where it started, the other
    /// one; Shift / Alt held at the press (they chose the mode: pressed
    /// later, they make a square, centered).
    Box { ellipse: bool, a: (f32, f32), b: (f32, f32), mode: SelectMode, held: (bool, bool) },
    /// The selection's outline dragged: from, to (canvas pixels).
    MoveSel { start: (f32, f32), now: (f32, f32) },
    /// Line, rectangle or ellipse being dragged.
    Shape { tool: Tool, start: (f32, f32), end: (f32, f32) },
}

/// A geometry tool's preview: its path, the handles to show, and the
/// arc's control lines (start, control point, end).
type Preview = (Vec<(f32, f32)>, Vec<(f32, f32)>, Vec<(f32, f32)>);

/// The transform box to draw: lines, square handles, round handles, pivot.
type XformDrawing = (Vec<Vec<(f32, f32)>>, Vec<(f32, f32)>, Vec<(f32, f32)>, Option<(f32, f32)>);

/// The selection's outline traced for the last selection and zoom.
#[derive(Default)]
struct Ants {
    stamp: u64,
    step:  u32,
    segs:  Vec<Segment>,
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
    /// A layer's pixels changed for good (stroke done, undo, clear), where
    /// (None: anywhere).
    LayerChanged(u64, Option<aurastudio_core::Rect>),
    /// A message for the options bar.
    Notice(String),
    /// A start screen thumbnail was decoded.
    Thumbnail { path: PathBuf, w: u32, h: u32, rgba: Vec<u8> },
    /// A save finished (see paint_thread::Event::Saved).
    Saved { doc: aurastudio_document::DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String> },
    /// The selection changed.
    Selection,
    /// A transform began on the paint thread: what moves, or why not.
    TransformStarted(Result<aurastudio_core::Rect, &'static str>),
    /// Its live preview's pieces.
    TransformLift(Box<aurastudio_document::Lift>),
    /// Transform preview n is on the projection.
    TransformShown(u64),
    /// How the document plays from the GPU (paint_thread::Event::PlayPlan).
    PlayPlan { gen: u64, plan: Option<Box<aurastudio_animation::play::PlayPlan>> },
    /// The tiles of one of its parts.
    PlayTiles { gen: u64, key: aurastudio_gpu::PlayKey, tiles: Box<aurastudio_animation::play::PlayTiles> },
}

/// The GPU preview's mesh for `warp` (lift.rs): a grid over the source
/// and the transparent pixel around it, canvas x, y and texture u, v.
fn lift_mesh(warp: &aurastudio_transform::Warp, src: aurastudio_core::Rect) -> Option<Vec<[f32; 4]>> {
    use aurastudio_transform::Homography;
    let h = Homography::square_to_quad(warp.quad)?;
    let n = aurastudio_gpu::LIFT_GRID;
    let (sw, sh) = ((src.2 - src.0) as f64, (src.3 - src.1) as f64);
    let mut out = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    for j in 0..=n {
        for i in 0..=n {
            // From one pixel before the source to one after.
            let (pu, pv) = (i as f64 / n as f64 * (sw + 2.0) - 1.0, j as f64 / n as f64 * (sh + 2.0) - 1.0);
            let (mut u, mut v) = (pu / sw, pv / sh);
            if let Some(m) = &warp.mesh { [u, v] = m.eval(u, v); }
            let (x, y) = h.apply(u, v);
            if !(x.is_finite() && y.is_finite()) { return None; }
            out.push([x as f32, y as f32, ((pu + 1.0) / (sw + 2.0)) as f32, ((pv + 1.0) / (sh + 2.0)) as f32]);
        }
    }
    Some(out)
}

/// The document locked, if it gets free within `wait`.
fn lock_soon(doc: &SharedDoc, wait: Duration) -> Option<std::sync::MutexGuard<'_, aurastudio_document::Document>> {
    use std::sync::atomic::Ordering;
    if let Ok(d) = doc.try_lock() { return Some(d); }
    // Taken (a stroke, a merge): ask the paint thread to let us in between
    // two of its steps.
    crate::paint_thread::UI_WANTS_DOC.store(true, Ordering::Release);
    let t = Instant::now();
    let got = loop {
        if let Ok(d) = doc.try_lock() { break Some(d); }
        if t.elapsed() >= wait { break None; }
        std::thread::yield_now();
    };
    crate::paint_thread::UI_WANTS_DOC.store(false, Ordering::Release);
    got
}

pub struct App {
    /// Name of the layer edit under way (its undo step, once over).
    layer_label: Option<&'static str>,
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
    /// A file named on the command line ("Open with" in the file
    /// manager), opened once the window is up.
    open_arg: Option<PathBuf>,
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
    /// Pen tilt of the last sample (-1..1 toward +x / +y; 0: mouse).
    tilt:       (f32, f32),
    /// Pen samples sent for the current stroke (0 at release = a click
    /// without moving: paint one dot).
    samples_sent: u32,
    /// Time origin for mouse samples (pen samples carry their own time).
    start:      Instant,
    modifiers:  ModifiersState,
    /// Monitor ICC profile (None: not found, treated as sRGB).
    display_profile: Option<DisplayProfile>,
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
    /// The native pen sent samples this stroke: the CursorMoved events
    /// Windows derives from it are ignored (two offset streams: zigzags).
    pen_active: bool,
    /// Where the button went down: the mouse stroke's first point, sent
    /// with the first move (dropped if the pen sends its own).
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
    prefs:      Prefs,
    /// Last pixels copied here (with where they came from: pasted back in
    /// place while they are still what the system clipboard holds).
    clip:       Option<aurastudio_document::Clip>,
    /// Selected tool, the fill tool's settings.
    tool:       Tool,
    fill:       FillParams,
    /// Brushes stay inside the reference layers' lines (CSP's
    /// anti-overflow).
    stay_inside: bool,
    shape:      ShapeOptions,
    gesture:    Option<Gesture>,
    /// The tool and brush last used in each toolbox group.
    tool_memory: ToolMemory,
    /// The arc tool between clicks (arc_tool.rs).
    arc:        ArcTool,
    /// Left button releases held back BOUNCE (debounce.rs).
    debounce:   Debounce<WindowEvent>,
    /// Selection tools' settings, the polygon being clicked (points, mode).
    select:     SelectOptions,
    picker:     PickerOptions,
    /// The loupe was in the last UI frame.
    loupe_shown: bool,
    poly:       Option<(Vec<(f32, f32)>, SelectMode)>,
    ants:       Ants,
    /// Transform tool: its settings, the transform under way (handles),
    /// one asked for and not started yet.
    transform_opts: TransformOptions,
    xf:         Option<Xform>,
    xf_pending: bool,
    /// The transform as last shown (GPU or paint thread); the paint
    /// thread's preview with this number is the one to wait for.
    xf_seq:     u64,
    /// Animation playback: when it started and from which frame; the next
    /// frame's time; its speed and looping.
    play:       Option<(Instant, u32)>,
    play_next:  Option<Instant>,
    /// Playback from the GPU: the build asked last (results of older ones
    /// are dropped), and the document and revision it is for.
    play_gen:   u64,
    play_built: Option<(aurastudio_document::DocId, u64)>,
    fps:        u32,
    looping:    bool,
    /// The layers asked for when the transform was started.
    pending_targets: Vec<u64>,
    /// The graphics API in use.
    renderer:   String,

    /// The paint thread updated the projection: upload it next frame.
    painted:      bool,
    /// A change outside strokes (visibility, opacity, add/remove/clear
    /// layer, resize) needs a full-canvas recomposite.
    layers_dirty: bool,
    last_frame:   Option<Instant>,
    /// Shortest time between two frames outside strokes: one display
    /// refresh (egui asks for frames "right away" when animating).
    frame_interval: Duration,
    /// A frame wanted before `frame_interval` was over: drawn at this time.
    deferred_frame: Option<Instant>,
    /// Since when the UI couldn't run (document held by the paint thread).
    ui_blocked_since: Option<Instant>,
    /// A frame is needed: drawn in about_to_wait, or right away by
    /// render_if_due.
    redraw_wanted: bool,
    /// The next frame must run the UI. Frames that only move the outline
    /// or show new paint reuse the last UI (a GPU pass, no egui layout).
    ui_dirty:     bool,
    /// When the memory report was last measured (it is shown).
    memory_at:    Option<Instant>,
    /// The cursor was over the canvas at the last full UI frame.
    ui_saw_canvas: bool,
    /// What woke the app lately (logged when it keeps busy idle).
    activity:     Activity,
    /// Delayed frame asked by egui (tooltip, notice fade-out).
    repaint_at:   Option<Instant>,
    /// Main-thread frame cost during the current stroke (logged at its end).
    frame_stats:  Timing,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let library = Library::load(brush_dir());
        log::info!("brushes: {} presets", library.presets.len());
        let mut workspace = Workspace::new();
        let doc = workspace.new_document("", PLACEHOLDER.0, PLACEHOLDER.1);
        let projection = Arc::new(std::sync::Mutex::new(Projection::new(PLACEHOLDER.0, PLACEHOLDER.1)));
        let worker_proxy = proxy.clone();
        let paint = PaintThread::spawn(Arc::clone(&projection), move |e| {
            let _ = worker_proxy.send_event(match e {
                paint_thread::Event::Painted => AppEvent::Painted,
                paint_thread::Event::LayerChanged { layer_id, rect } => AppEvent::LayerChanged(layer_id, rect),
                paint_thread::Event::Notice(text) => AppEvent::Notice(text.into()),
                paint_thread::Event::Saved { doc, path, revision, auto, result } => AppEvent::Saved { doc, path, revision, auto, result },
                paint_thread::Event::Selection => AppEvent::Selection,
                paint_thread::Event::TransformStarted(r) => AppEvent::TransformStarted(r),
                paint_thread::Event::TransformLift(l) => AppEvent::TransformLift(l),
                paint_thread::Event::TransformShown(n) => AppEvent::TransformShown(n),
                paint_thread::Event::PlayPlan { gen, plan } => AppEvent::PlayPlan { gen, plan },
                paint_thread::Event::PlayTiles { gen, key, tiles } => AppEvent::PlayTiles { gen, key, tiles },
            });
        });
        // Start with the round pen (the catalog starts with pencils).
        let first = library.presets.iter().position(|p| p.name == "Round pen").unwrap_or(0);
        Self {
            layer_label: None,
            proxy,
            window: None,
            gpu:    None,
            stylus: None,

            brush:       library.presets[first].settings.clone(),
            library,
            preset:      first,

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
            open_arg: std::env::args_os().nth(1).map(PathBuf::from),
            doc,
            paint,
            projection,
            canvas_edit: PLACEHOLDER,
            ui:          UiState::new(),

            is_drawing: false,
            pressure:   1.0,
            tilt:       (0.0, 0.0),
            samples_sent: 0,
            start:      Instant::now(),
            modifiers:  ModifiersState::empty(),
            display_profile: None,
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
            prefs:      load_prefs(),
            clip:       None,
            tool:       Tool::default(),
            fill:       FillParams::default(),
            stay_inside: false,
            shape:      ShapeOptions::default(),
            gesture:    None,
            tool_memory: ToolMemory::default(),
            arc:        ArcTool::default(),
            debounce:   Debounce::default(),
            select:     SelectOptions::default(),
            picker:     PickerOptions::default(),
            loupe_shown: false,
            poly:       None,
            ants:       Ants::default(),
            transform_opts: TransformOptions::default(),
            xf:         None,
            xf_pending: false,
            xf_seq:     0,
            play:       None,
            play_next:  None,
            play_gen:   0,
            play_built: None,
            fps:        24,
            looping:    true,
            pending_targets: Vec::new(),
            renderer:   String::new(),

            painted:      false,
            // Upload the first composite.
            layers_dirty: true,
            last_frame:   None,
            frame_interval: UI_FRAME,
            deferred_frame: None,
            ui_blocked_since: None,
            redraw_wanted: false,
            ui_dirty:     true,
            memory_at:    None,
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

    /// Fixed point of a zoom drag (where it began, CSP) or rotation drag
    /// (the picture's center if all in view, else the area's).
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

    /// Shift is down, from the modifier state or the keys themselves (with
    /// the pen, the modifier state alone missed Shift).
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

    /// Brush outline under the cursor over the canvas, in place of the OS
    /// cursor (back over the docks and while navigating).
    fn update_outline(&mut self) {
        let show = self.over_canvas && self.cursor_in_window && !self.space_down && self.nav.is_none()
            && self.tool.paints() && !self.picking_with_alt() && self.gesture.is_none() && self.prefs.brush_outline;
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
                Tool::Transform => self.transform_cursor().unwrap_or(Wanted::System(CursorIcon::Default)),
                _ if matches!(self.gesture, Some(Gesture::MoveSel { .. })) => Wanted::System(CursorIcon::Move),
                // The loupe draws its own cross.
                _ if self.loupe_on().is_some() => Wanted::Hidden,
                _ if self.picking_with_alt() || matches!(self.gesture, Some(Gesture::Pick)) => Wanted::Pipette,
                t if t.selects() && t != Tool::Transform && (self.gesture.is_some() || self.poly.is_some()) => Wanted::Cross(self.select_mark()),
                _ if self.gesture.is_some() => Wanted::System(CursorIcon::Crosshair),
                Tool::RectSelect | Tool::EllipseSelect | Tool::Lasso
                    if self.window_to_canvas(self.cursor.0, self.cursor.1).is_some_and(|p| self.on_selection(p)) => Wanted::System(CursorIcon::Move),
                Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc if self.prefs.brush_outline => Wanted::Hidden,
                Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc | Tool::Fill => Wanted::System(CursorIcon::Crosshair),
                Tool::Picker => Wanted::Pipette,
                Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect | Tool::PolySelect | Tool::MagicWand => Wanted::Cross(self.select_mark()),
            },
            None if self.over_canvas => Wanted::System(CursorIcon::Default),
            None => self.gpu.as_ref().map_or(Wanted::System(CursorIcon::Default),
                |g| crate::cursors::from_egui(g.egui_cursor())),
        };
        let (Some(window), Some(cursors)) = (&self.window, &mut self.cursors) else { return };
        cursors.apply(window, wanted);
    }

    /// Follow the scaling of the window's monitor, asked from Windows on
    /// every move / resize / focus (winit missed a 200 % -> 100 % move).
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
        self.apply_onion_look();
        let Some(gpu) = &mut self.gpu else { return };
        gpu.set_pixel_exact(!self.prefs.hidpi_scaling);
        gpu.set_ui_scale(self.prefs.ui_scale);
        gpu.set_quality(gpu_quality(self.prefs.canvas_quality));
        gpu.set_smooth_zoom_in(self.prefs.smooth_zoom_in);
        gpu.set_vsync(self.prefs.vsync);
        self.debounce.window = Duration::from_millis(self.prefs.debounce_ms as u64);
        self.update_cursor();
        self.request_redraw();
    }

    /// The onion skin's colors and fade from the preferences, on the
    /// document (shown again if they changed).
    fn apply_onion_look(&mut self) {
        let p = &self.prefs;
        let look = aurastudio_layers::onion::OnionLook { before: p.onion_before, after: p.onion_after, thickness: p.onion_thickness, by_frames: p.onion_by_frames };
        let Ok(mut d) = self.doc.try_lock() else { return };
        if d.layers.onion_look() == look { return; }
        d.layers.set_onion_look(look);
        let shown = d.layers.onion().enabled;
        drop(d);
        if shown { self.paint.send(Cmd::Recomposite(Arc::clone(&self.doc))); }
    }

    /// Alt held with the brush or the fill: a click picks a color (Krita,
    /// CSP, Photoshop).
    fn picking_with_alt(&self) -> bool {
        self.modifiers.alt_key() && matches!(self.tool, Tool::Brush | Tool::Eraser | Tool::Fill)
    }

    /// The loupe shows around the cursor (picker or selection tools with
    /// it on): a pixel taken (picker, wand) or a pixel corner aimed at.
    fn loupe_on(&self) -> Option<bool> {
        if !self.over_canvas || !self.cursor_in_window || self.home || self.space_down || self.nav.is_some() || self.is_drawing { return None; }
        if matches!(self.gesture, Some(Gesture::MoveSel { .. })) { return None; }
        match self.tool {
            Tool::Picker if self.picker.loupe => Some(true),
            Tool::MagicWand if self.select.loupe => Some(true),
            Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect | Tool::PolySelect if self.select.loupe => {
                // Over the selection, a press moves it: no loupe.
                if self.gesture.is_none() && self.poly.is_none() && self.window_to_canvas(self.cursor.0, self.cursor.1).is_some_and(|p| self.on_selection(p)) { return None; }
                Some(false)
            }
            _ => None,
        }
    }

    /// The canvas point a selection tool takes: on the pixel corner
    /// nearest the cursor with the loupe on.
    fn select_at(&self) -> Option<(f32, f32)> {
        let p = self.window_to_canvas(self.cursor.0, self.cursor.1)?;
        Some(if self.select.loupe { (p.0.round(), p.1.round()) } else { p })
    }

    /// The selection mode's sign by the cursor.
    fn select_mark(&self) -> crate::cursors::Mark {
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
    fn on_selection(&self, p: (f32, f32)) -> bool {
        if self.select_mode() != SelectMode::Replace || self.select.mode != SelectMode::Replace { return false; }
        let Ok(d) = self.doc.try_lock() else { return false };
        d.selection.as_ref().is_some_and(|s| p.0 >= 0.0 && p.1 >= 0.0 && s.inside(p.0 as u32, p.1 as u32))
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
                self.tilt = (0.0, 0.0);
                self.begin_stroke();
            }
            Tool::Picker => {
                self.gesture = Some(Gesture::Pick);
                self.pick_color();
            }
            Tool::Fill => {
                let Some((x, y)) = at.and_then(|p| pixel_in(p, self.canvas_size())) else { return };
                let [r, g, b] = self.brush.color;
                self.paint.send(Cmd::Fill {
                    doc: Arc::clone(&self.doc), x, y, color: [r, g, b, 255], params: self.fill,
                });
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
    fn tool_drag(&mut self) {
        let Some(raw) = self.window_to_canvas(self.cursor.0, self.cursor.1) else { return };
        let zoom = self.zoom();
        let snapped = self.select_at().unwrap_or(raw);
        let at = if matches!(self.gesture, Some(Gesture::Lasso { .. } | Gesture::Box { .. })) { snapped } else { raw };
        match &mut self.gesture {
            Some(Gesture::Pick) => self.pick_color(),
            Some(Gesture::Lasso { pts, .. }) => {
                // One point per screen pixel or so of travel.
                let last = pts.last().copied().unwrap_or(at);
                if (at.0 - last.0).abs().max((at.1 - last.1).abs()) * zoom >= 1.5 { pts.push(at); }
            }
            Some(Gesture::Box { b: end, .. } | Gesture::Shape { end, .. } | Gesture::MoveSel { now: end, .. }) => *end = at,
            None => return,
        }
        self.request_redraw();
    }

    /// The box a rectangle / ellipse selection covers: Shift pressed after
    /// the press makes it square, Alt centers it.
    fn select_box(&self, a: (f32, f32), b: (f32, f32), held: (bool, bool)) -> ((f32, f32), (f32, f32)) {
        let square = self.shift_held() && !held.0;
        let centered = self.modifiers.alt_key() && !held.1;
        crate::shapes::drag_box(a, b, square, centered)
    }

    /// What the selection gesture outlines now, canvas pixels.
    fn gesture_outline(&self) -> Option<Vec<(f32, f32)>> {
        if self.gesture.is_none() && self.poly.is_some() {
            // The polygon so far, and on to the cursor.
            let (pts, _) = self.poly.as_ref()?;
            let mut v = pts.clone();
            if let Some(c) = self.select_at() { v.push(c); }
            return Some(v);
        }
        match self.gesture.as_ref()? {
            Gesture::Lasso { pts, .. } => Some(pts.clone()),
            Gesture::Box { ellipse, a, b, held, .. } => {
                let (p, q) = self.select_box(*a, *b, *held);
                Some(if *ellipse { crate::shapes::ellipse(p, q) } else { vec![p, (q.0, p.1), q, (p.0, q.1)] })
            }
            _ => None,
        }
    }

    /// Left release: a selection drawn becomes the selection; a click
    /// without dragging deselects (Krita, Photoshop).
    fn tool_release(&mut self) {
        let Some(g) = self.gesture.take() else { return };
        let zoom = self.zoom();
        let far = |a: (f32, f32), b: (f32, f32), min: f32| (a.0 - b.0).abs().max((a.1 - b.1).abs()) * zoom >= min;
        let (aa, feather) = (self.select.aa, self.select.feather);
        let op = match g {
            Gesture::Pick => { self.update_cursor(); return; }
            Gesture::MoveSel { start, now } => {
                let (dx, dy) = ((now.0 - start.0).round() as i64, (now.1 - start.1).round() as i64);
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
                    Some(SelectOp::Shape { shape: Shape::Ellipse((p.0, p.1, q.0, q.1)), mode, aa, feather })
                } else {
                    let r = |v: f32| v.round().max(0.0) as u32;
                    Some(SelectOp::Shape { shape: Shape::Rect((r(p.0.min(q.0)), r(p.1.min(q.1)), r(p.0.max(q.0)), r(p.1.max(q.1)))), mode, aa, feather })
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
    fn poly_click(&mut self, p: (f32, f32)) {
        let zoom = self.zoom();
        match &mut self.poly {
            None => {
                let mode = self.select_mode();
                self.poly = Some((vec![p], mode));
            }
            Some((pts, _)) => {
                let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1) * zoom <= 8.0;
                if pts.len() >= 3 && close(p, pts[0]) { self.finish_poly(); return; }
                // A second click on the last point: done.
                if pts.last().is_some_and(|l| close(p, *l)) { if pts.len() >= 3 { self.finish_poly(); } return; }
                pts.push(p);
            }
        }
        self.request_redraw();
    }

    fn finish_poly(&mut self) {
        let Some((pts, mode)) = self.poly.take() else { return };
        if pts.len() >= 3 {
            let op = SelectOp::Shape { shape: Shape::Polygon(pts), mode, aa: self.select.aa, feather: self.select.feather };
            self.send_select(op);
        }
        self.request_redraw();
    }

    fn send_select(&mut self, op: SelectOp) {
        self.paint.send(Cmd::Select { doc: Arc::clone(&self.doc), op });
        self.request_redraw();
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
        let zoom = self.zoom();
        ArcCtx { zoom, shift: self.shift_held(), multipoint: self.shape.multipoint }
    }

    fn arc_press(&mut self, at: (f32, f32)) {
        let cx = self.arc_ctx();
        if let Some(path) = self.arc.press(at, cx) { self.paint_path(path); }
        self.request_redraw();
    }

    fn arc_release(&mut self, at: (f32, f32)) {
        let cx = self.arc_ctx();
        self.arc.release(at, cx);
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
        self.send_begin(false);
        let mut time = self.start.elapsed().as_secs_f64();
        let mut last = path[0];
        for (x, y) in path {
            // 2000 canvas px per second.
            time += (((x - last.0).powi(2) + (y - last.1).powi(2)).sqrt() as f64 / 2000.0).max(0.0005);
            last = (x, y);
            self.paint.send(Cmd::Sample(PenSample::new(x, y, 1.0, time)));
        }
        self.paint.send(Cmd::End);
    }

    /// Why the active layer can't be painted (paint thread busy: don't
    /// wait, it checks too).
    fn paint_refusal(&self) -> Option<&'static str> {
        match self.doc.try_lock() {
            Ok(doc) if doc.layers.can_paint() => None,
            Ok(doc) if doc.layers.anim_of(doc.layers.active_id()).is_some() => Some("This cel or sheet is locked"),
            Ok(doc) if doc.layers.active_node().is_folder() => Some("Select a layer to paint (a folder is selected)"),
            Ok(_) => Some("Layer is locked"),
            Err(_) => None,
        }
    }

    fn select(&mut self, cmd: SelectCmd) {
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

    // --- Transform tool ------------------------------------------------------

    /// The layers the transform tool takes: those chosen in the Layers
    /// docker, the paint layers of a chosen folder.
    fn transform_targets(&self) -> Vec<u64> {
        let Ok(d) = self.doc.try_lock() else { return Vec::new() };
        let mut out = Vec::new();
        for id in self.ui.chosen_layers(&d.layers) {
            if let Some(n) = d.layers.node(id) { n.push_layer_ids(&mut out); }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Ask the paint thread to start a transform (its box shows once it
    /// answers).
    fn start_transform(&mut self) {
        if self.home || self.xf.is_some() || self.xf_pending { return; }
        let targets = self.transform_targets();
        if targets.is_empty() {
            self.ui.notify("Select a layer to transform");
            self.request_redraw();
            return;
        }
        self.xf_pending = true;
        self.pending_targets = targets.clone();
        self.paint.send(Cmd::Transform { doc: Arc::clone(&self.doc), action: TransformAction::Begin(targets) });
    }

    /// Keep (or drop) the transform under way.
    fn end_transform(&mut self, apply: bool) {
        self.xf_pending = false;
        let Some(xf) = self.xf.take() else { return };
        // (The canvas keeps the GPU's last picture until the result comes.)
        if let Some(gpu) = &mut self.gpu { gpu.lift_end(); }
        let action = if apply && xf.changed() { TransformAction::Apply(xf.warp, self.transform_opts.interp) } else { TransformAction::Cancel };
        self.paint.send(Cmd::Transform { doc: Arc::clone(&self.doc), action });
        self.update_cursor();
        self.request_redraw();
    }

    /// Show the transform as it is now: drawn by the GPU at once; `fast`
    /// (a handle dragged): only that, else the paint thread's exact result
    /// follows (bilinear while dragging when the GPU can't).
    fn send_preview(&mut self, fast: bool) {
        let on_gpu = self.lift_show();
        self.xf_seq += 1;
        self.request_redraw();
        if fast && on_gpu { return; }
        let Some(xf) = &self.xf else { return };
        let interp = match self.transform_opts.interp { Interp::Nearest => Interp::Nearest, i if !fast => i, _ => Interp::Bilinear };
        self.paint.send(Cmd::Transform { doc: Arc::clone(&self.doc), action: TransformAction::Preview(xf.warp.clone(), interp, self.xf_seq) });
    }

    /// Draw the transform on the GPU (lift.rs), drawn over the canvas
    /// until the paint thread's matching preview is in. False: not ready.
    fn lift_show(&mut self) -> bool {
        let (Some(xf), Some(gpu)) = (&self.xf, &mut self.gpu) else { return false };
        if !gpu.lift_ready(xf.src) { return false; }
        let Some(verts) = lift_mesh(&xf.warp, xf.src) else { return false };
        gpu.lift_show(&verts, self.transform_opts.interp == Interp::Nearest, true);
        true
    }

    /// Handles' reach, canvas pixels.
    fn handle_tolerance(&self) -> f64 {
        9.0 / self.zoom().max(1e-3) as f64
    }

    fn xform_press(&mut self, p: (f32, f32)) {
        let mode = self.transform_opts.mode;
        let tol = self.handle_tolerance();
        let Some(xf) = &mut self.xf else {
            // None yet (empty layer, cancelled): try again.
            self.start_transform();
            return;
        };
        if mode == TransformMode::Warp && xf.warp.mesh.is_none() { xf.set_grid(self.transform_opts.grid); }
        let hit = xf.hit([p.0 as f64, p.1 as f64], tol, mode);
        xf.press(hit, [p.0 as f64, p.1 as f64]);
    }

    fn xform_mods(&self) -> Mods {
        Mods { shift: self.shift_held(), alt: self.modifiers.alt_key(), ctrl: self.ctrl_held() }
    }

    /// The pointer moved with the transform's handle held.
    fn xform_drag(&mut self) {
        let Some(at) = self.window_to_canvas(self.cursor.0, self.cursor.1) else { return };
        let (m, mode, keep) = (self.xform_mods(), self.transform_opts.mode, self.transform_opts.keep_aspect);
        let Some(xf) = &mut self.xf else { return };
        if xf.drag.is_none() { return; }
        if xf.drag_to([at.0 as f64, at.1 as f64], m, mode, keep) { self.send_preview(true); }
    }

    fn xform_release(&mut self) {
        let Some(xf) = &mut self.xf else { return };
        if xf.release() { self.send_preview(false); }
        self.update_cursor();
    }

    /// Other layers chosen while transforming: the transform so far is
    /// kept, a new one starts on them.
    fn follow_transform_targets(&mut self) {
        if self.tool != Tool::Transform || self.is_drawing || self.xf.as_ref().is_some_and(|x| x.drag.is_some()) { return; }
        let now = match (&self.xf, self.xf_pending) {
            (Some(x), _) => x.targets.clone(),
            (None, true) => self.pending_targets.clone(),
            (None, false) => return,
        };
        let targets = self.transform_targets();
        if !targets.is_empty() && targets != now {
            self.end_transform(true);
            self.start_transform();
        }
    }

    /// The options' buttons and fields.
    fn transform_cmd(&mut self, cmd: TransformCmd) {
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
    fn transform_overlay_data(&self) -> Option<XformDrawing> {
        let xf = self.xf.as_ref()?;
        let mode = self.transform_opts.mode;
        let f = |p: [f64; 2]| (p[0] as f32, p[1] as f32);
        let lines = xf.lines(mode);
        // A small box on screen shows its corners only (see Xform::hit).
        let q = xf.warp.quad;
        let small = (0..4).map(|i| (q[(i + 1) % 4][0] - q[i][0]).hypot(q[(i + 1) % 4][1] - q[i][1])).fold(f64::MAX, f64::min) < self.handle_tolerance() * 5.0;
        let (handles, points) = match mode {
            TransformMode::Warp => (Vec::new(), xf.mesh_points().into_iter().map(f).collect()),
            _ if small => (q.iter().copied().map(f).collect(), Vec::new()),
            _ => (q.iter().copied().chain(xf.edge_handles()).map(f).collect(), Vec::new()),
        };
        let pivot = (mode == TransformMode::Free && !small).then(|| f(xf.pivot));
        Some((lines, handles, points, pivot))
    }

    /// The cursor over the transform box: what a press there would do.
    fn transform_cursor(&self) -> Option<crate::cursors::Wanted> {
        use crate::cursors::Wanted;
        let xf = self.xf.as_ref()?;
        let at = self.window_to_canvas(self.cursor.0, self.cursor.1)?;
        let mode = self.transform_opts.mode;
        let hit = match &xf.drag { Some(d) => d.hit, None => xf.hit([at.0 as f64, at.1 as f64], self.handle_tolerance(), mode) };
        let dragging = xf.drag.is_some();
        // Resize arrows along the handle's direction on screen.
        let gpu = self.gpu.as_ref()?;
        let c = { let q = xf.warp.quad; [(q[0][0] + q[2][0]) / 2.0, (q[0][1] + q[2][1]) / 2.0] };
        let dir = |p: [f64; 2]| {
            let (a, b) = (gpu.canvas_to_screen((c[0] as f32, c[1] as f32)), gpu.canvas_to_screen((p[0] as f32, p[1] as f32)));
            let ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees().rem_euclid(180.0);
            match ang { a if !(22.5..157.5).contains(&a) => CursorIcon::EwResize, a if a < 67.5 => CursorIcon::NwseResize, a if a < 112.5 => CursorIcon::NsResize, _ => CursorIcon::NeswResize }
        };
        // A skew slides along the edge.
        let skew = |i: usize| {
            let q = xf.warp.quad;
            dir([c[0] + q[(i + 1) % 4][0] - q[i][0], c[1] + q[(i + 1) % 4][1] - q[i][1]])
        };
        Some(match hit {
            Hit::Inside => Wanted::System(CursorIcon::Move),
            Hit::Outside => Wanted::Rotate,
            Hit::Pivot => Wanted::System(CursorIcon::Crosshair),
            Hit::Corner(_) if mode == TransformMode::Perspective || self.ctrl_held() => Wanted::System(CursorIcon::Crosshair),
            Hit::Corner(i) => Wanted::System(dir(xf.warp.quad[i])),
            Hit::Side(i) => Wanted::System(skew(i)),
            Hit::Edge(i) if self.ctrl_held() => Wanted::System(skew(i)),
            Hit::Edge(i) => Wanted::System(dir(xf.edge_handles()[i])),
            Hit::Point(_) => if dragging { Wanted::HandClosed } else { Wanted::Hand },
        })
    }

    /// Color picker: the merged image's color under the cursor becomes the
    /// brush color.
    fn pick_color(&mut self) {
        let Some(p) = self.window_to_canvas(self.cursor.0, self.cursor.1) else { return };
        // Never wait for a stroke being painted: skip this sample.
        let Ok(d) = self.doc.try_lock() else { return };
        let Some((x, y)) = pixel_in(p, (d.layers.width, d.layers.height)) else { return };
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

    /// A toolbox group's shortcut: its last tool and brush (pressed again:
    /// its next tool).
    fn select_group(&mut self, g: Group) {
        if self.is_drawing || self.gesture.is_some() || self.nav.is_some() { return; }
        let (mut tool, mut preset) = (self.tool, self.preset);
        self.tool_memory.select(g, &mut tool, &mut preset, &self.library.presets);
        self.select_preset(preset);
        self.set_tool(tool);
        self.tool_memory.remember(self.tool, self.preset, &self.library.presets);
    }

    fn set_tool(&mut self, tool: Tool) {
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
        let intent = if self.prefs.perceptual { Intent::Perceptual } else { Intent::RelativeColorimetric };
        let transform = match (&self.display_profile, self.prefs.color_managed) {
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

    /// Ask for a frame (drawn in about_to_wait; posting Wake makes sure the
    /// loop gets there right away).
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
        let wake = [self.repaint_at, self.deferred_frame, self.autosave_at, self.debounce.deadline(), self.play_next].into_iter().flatten().min();
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None    => ControlFlow::Wait,
        });
    }

    /// Play the animation from the current frame (from the first if at
    /// the end), at `fps`.
    fn start_playback(&mut self) {
        if self.is_drawing || self.xf.is_some() || self.xf_pending { return; }
        let Ok(mut doc) = self.doc.try_lock() else { return };
        if doc.kind != aurastudio_document::Kind::Animation { return; }
        let rewind = doc.layers.time() >= doc.layers.last_frame();
        if rewind { doc.layers.set_time(1); }
        let from = doc.layers.time();
        let key = (doc.id(), doc.revision());
        // The onion skin during playback only if asked (then merged: the
        // GPU tiles have no ghosts).
        let ghosts = self.prefs.onion_playback && doc.layers.onion().enabled;
        if !ghosts { doc.layers.pause_onion(true); }
        drop(doc);
        if rewind { self.paint.send(Cmd::Recomposite(Arc::clone(&self.doc))); }
        if !ghosts { self.ensure_play_cache(key); }
        let now = Instant::now();
        self.play = Some((now, from));
        self.play_next = Some(now);
        self.request_redraw();
        crate::memory::event("play start");
    }

    fn stop_playback(&mut self) {
        if self.play.take().is_some() {
            // The exact merge of the frame stopped on (the GPU's frames may
            // differ by a level here and there), with its onion skin.
            if let Ok(mut d) = self.doc.lock() { d.layers.pause_onion(false); }
            self.paint.send(Cmd::Recomposite(Arc::clone(&self.doc)));
            self.request_redraw();
            crate::memory::event("play stop");
        }
        self.play_next = None;
    }

    /// The document shows its onion skin (marks on).
    fn onion_on(&self) -> bool { self.doc.try_lock().is_ok_and(|d| d.layers.onion().enabled) }

    /// Have the GPU tiles of document `key` (id, revision) made, unless
    /// they are already (or being) made for it.
    fn ensure_play_cache(&mut self, key: (aurastudio_document::DocId, u64)) {
        if self.play_built == Some(key) { return; }
        self.drop_play_cache();
        self.play_built = Some(key);
        self.paint.send(Cmd::PlayBuild { doc: Arc::clone(&self.doc), gen: self.play_gen });
    }

    /// The document changed: the playback tiles no longer show it.
    fn drop_play_cache(&mut self) {
        self.play_gen += 1;
        self.play_built = None;
        if let Some(gpu) = &mut self.gpu { gpu.play_end(); }
    }

    /// Playing: show the frame of now (frames are skipped if the merge
    /// can't keep up, the time stays right); loop or stop at the last
    /// frame drawn.
    fn tick_playback(&mut self) {
        let Some((start, from)) = self.play else { return };
        let now = Instant::now();
        if self.play_next.is_some_and(|t| now < t) { return; }
        let fps = self.fps.clamp(1, 120) as f64;
        let step = (now.duration_since(start).as_secs_f64() * fps) as u64;
        let Ok(mut doc) = self.doc.try_lock() else {
            self.play_next = Some(now + Duration::from_millis(2));
            return;
        };
        let last = doc.layers.last_frame() as u64;
        let mut t = from as u64 + step;
        let mut stop = false;
        if t > last {
            if self.looping && last > 1 { t = 1 + (t - 1) % last; } else { t = last; stop = true; }
        }
        let changed = doc.layers.time() as u64 != t;
        if changed { doc.layers.set_time(t as u32); }
        let key = (doc.id(), doc.revision());
        drop(doc);
        if changed {
            // From the GPU tiles when they are there (no onion skin shown),
            // else merged.
            let gpu_ok = !(self.prefs.onion_playback && self.onion_on());
            if gpu_ok { self.ensure_play_cache(key); }
            let shown = gpu_ok && self.gpu.as_mut().is_some_and(|g| g.play_show(t as u32));
            if !shown { self.paint.send(Cmd::Recomposite(Arc::clone(&self.doc))); }
        }
        self.request_redraw();
        if stop { self.stop_playback(); return; }
        self.play_next = Some(start + Duration::from_secs_f64((step + 1) as f64 / fps));
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    fn begin_stroke(&mut self) {
        // (Drawing stops the playback, on the frame shown.)
        self.stop_playback();
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
        self.send_begin(self.tool == Tool::Eraser);
    }

    /// Open a stroke on the paint thread with the current brush.
    fn send_begin(&self, erase: bool) {
        self.paint.send(Cmd::Begin {
            doc:   Arc::clone(&self.doc),
            brush: Brush { erase, ..self.brush.clone() },
            forge: self.library.presets[self.preset].brush.clone(),
            limit: self.stroke_limit(),
        });
    }

    /// "Stay inside the lines": the fill tool's tolerance and drafts
    /// setting, its expand (at least 1 px: under the lines' soft edge).
    fn stroke_limit(&self) -> Option<FillParams> {
        self.stay_inside.then_some(FillParams { expand: self.fill.expand.max(1), ..self.fill })
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
        self.paint.send(Cmd::Sample(PenSample { tilt: self.tilt, ..PenSample::new(x, y, self.pressure, time) }));
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
        self.xform_release();
        if self.arc.active() { self.finish_arcs(); }
        self.arc.reset_release();
        if self.is_drawing {
            self.end_stroke();
        }
        self.update_cursor();
        self.request_redraw();
    }

    /// Native pen samples (WinTab or Windows Ink): position and pressure;
    /// press / release come from MouseInput.
    fn drain_stylus(&mut self) {
        // A release held back (debounce.rs): the pen may be up.
        if self.debounce.holding() { return; }
        let Some(stylus) = &self.stylus else { return };
        for s in stylus.drain() {
            self.activity.pen_packets += 1;
            if self.is_drawing {
                self.pen_active = true;
                self.press_point = None;
                self.pressure   = s.pressure;
                self.tilt       = s.tilt;
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

    /// Render now when a frame is due (Windows sends WM_PAINT only once the
    /// queue is empty: pen packets kept it from ever coming). New paint:
    /// every 4 ms at most; else one frame per refresh.
    fn render_if_due(&mut self) {
        let min = if self.painted { PAINT_FRAME } else { self.frame_interval };
        if !self.painted && !self.redraw_wanted { return; }
        if self.last_frame.is_some_and(|t| t.elapsed() < min) { return; }
        self.redraw_wanted = false;
        self.deferred_frame = None;
        self.render_frame();
    }

    /// Window > Memory report open: measure once a second (the document
    /// is skipped while the paint thread has it) and show it.
    fn tick_memory(&mut self) {
        if !self.ui.memory_wanted() {
            self.memory_at = None;
            return;
        }
        let now = Instant::now();
        if self.memory_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) { return; }
        let gpu = self.gpu.as_ref().map(GpuState::memory);
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

    fn render_frame(&mut self) {
        self.update_outline();
        let preview = self.shape_preview();
        let drawing = self.gesture_outline();
        let ants_offset = match &self.gesture { Some(Gesture::MoveSel { start, now }) => ((now.0 - start.0).round(), (now.1 - start.1).round()), _ => (0.0, 0.0) };
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
        let prefs_before = self.prefs.clone();
        let mut prefs = self.prefs.clone();
        let display_profile = self.display_profile.as_ref().map(|p| p.name.as_str());
        let ui_state   = &mut self.ui;
        let doc        = &self.doc;
        let brush      = &mut self.brush;
        let presets    = &self.library.presets[..];
        let mediums    = &self.library.mediums[..];
        let preset_before = self.preset;
        let mut preset = self.preset;
        let canvas_edit = &mut self.canvas_edit;
        let home = self.home;
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
        let fps = &mut self.fps;
        let looping = &mut self.looping;
        let renderer = self.renderer.as_str();
        let shape = &mut self.shape;
        let snap = gpu.view_snapshot();
        let to_points = move |p: (f32, f32)| snap.to_points(p);
        let mut actions = Actions::default();
        // Never wait for the brush: document busy, redraw with the last UI
        // and try again in a moment.
        let mut doc_busy = false;
        let run_ui = std::mem::take(&mut self.ui_dirty) || !gpu.has_cached_ui();
        let ui_saw_canvas = &mut self.ui_saw_canvas;
        let layer_label = &mut self.layer_label;
        let over_canvas = self.over_canvas;
        if run_ui { self.activity.ui_frames += 1 } else { self.activity.canvas_frames += 1 }
        // Outside a stroke, wait a little for it: a merge on the paint
        // thread takes it band by band and lets go between two.
        // During a stroke, a short wait: the paint thread lets the UI in
        // between two samples.
        let wait = if self.is_drawing { Duration::from_millis(3) } else { Duration::from_millis(15) };
        match if run_ui { lock_soon(doc, wait) } else { None } {
            Some(mut doc) => gpu.render(|root| {
            *ui_saw_canvas = over_canvas;
            let (can_undo, can_redo) = (doc.history().undo_len() > 0, doc.history().redo_len() > 0);
            let (undo_label, redo_label) = (doc.history().undo_label(), doc.history().redo_label());
            let d = &mut *doc;
            // The outline, traced again when the selection or the zoom changed.
            match &d.selection {
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
                let mut rgba = vec![0u8; (side * side * 4) as usize];
                let (w, h) = (d.layers.width as i32, d.layers.height as i32);
                let r = (origin.0.max(0), origin.1.max(0), (origin.0 + side).min(w), (origin.1 + side).min(h));
                if r.0 < r.2 && r.1 < r.3 {
                    let px = d.layers.composite_rect((r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32));
                    let rw = (r.2 - r.0) as usize;
                    for (y, row) in px.chunks_exact(rw * 4).enumerate() {
                        let at = (((r.1 - origin.1) as usize + y) * side as usize + (r.0 - origin.0) as usize) * 4;
                        rgba[at..at + rw * 4].copy_from_slice(row);
                    }
                }
                let mid = ((side / 2 * side + side / 2) * 4) as usize;
                let inside = fx >= 0 && fy >= 0 && fx < w && fy < h;
                let ring = (ring && inside).then(|| [rgba[mid], rgba[mid + 1], rgba[mid + 2]]);
                let target = if pixel { (fx as f32, fy as f32) } else { (c.0.round(), c.1.round()) };
                (c, target, pixel, origin, rgba, ring, mark)
            });
            let mut model = Model {
                brush,
                presets,
                mediums,
                preset: &mut preset,
                doc_kind: d.kind,
                playing,
                fps,
                looping,
                layers: &mut d.layers,
                tool,
                tools: tool_memory,
                fill,
                stay_inside,
                has_selection: d.selection.is_some(),
                selection: d.selection.as_ref(),
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
                },
                shape,
                to_points: &to_points,
                can_undo,
                can_redo,
                undo_label,
                redo_label,
                view_zoom,
                view_angle,
                prefs: &mut prefs,
                display_profile,
                canvas_edit,
                max_canvas,
                home,
            };
            aurastudio_ui::show(root, ui_state, &mut model, &mut actions);
            // What the Layers docker did, as one undo step once over (the
            // opacity released, the name typed).
            if let Some(l) = actions.layer_edit { *layer_label = Some(l); }
            if actions.layers_idle && d.record_tree(layer_label.unwrap_or("Layers")) {
                log::info!("undo step: {}", layer_label.unwrap_or("Layers"));
            }
            if actions.layers_idle { *layer_label = None; }
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
        if t0.elapsed() > Duration::from_millis(100) {
            log::warn!("slow frame: {:.0} ms (UI {})", t0.elapsed().as_secs_f64() * 1000.0, if run_ui { "run" } else { "cached" });
        }
        // The UI waits while the paint thread holds the document: how long.
        match (doc_busy, self.ui_blocked_since) {
            (true, None) => self.ui_blocked_since = Some(t0),
            (false, Some(t)) if run_ui => {
                self.ui_blocked_since = None;
                if t.elapsed() > Duration::from_millis(100) {
                    log::warn!("UI waited {:.0} ms for the document (paint thread busy)", t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            _ => {}
        }
        self.last_frame = Some(Instant::now());
        // egui's cursor wish over its widgets is known now.
        self.apply_cursor();

        if prefs != prefs_before {
            let color = (prefs.color_managed, prefs.perceptual) != (prefs_before.color_managed, prefs_before.perceptual);
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
        if let Some(kind) = actions.import_brushes { self.import_brushes(kind); }
        if let Some((i, medium, before)) = actions.move_brush {
            // The selected brush keeps its working settings.
            self.library.presets[self.preset].settings = self.brush.clone();
            self.library.move_preset(i, &medium, before);
        }
        if let Some((medium, before)) = actions.move_medium { self.library.move_medium(&medium, before.as_deref()); }
        if let Some(kind) = actions.add_medium {
            let name = self.library.add_medium(kind);
            self.ui.rename_medium(&name);
        }
        if let Some((old, new)) = actions.rename_medium {
            if !self.library.rename_medium(&old, &new) { self.ui.notify("That name is empty or taken"); }
        }
        if let Some(name) = actions.remove_medium {
            if !self.library.remove_medium(&name) { self.ui.notify("Only an empty medium can be removed"); }
        }
        if let Some(action) = actions.filter {
            self.paint.send(Cmd::Filter { doc: Arc::clone(&self.doc), action });
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
        if actions.modified || actions.recomposite {
            if let Ok(mut d) = self.doc.try_lock() { d.touch(); }
            self.drop_play_cache();
        }
        // Another frame shown (no edit).
        if actions.frame_changed && !actions.recomposite { self.paint.send(Cmd::RecompositeTime(Arc::clone(&self.doc))); }
        if let Some(cmd) = actions.view { self.view_cmd(cmd); }
        match actions.play {
            Some(true) => self.start_playback(),
            Some(false) => self.stop_playback(),
            None => {}
        }
        if actions.undo { self.undo(); }
        if actions.redo { self.redo(); }
        if actions.clear_layer { self.clear_active_layer(); }
        let Some(gpu) = &mut self.gpu else { return };
        if actions.recomposite && actions.resize_to.is_none() && !self.layers_dirty {
            // Merged on the paint thread (filter layers make a whole merge
            // slow): the UI never waits for it.
            self.paint.send(Cmd::Recomposite(Arc::clone(&self.doc)));
        } else if actions.resize_to.is_some() || actions.recomposite || self.layers_dirty {
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

    /// Go `d` frames back or forward (Animation documents; not during a
    /// stroke or a transform).
    fn step_frame(&mut self, d: i64) {
        self.stop_playback();
        if self.is_drawing || self.xf.is_some() || self.xf_pending || self.ui.has_dialog() { return; }
        {
            let Ok(mut doc) = self.doc.try_lock() else { return };
            if doc.kind != aurastudio_document::Kind::Animation { return; }
            let t = (doc.layers.time() as i64 + d).max(1) as u32;
            if t == doc.layers.time() { return; }
            if !doc.layers.set_time(t) { drop(doc); self.request_redraw(); return; }
        }
        self.paint.send(Cmd::RecompositeTime(Arc::clone(&self.doc)));
        self.request_redraw();
    }

    // --- Files -------------------------------------------------------------

    /// Biggest canvas side the GPU takes (before the window exists: a
    /// common limit).
    /// View zoom (1.0 before the window exists).
    fn zoom(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, GpuState::zoom)
    }

    fn max_canvas(&self) -> u32 {
        self.gpu.as_ref().map_or(16384, GpuState::max_canvas_size)
    }

    fn file_cmd(&mut self, cmd: FileCmd) {
        if self.is_drawing || self.ui.has_dialog() { return; }
        self.cancel_filter();
        // What is saved or exported is the transform kept.
        if matches!(cmd, FileCmd::Save | FileCmd::SaveAs | FileCmd::Export | FileCmd::Close) { self.end_transform(true); }
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

    /// Close the filter dialog, putting the layer back as it was.
    fn cancel_filter(&mut self) {
        if self.ui.close_filter() {
            self.paint.send(Cmd::Filter { doc: Arc::clone(&self.doc), action: aurastudio_ui::FilterAction::Cancel });
        }
    }

    /// Do `action`, first asking to save unsaved changes.
    fn request(&mut self, action: Pending) {
        self.cancel_filter();
        self.end_transform(true);
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
                doc.kind = spec.kind;
                // An animation starts with its paper and an animation folder.
                if spec.kind == aurastudio_document::Kind::Animation { doc.layers.start_animation(); }
                log::info!("new document: {}, {} × {} px, {} dpi", spec.kind.name(), spec.width, spec.height, spec.dpi);
                self.install(doc);
                self.home = false;
            }
            Pending::Open(path) => self.open_path(&path),
            Pending::Close => {
                let id = self.workspace.reserve_id();
                self.install(Document::new(id, "", PLACEHOLDER.0, PLACEHOLDER.1));
                self.home = true;
                self.refresh_home();
                crate::memory::event("document closed");
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
        // A transform, polygon or playback of the document going away.
        self.play = None;
        self.play_next = None;
        self.drop_play_cache();
        self.xf = None;
        self.xf_pending = false;
        self.poly = None;
        let old = self.doc.lock().unwrap().id();
        self.autosaved_revision = doc.revision();
        self.autosave_at = None;
        self.doc = self.workspace.add(doc);
        crate::rescue::watch(&self.doc);
        self.apply_onion_look();
        self.workspace.close(old);
        *self.projection.lock().unwrap() = Projection::new(w, h);
        if let Some(gpu) = &mut self.gpu { gpu.resize_canvas(w, h); }
        self.canvas_edit = (w, h);
        self.ui.invalidate_all_thumbnails();
        self.layers_dirty = true;
        self.request_redraw();
        crate::memory::event("document shown (new / opened)");
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
        {
            let mut d = self.doc.lock().unwrap();
            // A save of a document closed since: nothing left to update.
            if d.id() != doc { return; }
            if !auto && result.is_ok() {
                if d.path.as_deref() != Some(&path) {
                    d.name = aurastudio_file::doc_name(&path);
                    d.path = Some(path.clone());
                }
                d.mark_saved(revision);
            }
        }
        match (auto, result) {
            (true, Ok(())) => {}
            (true, Err(e)) => log::warn!("autosave failed: {e}"),
            (false, Ok(())) => {
                // The work is safe in its own file now.
                if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); }
                self.autosaved_revision = revision;
                self.add_recent(&path);
                self.ui.notify(&format!("Saved {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
                if let Some(action) = self.after_save.take() { self.perform(action); }
            }
            (false, Err(e)) => {
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
                // Without the draft layers (CSP).
                (d.layers.composite_pick((0, 0, w, h), aurastudio_layers::Pick::EXPORT, false), w, h)
            };
            let file = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let msg = match aurastudio_file::export_image(&pixels, w, h, &path) {
                Ok(()) => format!("Exported {file}"),
                Err(e) => format!("Export failed: {e}"),
            };
            let _ = proxy.send_event(AppEvent::Notice(msg));
        });
        self.request_redraw();
    }

    /// Save unsaved work to the recovery file some time after it changed
    /// (on the paint thread's queue, like any save).
    fn check_autosave(&mut self) {
        let Ok(d) = self.doc.try_lock() else { return };
        let rev = d.revision();
        // (Not a filter's or a transform's preview: it isn't the document
        // yet.)
        if !d.is_modified() || rev == self.autosaved_revision || d.filtering() || d.transforming() {
            self.autosave_at = None;
            return;
        }
        let now = Instant::now();
        let Some(after) = autosave_after(self.prefs.autosave_minutes) else { return };
        match self.autosave_at {
            None => self.autosave_at = Some(now + after),
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
        if let Some(xf) = &mut self.xf {
            if xf.drag.is_some() { return; }
            // Inside the transform first.
            if xf.step_back() { self.send_preview(false); return; }
            // Nothing done in it: the document's undo, then a new one.
            self.end_transform(false);
            self.paint.send(Cmd::Undo(Arc::clone(&self.doc)));
            self.start_transform();
            return;
        }
        self.poly = None;
        self.paint.send(Cmd::Undo(Arc::clone(&self.doc)));
    }

    fn redo(&mut self) {
        if self.is_drawing { return; }
        if let Some(xf) = &mut self.xf {
            if xf.drag.is_some() { return; }
            if xf.step_forward() { self.send_preview(false); return; }
            if xf.changed() { return; }
            self.end_transform(false);
            self.paint.send(Cmd::Redo(Arc::clone(&self.doc)));
            self.start_transform();
            return;
        }
        self.paint.send(Cmd::Redo(Arc::clone(&self.doc)));
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            // Only there to wake the loop; about_to_wait does the work.
            AppEvent::Wake => {}
            AppEvent::Painted => self.on_painted(),
            AppEvent::LayerChanged(layer_id, rect) => {
                if self.frame_stats.count > 0 {
                    log::info!("stroke frames (composite+render, main thread): {}", self.frame_stats);
                    self.frame_stats = Timing::default();
                }
                self.on_painted();
                self.ui.invalidate_thumbnail(layer_id, rect);
                self.drop_play_cache();
                self.request_redraw();
            }
            AppEvent::PlayPlan { gen, plan } => {
                if let (true, Some(plan), Some(gpu)) = (gen == self.play_gen, plan, &mut self.gpu) { gpu.play_begin(*plan); }
            }
            AppEvent::PlayTiles { gen, key, tiles } => {
                if let (true, Some(gpu)) = (gen == self.play_gen, &mut self.gpu) { gpu.play_add(key, &tiles); }
            }
            AppEvent::Notice(text) => {
                self.ui.notify(&text);
                self.request_redraw();
            }
            AppEvent::Saved { doc, path, revision, auto, result } => self.on_saved(doc, path, revision, auto, result),
            AppEvent::Selection => self.request_redraw(),
            AppEvent::TransformStarted(r) => {
                let wanted = std::mem::take(&mut self.xf_pending) && self.tool == Tool::Transform;
                match r {
                    Ok(src) if wanted => {
                        let mut xf = Xform::new(src, std::mem::take(&mut self.pending_targets));
                        if self.transform_opts.mode == TransformMode::Warp { xf.set_grid(self.transform_opts.grid); xf.undo.clear(); }
                        self.xf = Some(xf);
                    }
                    // The tool was left meanwhile.
                    Ok(_) => self.paint.send(Cmd::Transform { doc: Arc::clone(&self.doc), action: TransformAction::Cancel }),
                    Err(msg) => self.ui.notify(msg),
                }
                self.update_cursor();
                self.request_redraw();
            }
            AppEvent::TransformLift(l) => {
                let Some(gpu) = &mut self.gpu else { return };
                if self.xf.as_ref().is_some_and(|x| x.src == l.src) {
                    let layers: Vec<(&[u8], f32)> = l.layers.iter().map(|x| (&x.premul[..], x.opacity)).collect();
                    if !gpu.lift_begin(l.src, &l.below, l.above.as_deref(), &layers) { log::warn!("transform: no live preview (sizes)"); }
                    // Changed before it came (Warp mode's grid...): show it.
                    if self.xf.as_ref().is_some_and(Xform::changed) { self.lift_show(); self.request_redraw(); }
                }
            }
            AppEvent::TransformShown(n) => {
                // The exact result is in: the canvas shows it.
                if n == self.xf_seq { if let Some(gpu) = &mut self.gpu { gpu.lift_hold(false); } }
            }
            AppEvent::Thumbnail { path, w, h, rgba } => {
                self.ui.set_recent_thumbnail(&path, w, h, &rgba);
                self.request_redraw();
            }
        }
    }

    /// Every pending OS message handled. WinTab packets come as messages
    /// without a winit event: drained here.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit {
            event_loop.exit();
            return;
        }
        for e in self.debounce.poll(Instant::now()) { self.handle_event(e); }
        self.drain_stylus();
        self.check_autosave();
        self.tick_playback();
        self.tick_memory();
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
            // One frame per display refresh; UI-only hover frames 60 per
            // second at most (a hovering pen moves all the time).
            let dragging = self.gpu.as_ref().is_some_and(GpuState::ui_is_dragging);
            let interval = if self.is_drawing {
                PAINT_FRAME
            } else if self.ui_dirty && !dragging && self.nav.is_none() {
                self.frame_interval.max(UI_FRAME)
            } else {
                self.frame_interval
            };
            let due = self.last_frame.map_or(now, |t| t + interval);
            if now >= due {
                self.render_frame();
                static FIRST: std::sync::Once = std::sync::Once::new();
                FIRST.call_once(|| crate::memory::mark("first frame drawn"));
            } else {
                self.deferred_frame = Some(self.deferred_frame.map_or(due, |t| t.min(due)));
            }
        }
        self.schedule(event_loop);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Opens maximized. Restored, it fits the screen and is centered
        // (1440 x 900 is wider than an HD screen at 150 % scaling).
        let mut attrs = Window::default_attributes()
            .with_title(APP_TITLE)
            .with_window_icon(window_icon())
            .with_maximized(self.prefs.open_maximized)
            .with_inner_size(winit::dpi::LogicalSize::new(1440u32, 900u32));
        if let Some(m) = event_loop.primary_monitor() {
            let (screen, pos) = (m.size(), m.position());
            let w = (1440.0 * m.scale_factor()).min(screen.width as f64 * 0.85);
            let h = (900.0 * m.scale_factor()).min(screen.height as f64 * 0.85);
            attrs = attrs
                .with_inner_size(winit::dpi::PhysicalSize::new(w, h))
                .with_position(winit::dpi::PhysicalPosition::new(
                    pos.x as f64 + (screen.width as f64 - w) / 2.0,
                    pos.y as f64 + (screen.height as f64 - h) / 2.0,
                ));
        }

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        crate::memory::mark("window");
        let size   = {
            let d = self.doc.lock().unwrap();
            (d.layers.width, d.layers.height)
        };
        use aurastudio_gpu::Backend;
        let prefer = match self.prefs.renderer {
            aurastudio_ui::Renderer::Vulkan => Backend::Vulkan,
            aurastudio_ui::Renderer::DirectX12 => Backend::DirectX12,
            aurastudio_ui::Renderer::OpenGl => Backend::OpenGl,
            aurastudio_ui::Renderer::Metal => Backend::Metal,
        };
        let mut gpu = match pollster::block_on(GpuState::new(Arc::clone(&window), size, prefer, gpu_quality(self.prefs.canvas_quality), self.prefs.vsync)) {
            Ok(g) => g,
            Err(e) => {
                // Nothing to draw with at all: say so, don't just vanish.
                log::error!("no graphics API runs: {e}");
                rfd::MessageDialog::new()
                    .set_title("AuraStudio")
                    .set_description(format!("AuraStudio can't draw its window: no graphics driver it can use was found (Vulkan, DirectX 12, OpenGL). Updating the graphics driver may help.\n\n{e}"))
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                event_loop.exit();
                return;
            }
        };
        crate::memory::mark("graphics device");
        self.renderer = gpu.backend().name().to_string();
        log::info!("graphics: {}", self.renderer);
        gpu.clear_color = aurastudio_ui::theme::backdrop_rgb();
        log::info!("max canvas size: {} px", gpu.max_canvas_size());

        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let stylus = Stylus::install(h.hwnd.get());
                log::info!("pen input backend: {}", stylus.backend());
                self.stylus = Some(stylus);
                if !aurastudio_input::disable_alt_menu(h.hwnd.get()) { log::warn!("couldn't turn off the Alt menu"); }
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
        self.cursors = Some(crate::cursors::Cursors::new(event_loop, window_scale));
        self.gpu    = Some(gpu);
        log::info!("display scaling: {:.0} % ({})", window_scale * 100.0,
            self.hwnd().map_or_else(String::new, aurastudio_input::dpi_report));
        self.apply_settings();
        self.refresh_dpi("startup");
        self.refresh_display_profile();
        self.refresh_home();
        crate::memory::mark("settings and start screen");
        self.offer_recovery();
        if let Some(path) = self.open_arg.take() { self.open_path(&path); }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id:        WindowId,
        event:      WindowEvent,
    ) {
        if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = &event {
            // Cursors at the new display scaling.
            self.cursors = Some(crate::cursors::Cursors::new(event_loop, *scale_factor));
        }
        // Left button bounce filter first (debounce.rs): a release is held
        // back until it is sure not to be the switch chattering.
        let kind = match &event {
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } if state.is_pressed() => DebounceKind::LeftPress,
            WindowEvent::MouseInput { button: MouseButton::Left, .. } => DebounceKind::LeftRelease,
            WindowEvent::CursorMoved { .. } => DebounceKind::Move,
            _ => DebounceKind::Other,
        };
        // Leaving the window: nothing held may stay behind.
        let flush = matches!(event, WindowEvent::Focused(false) | WindowEvent::CloseRequested);
        let before = self.debounce.bounces;
        let mut events = if flush { self.debounce.flush() } else { Vec::new() };
        events.extend(self.debounce.event(event, kind, Instant::now()));
        if self.debounce.bounces != before {
            log::info!("mouse button bounce ignored ({} so far)", self.debounce.bounces);
        } else if kind == DebounceKind::LeftPress {
            // A release this short that went through: likely a bounce longer
            // than the debounce (or a very fast double click). Logged, to
            // tune the setting.
            if let Some(gap) = self.debounce.last_gap.filter(|g| *g < Duration::from_millis(150)) {
                log::info!("mouse button up for {} ms only (debounce {} ms)", gap.as_millis(), self.debounce.window.as_millis());
            }
        }
        for e in events { self.handle_event(e); }
    }
}

impl App {
    fn handle_event(&mut self, event: WindowEvent) {
        // egui says when it needs a frame. RedrawRequested must not ask for
        // another (that looped: 1.6 cores idle over a docker).
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
        let ui_dragging = self.gpu.as_ref().is_some_and(GpuState::ui_is_dragging);
        let is_redraw = matches!(event, WindowEvent::RedrawRequested);
        // Moving over the canvas changes no hover: only the outline moves.
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
                // Alt switches the brush to the picker, Shift / Alt change
                // the selection mode: cursor (and loupe sign) follow.
                if self.over_canvas {
                    self.update_cursor();
                    if self.loupe_on().is_some() { self.request_redraw(); }
                    else if alt != self.modifiers.alt_key() { self.request_canvas_frame(); }
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

            // Maybe on another monitor now: other profile, other scaling.
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
                let ui_typing = self.gpu.as_ref().is_some_and(GpuState::ui_wants_keyboard);
                if key.state != ElementState::Pressed || ui_typing { return; }
                let ctrl  = self.modifiers.control_key() || self.modifiers.super_key();
                let shift = self.modifiers.shift_key();
                // Alt + a key is a combination, never the key's own shortcut
                // (Alt+E must not pick the eraser).
                let alt   = self.modifiers.alt_key();
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
                    Key::Named(NamedKey::Delete) if !key.repeat && !alt => {
                        // Over the Layers docker or the Animation panel: delete what is picked there.
                        if !self.ui.delete_key() { self.clear_active_layer(); }
                    }
                    // Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y (repeat when held); building an arc
                    // outline, Ctrl+Z takes back its last arc.
                    Key::Character(c) if ctrl && !shift && c.eq_ignore_ascii_case("z") && self.arc.active() => self.undo_arc(),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
                        if shift { self.redo() } else { self.undo() }
                    }
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => self.redo(),
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("k") => {
                        self.ui.open_prefs();
                        self.request_redraw();
                    }
                    // Layers: Ctrl+G group, Ctrl+Shift+G ungroup, Ctrl+Alt+G
                    // clip to the layer below (Photoshop's keys).
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("g") => {
                        let cmd = if alt { LayerCmd::Clip } else if shift { LayerCmd::Ungroup } else { LayerCmd::Group };
                        self.ui.layer_shortcut(cmd);
                        self.request_redraw();
                    }
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => self.select(SelectCmd::All),
                    Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("d") => self.select(SelectCmd::Reselect),
                    Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("i") => self.select(SelectCmd::Invert),
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("t") => {
                        if self.tool == Tool::Transform { self.start_transform(); } else { self.set_tool(Tool::Transform); }
                    }
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("c") => self.edit_cmd(EditCmd::Copy),
                    Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("v") => self.edit_cmd(EditCmd::Paste),
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("d") => self.select(SelectCmd::None),
                    Key::Character(c) if !ctrl && !alt && !key.repeat && Group::from_key(c).is_some() => {
                        if let Some(g) = Group::from_key(c) { self.select_group(g); }
                    }
                    // Arc outline: Enter paints it, Escape drops it.
                    Key::Named(NamedKey::Enter) if self.arc.active() => self.finish_arcs(),
                    Key::Named(NamedKey::Escape) if self.arc.active() => self.cancel_arcs(),
                    // Transform: Enter keeps it, Escape drops it; arrows nudge.
                    Key::Named(NamedKey::Enter) if self.xf.is_some() => self.end_transform(true),
                    Key::Named(NamedKey::Escape) if self.xf.is_some() => self.end_transform(false),
                    // On the transform tool with none under way: a new one.
                    Key::Named(NamedKey::Enter) if self.tool == Tool::Transform && !key.repeat && !self.is_drawing => self.start_transform(),
                    Key::Named(k @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) if self.xf.as_ref().is_some_and(|x| x.drag.is_none()) => {
                        let d = if shift { 10.0 } else { 1.0 };
                        let (dx, dy) = match k { NamedKey::ArrowLeft => (-d, 0.0), NamedKey::ArrowRight => (d, 0.0), NamedKey::ArrowUp => (0.0, -d), _ => (0.0, d) };
                        if let Some(xf) = &mut self.xf { xf.nudge(dx, dy); }
                        self.send_preview(false);
                    }
                    // Polygon selection: Enter closes it, Escape drops it,
                    // Backspace takes back its last point.
                    Key::Named(NamedKey::Enter) if self.poly.is_some() => self.finish_poly(),
                    Key::Named(NamedKey::Escape) if self.poly.is_some() => { self.poly = None; self.request_redraw(); }
                    Key::Named(NamedKey::Backspace) if self.poly.is_some() => {
                        if let Some((pts, _)) = &mut self.poly { pts.pop(); if pts.is_empty() { self.poly = None; } }
                        self.request_redraw();
                    }
                    Key::Named(NamedKey::Escape) if self.gesture.is_some() => { self.gesture = None; self.update_cursor(); self.request_redraw(); }
                    // View keys (Krita): 1 = 100 %, 2 = fit, 4 / 6 = rotate
                    // by 15 degrees, 5 = reset rotation, + / - = zoom.
                    Key::Character(c) if !ctrl && !alt => match c {
                        "1" => self.view_cmd(ViewCmd::ActualSize),
                        "2" => self.view_cmd(ViewCmd::Fit),
                        "4" => self.view_cmd(ViewCmd::RotateLeft),
                        "6" => self.view_cmd(ViewCmd::RotateRight),
                        "5" => self.view_cmd(ViewCmd::ResetRotation),
                        "+" | "=" => self.view_cmd(ViewCmd::ZoomIn),
                        "-" => self.view_cmd(ViewCmd::ZoomOut),
                        // Previous / next frame (OpenToonz).
                        "," => self.step_frame(-1),
                        "." => self.step_frame(1),
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
                } else if self.xf.as_ref().is_some_and(|x| x.drag.is_some()) {
                    self.xform_release();
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
                // The loupe is UI: a UI frame (also when it just left).
                let loupe = self.loupe_on().is_some();
                if loupe || self.loupe_shown { self.request_redraw(); }
                if self.nav.is_some() {
                    self.drag_nav();
                } else if self.xf.as_ref().is_some_and(|x| x.drag.is_some()) {
                    self.xform_drag();
                } else if self.gesture.is_some() {
                    self.tool_drag();
                } else if self.poly.is_some() {
                    self.request_redraw();
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
        // A slider or the view being dragged follows the pointer now.
        if (ui_dragging || self.nav.is_some()) && !is_redraw { self.render_if_due(); }
    }
}

/// The logo, for the title bar and the taskbar.
fn window_icon() -> Option<winit::window::Icon> {
    let png = include_bytes!("../../../assets/logo/aurastudio-256.png");
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// The canvas display quality chosen in Preferences, for the GPU side.
fn gpu_quality(q: aurastudio_ui::CanvasQuality) -> aurastudio_gpu::CanvasQuality {
    match q {
        aurastudio_ui::CanvasQuality::High => aurastudio_gpu::CanvasQuality::High,
        aurastudio_ui::CanvasQuality::Standard => aurastudio_gpu::CanvasQuality::Standard,
    }
}

/// Send the whole merged image to the GPU, in bands of rows: the full
/// image never exists in RAM (a B4 page at 600 dpi is 200 MB).
/// The canvas pixel under `p`, if inside a `size` canvas.
fn pixel_in((x, y): (f32, f32), (w, h): (u32, u32)) -> Option<(u32, u32)> {
    (x >= 0.0 && y >= 0.0 && x < w as f32 && y < h as f32).then_some((x as u32, y as u32))
}

fn upload_all(layers: &aurastudio_layers::LayerStack, gpu: &mut GpuState) {
    let (w, h) = (layers.width, layers.height);
    let mut band = Vec::new();
    for rect in layers.merge_bands((0, 0, w, h), 64) {
        band.resize(w as usize * (rect.3 - rect.1) as usize * 4, 0);
        layers.composite_rect_to(rect, &mut band);
        gpu.upload(rect, &band);
    }
}

/// What makes the app work, over 5 s: logged when it keeps drawing with no
/// stroke or view drag going on.
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
    /// Logged above this (10 % of a core).
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

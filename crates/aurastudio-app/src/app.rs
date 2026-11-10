//! The application: the window, its events, and what holds it together.
//! The work is split by subject in the modules below (all `impl App`).

mod files;
mod frame;
mod keys;
mod pointer;
mod tools;
mod xform;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use crate::Locked;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use aurastudio_core::{DocType, Point};
use aurastudio_document::{Document, Edit, SelectMode, SharedDoc, Workspace};
use aurastudio_file::EXTENSION;
use aurastudio_forge::{Brush, Library};
use aurastudio_input::{Device, Input};
use aurastudio_layers::FillParams;
use aurastudio_tools::arc::ArcTool;
use aurastudio_tools::xform::Xform;
use aurastudio_ui::{PickerOptions, Prefs, SelectOptions, ShapeOptions, Tool, ToolMemory, TransformOptions, UiState, ViewCmd, WindowCmd};

use crate::paint_thread::{self, Cmd, PaintThread};
use crate::projection::{Projection, SharedProjection};
use crate::window::Screen;

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
fn brush_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AURASTUDIO_BRUSH_DIR") { return Some(PathBuf::from(dir)); }
    dirs::data_dir().map(|d| d.join("AuraStudio").join("brushes"))
}

/// A recently used file, and when AuraStudio last opened or saved it
/// (Unix seconds; None: listed before that was kept).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Recent {
    pub path: PathBuf,
    pub at:   Option<i64>,
}

/// The list of recently used files: "seconds<TAB>path" per line (a line
/// of a path alone: from before the time was kept).
fn recent_list_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("AuraStudio").join("recent.txt"))
}

fn load_recent() -> Vec<Recent> {
    recent_list_path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Vec::new, |s| parse_recent(&s))
}

fn parse_recent(text: &str) -> Vec<Recent> {
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| match l.split_once('\t') {
        // (A tab can't be in a Windows path.)
        Some((t, p)) if t.parse::<i64>().is_ok() => Recent { path: PathBuf::from(p), at: t.parse().ok() },
        _ => Recent { path: PathBuf::from(l), at: None },
    }).take(MAX_RECENT).collect()
}

fn recent_text(list: &[Recent]) -> String {
    list.iter().map(|r| match r.at {
        Some(t) => format!("{t}\t{}\n", r.path.display()),
        None => format!("{}\n", r.path.display()),
    }).collect()
}

fn save_recent(list: &[Recent]) {
    let Some(path) = recent_list_path() else { return };
    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Err(e) = std::fs::write(&path, recent_text(list)) { log::warn!("recent files: {e}"); }
}

/// "10 Nov 2026, 14:32": Unix seconds in the system's time zone.
pub(crate) fn local_time(secs: i64) -> Option<String> {
    let t = jiff::Timestamp::from_second(secs).ok()?;
    Some(t.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %b %Y, %H:%M").to_string())
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

/// "5 minutes ago"... (a hint: the exact time is in the tooltip).
fn ago(t: std::time::SystemTime) -> String { ago_secs(t.elapsed().map_or(0, |d| d.as_secs())) }

fn ago_secs(s: u64) -> String {
    const DAY: u64 = 86_400;
    let (n, unit) = match s {
        0..=89 => return "a moment ago".into(),
        90..=5399 => (s / 60, "minute"),
        5400..=129_599 => (s / 3600, "hour"),
        _ if s < 30 * DAY => (s / DAY, "day"),
        _ if s < 365 * DAY => (s / (30 * DAY), "month"),
        _ => (s / (365 * DAY), "year"),
    };
    format!("{n} {unit}{} ago", if n > 1 { "s" } else { "" })
}

/// Now, Unix seconds.
fn now_secs() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) }

/// A drag of a tool other than the brush, in progress.
enum Gesture {
    /// Color picker: picks under the cursor as it moves.
    Pick,
    /// Lasso: its outline so far, canvas pixels.
    Lasso { pts: Vec<Point>, mode: SelectMode },
    /// Rectangle / ellipse selection: corner where it started, the other
    /// one; Shift / Alt held at the press (they chose the mode: pressed
    /// later, they make a square, centered).
    Box { ellipse: bool, a: Point, b: Point, mode: SelectMode, held: (bool, bool) },
    /// The selection's outline dragged: from, to (canvas pixels).
    MoveSel { start: Point, now: Point },
    /// Line, rectangle or ellipse being dragged.
    Shape { tool: Tool, start: Point, end: Point },
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
    segs:  Vec<aurastudio_document::Segment>,
}

/// View navigation drag in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav { Pan, Rotate, Zoom }

/// The button holding a view drag (only its release ends it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavButton { Left, Middle }

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
    /// The brush's stroke preview was drawn.
    Preview(crate::preview::Done),
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
    /// The tree changed (an undo of a layer step...).
    TreeChanged,
    /// A document read on its thread, or why not.
    Opened { what: Opening, result: Result<Read, String> },
    /// The canvas was resized on the paint thread.
    Resized(u32, u32),
    /// Brush files read on the import thread.
    BrushesRead(BrushFiles),
}

/// Brush files read, each or why not.
pub(crate) struct BrushFiles(pub Vec<(PathBuf, Result<aurastudio_forge::ReadFile, String>)>);

impl std::fmt::Debug for BrushFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "BrushFiles({})", self.0.len()) }
}

/// A document read on the open thread.
pub(crate) struct Read(pub Box<Document>);

impl std::fmt::Debug for Read {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Read({})", self.0.name) }
}

/// What is being opened on the open thread.
#[derive(Debug)]
pub(crate) enum Opening {
    File(PathBuf),
    /// The autosave left by a session that didn't close normally.
    Recovery,
}

/// The GPU preview's mesh for `warp` (lift.rs): a grid over the source
/// and the transparent pixel around it, canvas x, y and texture u, v.
fn lift_mesh(warp: &aurastudio_transform::Warp, src: aurastudio_core::Rect) -> Option<Vec<[f32; 4]>> {
    let n = aurastudio_gpu::LIFT_GRID;
    let (sw, sh) = (src.w() as f64, src.h() as f64);
    let m = warp.mapper(src)?;
    let mut out = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    for j in 0..=n {
        for i in 0..=n {
            // From one pixel before the source to one after.
            let (pu, pv) = (i as f64 / n as f64 * (sw + 2.0) - 1.0, j as f64 / n as f64 * (sh + 2.0) - 1.0);
            let p = m.map(Point::new(src.x0 as f64 + pu, src.y0 as f64 + pv));
            if !(p.x.is_finite() && p.y.is_finite()) { return None; }
            out.push([p.x as f32, p.y as f32, ((pu + 1.0) / (sw + 2.0)) as f32, ((pv + 1.0) / (sh + 2.0)) as f32]);
        }
    }
    Some(out)
}

/// The document locked, if it gets free within `wait`.
fn lock_soon(doc: &SharedDoc, wait: Duration) -> Option<std::sync::MutexGuard<'_, aurastudio_document::Document>> {
    use std::sync::atomic::Ordering;
    if let Ok(d) = doc.try_lock() { return Some(d); }
    if wait.is_zero() { return None; }
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
    proxy:  EventLoopProxy<AppEvent>,
    window: Option<Arc<Window>>,
    gpu:    Option<Screen>,
    /// Pen, mouse and touch (aurastudio-input).
    input:  Input,

    /// The brushes: built-ins and imported files (kept on disk).
    library:      Library,
    /// The stroke previews' thread.
    previews:     crate::preview::Previews,
    /// Strokes begun: each one's jitter seed.
    strokes:      u64,
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
    recent: Vec<Recent>,
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
    /// Alt + right drag: brush size / hardness (size_drag.rs).
    size_drag:  Option<crate::size_drag::SizeDrag>,
    pressure:   f32,
    /// The pen's tilt (degrees toward +x / +y) and barrel rotation at the
    /// last sample.
    pen:        ((f32, f32), f32),
    /// The way the pen last went (radians, counter-clockwise, y up), and
    /// the canvas point it is measured from: the cursor's direction.
    cursor_dir: f32,
    dir_from:   Option<(f32, f32)>,
    /// The device drawing the stroke (only its samples go in it).
    stroke_device: Option<Device>,
    /// The pen is near the tablet (between ProximityIn and Out), and the
    /// cursor was last set for the mouse (a switch resets it).
    pen_near:       bool,
    cursor_for_mouse: bool,
    /// Where the current stroke's last sample was sent (window pixels).
    last_sent: Option<(f32, f32)>,
    modifiers:  ModifiersState,
    /// Monitor ICC profile (None: not found, treated as sRGB).
    display_profile: Option<aurastudio_color::Profile>,
    /// Space held: a left drag moves the view instead of painting.
    space_down: bool,
    /// A Shift key is held (tracked from the keys too, see shift_held).
    shift_down: bool,
    /// A Ctrl key is held (same reason).
    ctrl_down:  bool,
    /// View drag in progress, with the last cursor position, and the
    /// button that started it (only its release ends the drag).
    nav:        Option<(Nav, (f32, f32))>,
    nav_button: NavButton,
    /// Fixed point of the zoom / rotation drag (window pixels).
    nav_pivot:  (f32, f32),
    /// The cursor is on a window edge that resizes the window.
    edge_hover: bool,
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
    /// Selection tools' settings, the polygon being clicked (points, mode).
    select:     SelectOptions,
    picker:     PickerOptions,
    /// The loupe was in the last UI frame.
    loupe_shown: bool,
    poly:       Option<(Vec<Point>, SelectMode)>,
    ants:       Ants,
    /// Transform tool: its settings, the transform under way (handles),
    /// one asked for and not started yet.
    transform_opts: TransformOptions,
    xf:         Option<Xform>,
    xf_pending: bool,
    /// The transform as last shown (GPU or paint thread); the paint
    /// thread's preview with this number is the one to wait for.
    xf_seq:     u64,
    /// The transform's live preview on the GPU (lift.rs).
    lift:       Option<aurastudio_gpu::Lift>,
    /// A lift over this area asked for (the transform moved past the
    /// last one's).
    lift_asked: Option<aurastudio_core::Rect>,
    /// Animation playback: when it started and from which frame; the next
    /// frame's time; its speed and looping.
    play:       Option<(Instant, u32)>,
    play_next:  Option<Instant>,
    looping:    bool,
    /// The layers asked for when the transform was started.
    xf_targets: Vec<u64>,
    /// The graphics API in use.
    renderer:   String,

    /// The paint thread updated the projection: upload it next frame.
    painted:      bool,
    /// A change outside strokes (visibility, opacity, add/remove/clear
    /// layer, resize) needs a full-canvas recomposite.
    layers_dirty: bool,
    /// A document is being read on the open thread.
    opening: bool,
    last_frame:   Option<Instant>,
    /// Shortest time between two frames outside strokes: one display
    /// refresh (egui asks for frames "right away" when animating).
    frame_interval: Duration,
    /// A frame wanted before `frame_interval` was over: drawn at this time.
    deferred_frame: Option<Instant>,
    /// Since when the UI couldn't run (document held by the paint thread).
    ui_blocked_since: Option<Instant>,
    /// What the paint thread was doing when the UI started waiting.
    ui_blocked_by: crate::paint_thread::Work,
    /// Color management to set up again once the document is free (it
    /// was busy when asked).
    color_pending: bool,
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
    /// Delayed frame asked by egui (tooltip, notice fade-out).
    repaint_at:   Option<Instant>,
    /// Why the app woke up and drew without being touched (logged when it
    /// does so often: an idle app should sleep).
    wakes:        Wakes,
}

/// Frames drawn while nobody touches the app, by what asked for them,
/// over 10 s.
#[derive(Default)]
struct Wakes {
    since:    Option<Instant>,
    /// Pointer (pen included), keys, window events seen in the window of
    /// time.
    touched:  bool,
    frames:   u32,
    delayed:  u32,
    deferred: u32,
    egui:     u32,
    other:    u32,
}

impl Wakes {
    /// Each wake: log the last 10 s if they drew more than a few frames
    /// untouched.
    fn tick(&mut self) {
        let now = Instant::now();
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < Duration::from_secs(10) { return; }
        if !self.touched && self.frames > 10 {
            log::info!("idle but drawing: {} frames in 10 s (egui asked {}, delayed repaints {}, deferred frames {}, other {})",
                self.frames, self.egui, self.delayed, self.deferred, self.other);
        }
        *self = Wakes { since: Some(now), ..Wakes::default() };
    }
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let library = match brush_dir() {
            Some(dir) => {
                let (lib, warnings) = Library::open(dir);
                for w in warnings { log::warn!("brush library: {w}"); }
                lib
            }
            None => Library::builtin(),
        };
        // Stroke previews: the list's kept on disk, all of them made once.
        let preview_proxy = proxy.clone();
        let previews = crate::preview::Previews::start(library.tip_reader(), dirs::cache_dir().map(|d| d.join("AuraStudio").join("stroke-previews")), move |done| {
            let _ = preview_proxy.send_event(AppEvent::Preview(done));
        });
        previews.keep_all(library.presets.iter().map(|p| p.settings.clone()));
        let mut workspace = Workspace::new();
        let doc = workspace.new_document("", PLACEHOLDER.0, PLACEHOLDER.1, DocType::Illustration);
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
                paint_thread::Event::TreeChanged => AppEvent::TreeChanged,
                paint_thread::Event::Resized(w, h) => AppEvent::Resized(w, h),
            });
        });
        // Start with the pen.
        let first = library.presets.iter().position(|p| p.name == "Pen").unwrap_or(0);
        Self {
            proxy,
            window: None,
            gpu:    None,
            input:  Input::new(env!("CARGO_PKG_VERSION")),

            brush:       library.presets[first].settings.clone(),
            library,
            previews,
            strokes:     0,
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
            size_drag:  None,
            pressure:   1.0,
            pen:        ((0.0, 0.0), 0.0),
            cursor_dir: 0.0,
            dir_from:   None,
            stroke_device: None,
            pen_near: false,
            cursor_for_mouse: true,
            last_sent: None,
            modifiers:  ModifiersState::empty(),
            display_profile: None,
            space_down: false,
            shift_down: false,
            ctrl_down:  false,
            nav_pivot:  (0.0, 0.0),
            nav:        None,
            nav_button: NavButton::Left,
            edge_hover: false,
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
            select:     SelectOptions::default(),
            picker:     PickerOptions::default(),
            loupe_shown: false,
            poly:       None,
            ants:       Ants::default(),
            transform_opts: TransformOptions::default(),
            xf:         None,
            xf_pending: false,
            xf_seq:     0,
            lift:       None,
            lift_asked: None,
            play:       None,
            play_next:  None,
            looping:    true,
            xf_targets: Vec::new(),
            renderer:   String::new(),

            painted:      false,
            // Upload the first composite.
            layers_dirty: true,
            opening: false,
            last_frame:   None,
            frame_interval: UI_FRAME,
            deferred_frame: None,
            ui_blocked_since: None,
            ui_blocked_by: crate::paint_thread::Work::Idle,
            color_pending: false,
            redraw_wanted: false,
            ui_dirty:     true,
            memory_at:    None,
            ui_saw_canvas: false,
            repaint_at:   None,
            wakes:        Wakes::default(),
        }
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
            ViewCmd::ActualSize    => { let u = gpu.zoom_unit(); gpu.view_set_zoom(u, c) }
            ViewCmd::Fit           => gpu.view.fit_to_area(),
            ViewCmd::RotateLeft    => gpu.view_rotate_at(-step, turn),
            ViewCmd::RotateRight   => gpu.view_rotate_at(step, turn),
            ViewCmd::ResetRotation => gpu.view_reset_rotation(),
            ViewCmd::MirrorH       => gpu.view_mirror(true),
            ViewCmd::MirrorV       => gpu.view_mirror(false),
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
        let wake = [self.repaint_at, self.deferred_frame, self.autosave_at, self.input.deadline(), self.play_next].into_iter().flatten().min();
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None    => ControlFlow::Wait,
        });
    }

    /// The canvas to merge again for the screen (after an edit made here).
    fn show(&self, region: aurastudio_document::Region) {
        if !region.is_nothing() { self.paint.send(Cmd::Show { doc: Arc::clone(&self.doc), region }); }
    }

    /// An edit through the paint thread's queue (after the strokes sent
    /// before it).
    fn send_edit(&self, edit: Edit) { self.paint.send(Cmd::Edit { doc: Arc::clone(&self.doc), edit, seq: None }); }

    /// The title bar's buttons: the window minimized, maximized, closed or
    /// moved with the pointer.
    fn window_cmd(&mut self, cmd: WindowCmd) {
        let Some(w) = self.window.clone() else { return };
        match cmd {
            WindowCmd::Minimize => w.set_minimized(true),
            WindowCmd::ToggleMaximize => w.set_maximized(!w.is_maximized()),
            WindowCmd::Close => self.request(Pending::Quit),
            WindowCmd::Drag => { if let Err(e) = w.drag_window() { log::warn!("window drag: {e}"); } }
        }
    }

    /// Win32 window handle, for the monitor profile lookup.
    fn hwnd(&self) -> Option<isize> {
        let handle = self.window.as_ref()?.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    /// Biggest canvas side the GPU takes (before the window exists: a
    /// common limit).
    /// View zoom (1.0 before the window exists).
    fn zoom(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |g| g.zoom())
    }

    fn max_canvas(&self) -> u32 {
        self.gpu.as_ref().map_or(16384, |g| g.max_canvas_size())
    }

    fn canvas_size(&self) -> (u32, u32) {
        let p = self.projection.locked();
        (p.width, p.height)
    }
}


impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        profiling::scope!("app event");
        match event {
            // Only there to wake the loop; about_to_wait does the work.
            AppEvent::Wake => {}
            AppEvent::Painted => self.on_painted(),
            AppEvent::LayerChanged(layer_id, rect) => {
                self.on_painted();
                self.ui.invalidate_thumbnail(layer_id, rect);
                self.request_redraw();
            }
            AppEvent::Opened { what, result } => self.opened(what, result),
            AppEvent::Resized(w, h) => self.resized(w, h),
            AppEvent::BrushesRead(files) => self.brushes_read(files),
            AppEvent::TreeChanged => {
                self.on_painted();
                self.ui.invalidate_all_thumbnails();
                self.request_redraw();
            }
            AppEvent::Notice(text) => {
                self.ui.notify(&text);
                self.request_redraw();
            }
            AppEvent::Saved { doc, path, revision, auto, result } => self.on_saved(doc, path, revision, auto, result),
            AppEvent::Selection => self.request_redraw(),
            AppEvent::TransformStarted(r) => self.transform_started(r),
            AppEvent::TransformLift(l) => self.transform_lift(&l),
            AppEvent::TransformShown(n) => {
                // The exact result is in: the canvas shows it.
                if n == self.xf_seq { if let Some(l) = &mut self.lift { l.live = false; } }
            }
            AppEvent::Thumbnail { path, w, h, rgba } => {
                self.ui.set_recent_thumbnail(&path, w, h, &rgba);
                self.request_redraw();
            }
            AppEvent::Preview(done) => {
                match done {
                    crate::preview::Done::Current { slot, w, h, rgba } => self.ui.set_stroke_preview(slot, w, h, &rgba),
                    crate::preview::Done::List { index, settings, w, h, alpha } => self.ui.set_list_preview(index, &settings, w, h, &alpha),
                }
                self.request_redraw();
            }
        }
    }

    /// Every pending OS message handled. WinTab packets come as messages
    /// without a winit event: drained here.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        profiling::scope!("wake");
        #[cfg(feature = "profile")]
        tracy_client::plot!("heap MB", crate::memory::heap_bytes() as f64 / 1_048_576.0);
        if self.quit {
            // The brush in use as last changed, kept with its preset.
            if let Err(e) = self.library.keep(self.preset, self.brush.clone()) { log::warn!("brush not kept: {e}"); }
            event_loop.exit();
            return;
        }
        // (Pen packets come without a winit event; a mouse release held
        // back by the bounce filter comes due.)
        self.poll_input();
        self.check_autosave();
        self.tick_playback();
        self.tick_memory();
        self.update_title();
        if self.color_pending { self.apply_color_management(); }

        self.wakes.tick();
        if self.repaint_at.is_some_and(|t| Instant::now() >= t) {
            self.repaint_at = None;
            self.ui_dirty = true;
            self.redraw_wanted = true;
            self.wakes.delayed += 1;
        }
        let now = Instant::now();
        if self.deferred_frame.is_some_and(|t| now >= t) {
            self.deferred_frame = None;
            self.redraw_wanted = true;
            self.wakes.deferred += 1;
        }
        if std::mem::take(&mut self.redraw_wanted) {
            // New paint as soon as merged; while drawing, the brush outline
            // alone one canvas frame after the last frame (the paint's
            // frames come first); one frame per display refresh otherwise;
            // UI-only hover frames 60 per second at most (a hovering pen
            // moves all the time).
            let dragging = self.gpu.as_ref().is_some_and(Screen::ui_is_dragging);
            let interval = if self.painted {
                Self::paint_frame()
            } else if self.is_drawing {
                paint_thread::canvas_frame()
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
            // The title bar is the interface's own (menus, name, buttons).
            .with_decorations(false)
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

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                // No window: say so and leave (a panic said nothing).
                log::error!("no window: {e}");
                rfd::MessageDialog::new()
                    .set_title("AuraStudio")
                    .set_description(format!("AuraStudio can't open its window.\n\n{e}"))
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                event_loop.exit();
                return;
            }
        };
        crate::memory::mark("window");
        let size   = {
            let d = self.doc.locked();
            (d.stack().width, d.stack().height)
        };
        use aurastudio_gpu::Backend;
        let prefer = match self.prefs.renderer {
            aurastudio_ui::Renderer::Vulkan => Backend::Vulkan,
            aurastudio_ui::Renderer::DirectX12 => Backend::DirectX12,
            aurastudio_ui::Renderer::OpenGl => Backend::OpenGl,
            aurastudio_ui::Renderer::Metal => Backend::Metal,
        };
        let mut gpu = match pollster::block_on(Screen::new(Arc::clone(&window), size, prefer, gpu_quality(self.prefs.canvas_quality), self.prefs.vsync)) {
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
        crate::rescue::note("graphics", self.renderer.clone());
        gpu.clear_color = aurastudio_ui::theme::backdrop_rgb();
        log::info!("max canvas size: {} px", gpu.max_canvas_size());

        let hwnd = window.window_handle().ok().and_then(|h| match h.as_raw() { RawWindowHandle::Win32(h) => Some(h.hwnd.get()), _ => None });
        // The pen thread wakes the loop at once: its samples go to the
        // paint thread without waiting for another event.
        let proxy = self.proxy.clone();
        self.input.set_waker(move || { let _ = proxy.send_event(AppEvent::Wake); });
        let backend = self.input.install(hwnd.unwrap_or(0));
        log::info!("pen input backend: {}", backend.name());
        crate::rescue::note("pen", backend.name().to_string());
        if let Some(h) = hwnd {
            if !crate::system::disable_alt_menu(h) { log::warn!("couldn't turn off the Alt menu"); }
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
            self.hwnd().map_or_else(String::new, crate::system::dpi_report));
        self.apply_settings();
        self.refresh_dpi("startup");
        self.refresh_display_profile();
        self.refresh_home();
        crate::memory::mark("settings and start screen");
        self.offer_recovery();
        if let Some(path) = self.open_arg.take() { self.open_path(&path); }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        profiling::scope!("window event");
        if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = &event {
            // Cursors at the new display scaling.
            self.cursors = Some(crate::cursors::Cursors::new(event_loop, *scale_factor));
        }
        // egui says when it needs a frame. RedrawRequested must not ask for
        // another (that looped: 1.6 cores idle over a docker).
        let ui_needs_frame = self.gpu.as_mut().is_some_and(|gpu| gpu.egui_handle_event(&event));
        if !matches!(event, WindowEvent::RedrawRequested) { self.wakes.touched = true; }
        if let WindowEvent::CursorMoved { position, .. } = &event {
            self.cursor = (position.x as f32, position.y as f32);
            self.cursor_in_window = true;
        }
        let over_canvas = self.gpu.as_ref().is_some_and(|gpu| gpu.is_canvas_at(self.cursor));
        let ui_dragging = self.gpu.as_ref().is_some_and(Screen::ui_is_dragging);
        let is_redraw = matches!(event, WindowEvent::RedrawRequested);
        // Moving over the canvas changes no hover: only the outline moves.
        let canvas_move = matches!(event, WindowEvent::CursorMoved { .. }) && over_canvas && self.ui_saw_canvas && !ui_dragging;
        if !is_redraw && (ui_needs_frame || ui_dragging) && !canvas_move { self.request_redraw(); }
        if !self.is_drawing && self.nav.is_none() { self.over_canvas = over_canvas; }

        match event {
            // Asks to save unsaved changes first.
            WindowEvent::CloseRequested => if !self.ui.has_dialog() { self.request(Pending::Quit) },
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu { gpu.resize(size); }
                self.input.window_moved();
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
            WindowEvent::Focused(true) => {
                self.input.activated(true);
                self.refresh_dpi("focused");
                // The cursor set again (back from another window: the
                // system arrow could stay over the brush outline).
                if let Some(c) = &mut self.cursors { c.invalidate(); }
                self.update_cursor();
            }
            // Maybe on another monitor now: other profile, other scaling.
            WindowEvent::Moved(_) => {
                self.input.window_moved();
                self.refresh_dpi("moved");
                self.refresh_display_profile();
            }
            // Another monitor (or the screens changed): the tablet context
            // is opened again on the new desktop.
            WindowEvent::ScaleFactorChanged { .. } => {
                if !self.is_drawing { self.input.display_changed(); } else { self.input.window_moved(); }
                self.refresh_dpi("moved");
                self.refresh_display_profile();
            }
            WindowEvent::KeyboardInput { event: key, .. } => self.on_key(key),
            // The pointer goes to aurastudio-input; its events come back
            // through poll_input, below.
            WindowEvent::CursorMoved { position, .. } => self.input.mouse_moved(position.x, position.y),
            WindowEvent::MouseInput { button, state, .. } => {
                let b = match button {
                    MouseButton::Left => aurastudio_input::Button::Left,
                    MouseButton::Right => aurastudio_input::Button::Right,
                    MouseButton::Middle => aurastudio_input::Button::Middle,
                    MouseButton::Back => aurastudio_input::Button::Other(0),
                    MouseButton::Forward => aurastudio_input::Button::Other(1),
                    MouseButton::Other(n) => aurastudio_input::Button::Other(n.min(250) as u8),
                };
                self.input.mouse_button(b, state.is_pressed());
            }
            WindowEvent::MouseWheel { delta, .. } => {
                use winit::event::MouseScrollDelta;
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 60.0, p.y as f32 / 60.0),
                };
                self.input.wheel(dx, dy);
            }
            WindowEvent::CursorEntered { .. } => {
                self.cursor_in_window = true;
                if let Some(c) = &mut self.cursors { c.invalidate(); }
            }
            WindowEvent::CursorLeft { .. } => {
                self.input.mouse_left();
                self.cursor_in_window = false;
                if let Some(c) = &mut self.cursors { c.invalidate(); }
                self.update_cursor();
                self.request_redraw();
            }
            // Fingers (Windows: the pen comes through WinTab / Windows Ink,
            // and winit's touch would draw it twice).
            WindowEvent::Touch(t) if cfg!(not(windows)) => {
                use winit::event::{Force, TouchPhase};
                let force = t.force.map(|f| match f {
                    Force::Normalized(f) => f as f32,
                    Force::Calibrated { force, max_possible_force, .. } => (force / max_possible_force) as f32,
                });
                let down = matches!(t.phase, TouchPhase::Started | TouchPhase::Moved);
                self.input.touch(t.id, t.location.x, t.location.y, down, force);
            }
            WindowEvent::RedrawRequested => self.render_frame(),
            _ => {}
        }
        self.poll_input();
        // A slider being dragged follows the pointer now.
        if ui_dragging && !is_redraw { self.render_if_due(); }
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

/// The canvas pixel under `p`, if inside a `size` canvas.
fn pixel_in(p: Point, (w, h): (u32, u32)) -> Option<(i32, i32)> {
    (p.x >= 0.0 && p.y >= 0.0 && p.x < w as f64 && p.y < h as f64).then_some((p.x as i32, p.y as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list keeps when each file was last had; an old line (a path
    /// alone) still reads, without a time.
    #[test]
    fn the_recent_list_keeps_when_a_file_was_opened() {
        let list = parse_recent("1762780320\tC:\\art\\a.aura\nC:\\refs\\old.jpg\n\n");
        assert_eq!(list, vec![
            Recent { path: PathBuf::from("C:\\art\\a.aura"), at: Some(1_762_780_320) },
            Recent { path: PathBuf::from("C:\\refs\\old.jpg"), at: None },
        ]);
        assert_eq!(parse_recent(&recent_text(&list)), list);
    }

    #[test]
    fn ago_reads_in_the_right_unit() {
        assert_eq!(ago_secs(30), "a moment ago");
        assert_eq!(ago_secs(6 * 86_400 + 5), "6 days ago");
        assert_eq!(ago_secs(95 * 86_400), "3 months ago");
        assert_eq!(ago_secs(974 * 86_400), "2 years ago");
        assert_eq!(ago_secs(400 * 86_400), "1 year ago");
        // The exact time, day and month never confused.
        let t = local_time(1_762_780_320).unwrap();
        assert!(t.contains("Nov 2025"), "{t}");
    }
}

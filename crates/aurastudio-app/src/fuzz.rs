//! Crash hunt: random sequences of what a user can do, run headless.
//!
//! The real UI (egui, every docker, the dock area) gets random pointer
//! clicks, drags, wheel and keys; its actions are applied the way the app
//! applies them (recomposite, resize, undo / redo / clear through the paint
//! thread). Strokes with random brushes, MyPaint presets, sizes and
//! positions (far off canvas too) run on the real paint thread, at the same
//! time as the UI, like in the app. The view gets random zoom / rotation /
//! pan. Any panic, on any thread, fails the run.
//!
//!     FUZZ_STEPS=1000000 FUZZ_SEED=7 cargo test --release -p aurastudio fuzz -- --ignored --nocapture
//!
//! FUZZ_SECS stops it after that many seconds instead (steps then only
//! cap it). Filter dialogs are opened now and then as their menu does
//! (random clicks seldom find one).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aurastudio_brush::{Brush, PenSample};
use aurastudio_brushes::Library;
use aurastudio_document::{DocId, Document, SharedDoc};
use aurastudio_gpu::CanvasView;
use aurastudio_layers::{FillParams, Selection};
use aurastudio_ui::{Actions, BlendMode, Model, Overlay, SelectCmd, SelectOptions, ShapeOptions, Tool, TransformInfo, TransformOptions, UiState, ViewCmd};
use egui::{pos2, vec2, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

use crate::paint_thread::{Cmd, PaintThread};
use crate::projection::Projection;

const VIEW_CMDS: [ViewCmd; 7] = [
    ViewCmd::ZoomIn, ViewCmd::ZoomOut, ViewCmd::ActualSize, ViewCmd::Fit,
    ViewCmd::RotateLeft, ViewCmd::RotateRight, ViewCmd::ResetRotation,
];

static PANICKED: AtomicBool = AtomicBool::new(false);

pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub(crate) fn below(&mut self, n: u64) -> u64 { self.next() % n.max(1) }
    pub(crate) fn f(&mut self) -> f32 { (self.next() >> 40) as f32 / (1u64 << 24) as f32 }
    pub(crate) fn range(&mut self, a: f32, b: f32) -> f32 { a + (b - a) * self.f() }
    pub(crate) fn chance(&mut self, p: f32) -> bool { self.f() < p }
}

const SCREEN: Rect = Rect { min: Pos2 { x: 0.0, y: 0.0 }, max: Pos2 { x: 1440.0, y: 900.0 } };

/// A pointer position, mostly where the UI is (bars on top, dock column on
/// the right), sometimes anywhere.
fn ui_pos(r: &mut Rng) -> Pos2 {
    match r.below(10) {
        0..=4 => pos2(r.range(1100.0, 1440.0), r.range(0.0, 900.0)),
        5 => pos2(r.range(0.0, 44.0), r.range(60.0, 400.0)),
        6..=7 => pos2(r.range(0.0, 1440.0), r.range(0.0, 70.0)),
        _ => pos2(r.range(-20.0, 1460.0), r.range(-20.0, 920.0)),
    }
}

fn key(k: Key, mods: Modifiers) -> [Event; 2] {
    [
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: mods },
        Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: mods },
    ]
}

/// Random input events for one UI frame.
fn ui_events(r: &mut Rng, pointer: &mut Pos2, held: &mut Option<PointerButton>) -> Vec<Event> {
    let mut ev = Vec::new();
    let mods = if r.chance(0.1) { Modifiers::CTRL } else if r.chance(0.05) { Modifiers::SHIFT } else { Modifiers::NONE };
    match r.below(20) {
        // Press somewhere (left mostly, right opens the slider fields).
        0..=4 if held.is_none() => {
            *pointer = ui_pos(r);
            let button = if r.chance(0.15) { PointerButton::Secondary } else { PointerButton::Primary };
            ev.push(Event::PointerMoved(*pointer));
            ev.push(Event::PointerButton { pos: *pointer, button, pressed: true, modifiers: mods });
            *held = Some(button);
        }
        // Drag a bit (sliders, separators, dock tabs, floating docks).
        0..=9 => {
            let step = if r.chance(0.2) { 300.0 } else { 30.0 };
            *pointer += vec2(r.range(-step, step), r.range(-step, step));
            ev.push(Event::PointerMoved(*pointer));
        }
        // Release.
        10..=12 => {
            if let Some(button) = held.take() {
                ev.push(Event::PointerButton { pos: *pointer, button, pressed: false, modifiers: mods });
            }
        }
        // Click (press + release in the same frame), sometimes a double one.
        13..=14 => {
            *pointer = ui_pos(r);
            ev.push(Event::PointerMoved(*pointer));
            for _ in 0..if r.chance(0.2) { 2 } else { 1 } {
                ev.push(Event::PointerButton { pos: *pointer, button: PointerButton::Primary, pressed: true, modifiers: mods });
                ev.push(Event::PointerButton { pos: *pointer, button: PointerButton::Primary, pressed: false, modifiers: mods });
            }
        }
        15 => ev.push(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, r.range(-5.0, 5.0)),
            modifiers: mods,
        }),
        // Typing: digits, text, Enter, Escape, Delete, Tab...
        16 => ev.push(Event::Text(["7", "1000", "-3", "abc", "0", "99999", "é"][r.below(7) as usize].into())),
        _ => {
            let k = [Key::Enter, Key::Escape, Key::Delete, Key::Backspace, Key::Tab, Key::ArrowUp, Key::Z, Key::A]
                [r.below(8) as usize];
            ev.extend(key(k, mods));
        }
    }
    if r.chance(0.01) { ev.push(Event::PointerGone); }
    ev
}

fn random_brush(r: &mut Rng, base: &Brush) -> Brush {
    let mut b = base.clone();
    b.radius = match r.below(10) { 0..=2 => 0.5, 3..=5 => r.below(8) as f32 + 0.5, 6..=8 => r.below(60) as f32 + 1.0, _ => r.below(500) as f32 + 1.0 };
    b.opacity = if r.chance(0.1) { 0.0 } else { r.f() };
    b.hardness = if r.chance(0.2) { [0.0, 1.0][r.below(2) as usize] } else { r.f() };
    b.blend = BlendMode::ALL[r.below(BlendMode::ALL.len() as u64) as usize];
    b.color = [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8];
    b
}

/// Send one random stroke to the paint thread.
fn stroke(r: &mut Rng, pt: &PaintThread, doc: &SharedDoc, lib: &Library, size: (u32, u32), finish: bool) {
    let preset = &lib.presets[r.below(lib.presets.len() as u64) as usize];
    let brush = random_brush(r, &preset.settings);
    pt.send(Cmd::Begin { doc: Arc::clone(doc), brush, forge: preset.brush.clone(), limit: (r.below(4) == 0).then(FillParams::default) });
    let (w, h) = (size.0 as f32, size.1 as f32);
    // Sometimes a geometry tool's shape (as App::paint_path sends it).
    if r.chance(0.2) {
        let far = if r.chance(0.1) { 1e6 } else { 1.3 };
        let mut p = || (r.range(-0.3, far) * w, r.range(-0.3, far) * h);
        let (a, b, m) = (p(), p(), p());
        let path = match r.below(4) {
            0 => crate::shapes::line(a, b),
            1 => crate::shapes::rectangle(a, b),
            2 => crate::shapes::ellipse(a, b),
            _ => crate::shapes::arc(a, b, m),
        };
        for (i, (x, y)) in path.into_iter().enumerate() {
            pt.send(Cmd::Sample(PenSample::new(x, y, 1.0, i as f64 * 0.001)));
        }
        if finish { pt.send(Cmd::End); }
        return;
    }
    let far = r.chance(0.1);
    let mut t = 0.0f64;
    let (mut x, mut y) = (r.range(-50.0, w + 50.0), r.range(-50.0, h + 50.0));
    for _ in 0..r.below(40) {
        if far {
            x = r.range(-1e6, 1e6);
            y = r.range(-1e6, 1e6);
        } else {
            x += r.range(-60.0, 60.0);
            y += r.range(-60.0, 60.0);
        }
        // Pen time mostly rising, sometimes repeated or going back.
        t += if r.chance(0.05) { -0.01 } else if r.chance(0.05) { 0.0 } else { r.range(0.001, 0.02) as f64 };
        let pressure = match r.below(5) { 0 => 0.0, 1 => 1.0, _ => r.f() };
        pt.send(Cmd::Sample(PenSample::new(x, y, pressure, t)));
    }
    if finish { pt.send(Cmd::End); }
}

#[test]
#[ignore]
fn fuzz() {
    let secs: Option<f64> = std::env::var("FUZZ_SECS").ok().and_then(|s| s.parse().ok());
    let steps: u64 = std::env::var("FUZZ_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(if secs.is_some() { u64::MAX } else { 20_000 });
    let seed: u64 = std::env::var("FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICKED.store(true, Ordering::SeqCst);
        default_hook(info);
    }));

    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut lib = Library::load();
    let mut size = (r.below(300) as u32 + 1, r.below(300) as u32 + 1);
    let doc: SharedDoc = Arc::new(Mutex::new(Document::new(DocId(1), "fuzz", size.0, size.1)));
    let projection = Arc::new(Mutex::new(Projection::new(size.0, size.1)));
    let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});

    let ctx = egui::Context::default();
    let mut ui = UiState::new();
    let mut brush = lib.presets[0].settings.clone();
    let mut preset = 0usize;
    let mut canvas_edit = size;
    let mut view = CanvasView::default();
    let area = [0.0, 70.0, 1100.0, 830.0];
    let mut pointer = pos2(700.0, 400.0);
    let mut held = None;
    let mut stroke_open = false;
    let mut tool = Tool::default();
    let mut tool_memory = aurastudio_ui::ToolMemory::default();
    let mut fill = FillParams::default();
    let mut shape = ShapeOptions::default();
    let mut prefs = aurastudio_ui::Prefs::default();
    let mut stay_inside = false;
    let mut select_opts = SelectOptions::default();
    let mut transform_opts = TransformOptions::default();
    let (mut ui_frames, mut strokes, mut ops, mut resets, mut slow) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let progress: u64 = std::env::var("FUZZ_PROGRESS").ok().and_then(|s| s.parse().ok()).unwrap_or(5_000);
    let t0 = Instant::now();
    let trace = std::env::var("FUZZ_TRACE").is_ok();

    let mut done = 0u64;
    for step in 0..steps {
        if PANICKED.load(Ordering::SeqCst) { break; }
        if secs.is_some_and(|s| t0.elapsed().as_secs_f64() >= s) { break; }
        done = step + 1;
        let kind = r.below(100);
        let step_start = Instant::now();
        if trace { eprintln!("step {step} kind {kind} size {size:?} rss {} MB", rss_mb()); }
        match kind {
            // The UI, with random input.
            0..=69 => {
                let Ok(mut d) = doc.try_lock() else { continue };
                let input = RawInput {
                    screen_rect: Some(SCREEN),
                    events: ui_events(&mut r, &mut pointer, &mut held),
                    time: Some(step as f64 * 0.016),
                    ..Default::default()
                };
                let (can_undo, can_redo) = (d.history().undo_len() > 0, d.history().redo_len() > 0);
                let (undo_label, redo_label) = (d.history().undo_label(), d.history().redo_label());
                let ants = d.selection.as_ref().map(|s| s.outline(2)).unwrap_or_default();
                let selection = d.selection.clone();
                let to_points = |p: (f32, f32)| { let q = view.canvas_to_screen(p, size, area); (q.0, q.1) };
                let mut actions = Actions::default();
                let mut p = preset;
                let open_filter = (r.below(40) == 0).then(|| r.below(64) as usize);
                let _ = ctx.run(input, |ctx| {
                    let mut model = Model {
                        brush: &mut brush,
                        presets: &lib.presets,
                        mediums: &lib.mediums,
                        preset: &mut p,
                        layers: &mut d.layers,
                        can_undo,
                        can_redo,
                        undo_label,
                        redo_label,
                        view_zoom: view.zoom,
                        view_angle: view.angle.to_degrees(),
                        prefs: &mut prefs,
                        display_profile: None,
                        canvas_edit: &mut canvas_edit,
                        max_canvas: 4096,
                        home: false,
                        tool: &mut tool,
                        tools: &mut tool_memory,
                        fill: &mut fill,
                        stay_inside: &mut stay_inside,
                        shape: &mut shape,
                        has_selection: selection.is_some(),
                        selection: selection.as_ref(),
                        select: &mut select_opts,
                        transform: &mut transform_opts,
                        transform_info: TransformInfo::default(),
                        renderer: "Vulkan",
                        overlay: Overlay { ants: &ants, ..Default::default() },
                        to_points: &to_points,
                    };
                    if let Some(n) = open_filter { ui.open_filter_dialog(ctx, n, &model); }
                    aurastudio_ui::show(ctx, &mut ui, &mut model, &mut actions);
                });
                ui_frames += 1;
                // As App::render_frame: the docker's edits become undo steps.
                if actions.layers_idle { d.record_tree(actions.layer_edit.unwrap_or("Layers")); }
                // As App::select_preset.
                if p != preset && p < lib.presets.len() {
                    let color = brush.color;
                    lib.presets[preset].settings = brush.clone();
                    brush = lib.presets[p].settings.clone();
                    brush.color = color;
                    preset = p;
                }
                // As the end of App::render_frame.
                if let Some(cmd) = actions.view { apply_view(&mut view, cmd, size, area); }
                drop(d);
                if let Some(cmd) = actions.select {
                    use aurastudio_document::SelectOp;
                    let op = match cmd {
                        SelectCmd::All => SelectOp::All,
                        SelectCmd::None => SelectOp::None,
                        SelectCmd::Reselect => SelectOp::Reselect,
                        SelectCmd::Invert => SelectOp::Invert,
                        SelectCmd::Grow(v) => SelectOp::Grow(v),
                        SelectCmd::Shrink(v) => SelectOp::Shrink(v),
                        SelectCmd::Feather(v) => SelectOp::Feather(v),
                        SelectCmd::Border(v) => SelectOp::Border(v),
                        SelectCmd::FromLayer => SelectOp::FromLayer(select_opts.mode),
                    };
                    pt.send(Cmd::Select { doc: Arc::clone(&doc), op });
                }
                if !stroke_open {
                    if actions.undo { pt.send(Cmd::Undo(Arc::clone(&doc))); }
                    if actions.redo { pt.send(Cmd::Redo(Arc::clone(&doc))); }
                    if actions.clear_layer { pt.send(Cmd::Clear(Arc::clone(&doc))); }
                }
                if let Some(action) = actions.filter { pt.send(Cmd::Filter { doc: Arc::clone(&doc), action }); }
                if actions.recomposite && actions.resize_to.is_none() {
                    // As the app: merged on the paint thread.
                    pt.send(Cmd::Recomposite(Arc::clone(&doc)));
                } else if actions.resize_to.is_some() {
                    let mut d = doc.lock().unwrap();
                    if let Some((w, h)) = actions.resize_to {
                        d.resize(w, h);
                        *projection.lock().unwrap() = Projection::new(w, h);
                        size = (w, h);
                        ui.invalidate_all_thumbnails();
                    }
                    projection.lock().unwrap().clear();
                    merge_all_in_bands(&d.layers);
                }
                // As App::render_frame: the merged pieces go to the GPU.
                check_patches(&projection);
            }
            // Strokes (sometimes left open, like a lost pen-up).
            70..=84 => {
                if stroke_open && r.chance(0.7) {
                    pt.send(Cmd::End);
                    stroke_open = false;
                } else {
                    let finish = r.chance(0.9);
                    stroke(&mut r, &pt, &doc, &lib, size, finish);
                    stroke_open = !finish;
                    strokes += 1;
                }
            }
            // Shortcuts, also sent mid-stroke (the paint thread refuses).
            85..=91 => {
                let d = Arc::clone(&doc);
                let (w, h) = (size.0 as f32, size.1 as f32);
                match r.below(10) {
                    6 => {
                        // Copy, then paste somewhere (off the canvas too).
                        let clip = d.lock().unwrap().copy();
                        if let Some(mut clip) = clip {
                            clip.x += r.range(-1.5, 1.5) as i64 * size.0 as i64;
                            clip.y += r.range(-1.5, 1.5) as i64 * size.1 as i64;
                            pt.send(Cmd::Paste { doc: d, clip });
                        }
                    }
                    0 => pt.send(Cmd::Undo(d)),
                    1 => pt.send(Cmd::Redo(d)),
                    2 => pt.send(Cmd::Clear(d)),
                    3 | 4 => {
                        let refer = [aurastudio_layers::Refer::Layer, aurastudio_layers::Refer::All, aurastudio_layers::Refer::References][r.below(3) as usize];
                        let params = FillParams { tolerance: r.below(256) as u8, refer, skip_drafts: r.chance(0.5), expand: r.below(4) as u32 };
                        let color = [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8, r.below(256) as u8];
                        // Off the canvas too (the app checks; the thread must cope).
                        let (x, y) = (r.range(-0.1, 1.1) * w, r.range(-0.1, 1.1) * h);
                        pt.send(Cmd::Fill { doc: d, x: x.max(0.0) as u32, y: y.max(0.0) as u32, color, params });
                    }
                    // A transform: begin, previews, apply or cancel.
                    8 | 9 => {
                        use crate::paint_thread::TransformAction as T;
                        let ids = vec![d.lock().unwrap().layers.active_id()];
                        pt.send(Cmd::Transform { doc: Arc::clone(&d), action: T::Begin(ids) });
                        let src = (size.0 / 4, size.1 / 4, (size.0 / 4 + 1).max(size.0 * 3 / 4).min(size.0), (size.1 / 4 + 1).max(size.1 * 3 / 4).min(size.1));
                        for _ in 0..r.below(5) {
                            pt.send(Cmd::Transform { doc: Arc::clone(&d), action: T::Preview(crate::pure::random_warp(&mut r, src), crate::pure::random_interp(&mut r)) });
                        }
                        let end = if r.chance(0.6) { T::Apply(crate::pure::random_warp(&mut r, src), crate::pure::random_interp(&mut r)) } else { T::Cancel };
                        pt.send(Cmd::Transform { doc: d, action: end });
                    }
                    // A selection command (the tools, the Select menu).
                    5 | 7 => {
                        let op = crate::pure::random_select_op(&mut r, size.0, size.1);
                        pt.send(Cmd::Select { doc: d, op });
                    }
                    // A new selection: lasso, rectangle, all or none.
                    _ => {
                        let sel = match r.below(4) {
                            0 => {
                                let pts: Vec<(f32, f32)> = (0..r.below(60)).map(|_| (r.range(-0.3, 1.3) * w, r.range(-0.3, 1.3) * h)).collect();
                                Selection::polygon(size.0, size.1, &pts, r.chance(0.5))
                            }
                            1 => {
                                let (x, y) = (r.below(size.0 as u64 + 50) as u32, r.below(size.1 as u64 + 50) as u32);
                                Selection::rect(size.0, size.1, (x, y, x + r.below(400) as u32, y + r.below(400) as u32))
                            }
                            2 => Some(Selection::all(size.0, size.1)),
                            _ => None,
                        };
                        d.lock().unwrap().selection = sel;
                    }
                }
                ops += 1;
            }
            // Layer changes while the paint thread may be painting.
            92..=95 => {
                let mut d = doc.lock().unwrap();
                let ids = d.layers.layer_ids();
                match r.below(10) {
                    0 => { d.layers.add_layer("L"); }
                    1 => { d.layers.add_folder("F"); }
                    // Filter layers, marks, groups (merged in the general
                    // merge, saved in the file).
                    7 => {
                        let kinds: Vec<_> = aurastudio_filters::GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
                        let mut f = aurastudio_filters::Filter::new(kinds[r.below(kinds.len() as u64) as usize]);
                        for v in f.values.iter_mut() { if r.chance(0.3) { *v *= r.range(0.0, 3.0); } }
                        d.layers.add_filter_layer(f);
                    }
                    8 => {
                        let active = d.layers.active_id();
                        let on = !d.layers.active_node().props().reference;
                        let p = d.layers.active_node_mut().props_mut();
                        match r.below(4) {
                            0 => p.clip ^= true,
                            1 => d.layers.set_reference(active, on),
                            2 => p.draft ^= true,
                            _ => p.label = r.below(9) as u8,
                        }
                    }
                    // Several layers grouped, moved together.
                    9 => {
                        let some: Vec<u64> = ids.iter().copied().filter(|_| r.chance(0.3)).collect();
                        match r.below(4) {
                            0 => { d.layers.group_active("G"); }
                            1 => { d.layers.ungroup_active(); }
                            2 => { d.layers.group_nodes(&some, "G"); }
                            _ => if let (Some(&first), Some(&target)) = (some.first(), ids.get(r.below(ids.len() as u64) as usize)) {
                                let place = [aurastudio_layers::Place::Above, aurastudio_layers::Place::Below, aurastudio_layers::Place::Inside][r.below(3) as usize];
                                d.layers.move_nodes(&some, first, target, place);
                            },
                        }
                    }
                    2 => { d.layers.remove_active(); }
                    3 => { d.layers.duplicate_active(); }
                    4 => d.layers.move_active(r.chance(0.5)),
                    5 => if let Some(&id) = ids.get(r.below(ids.len() as u64) as usize) { d.layers.set_active(id) },
                    _ => if let Some(&id) = ids.get(r.below(ids.len() as u64) as usize) {
                        if let Some(n) = d.layers.node_mut(id) { *n.locked_mut() ^= true; }
                    },
                }
                // Recorded for undo now, or with the next ones.
                if r.chance(0.6) { d.record_tree("Fuzz"); }
                ops += 1;
            }
            // Save and open again (in memory): the same document comes back.
            96 if r.chance(0.3) => {
                let d = doc.lock().unwrap();
                let mut buf = std::io::Cursor::new(Vec::new());
                aurastudio_file::write(&aurastudio_file::snapshot(&d, None), &mut buf).unwrap();
                buf.set_position(0);
                let back = aurastudio_file::read(buf, DocId(2), "fuzz", 1 << 15).expect("saved file must open").doc;
                assert_eq!(back.layers.layer_ids(), d.layers.layer_ids());
                assert_eq!(back.layers.active_id(), d.layers.active_id());
                assert_eq!(back.layers.rows().len(), d.layers.rows().len(), "filter layers / folders lost");
                for id in d.layers.layer_ids() {
                    let (Some(aurastudio_layers::Node::Layer(a)), Some(aurastudio_layers::Node::Layer(b))) =
                        (d.layers.node(id), back.layers.node(id)) else { panic!("layer {id} lost") };
                    assert!(a.tiles == b.tiles && a.name == b.name && a.locked == b.locked, "layer {id} differs");
                }
                ops += 1;
            }
            // Canvas resize (rare, big ones rarer).
            96 => {
                let (w, h) = if r.chance(0.05) {
                    (r.below(3000) as u32 + 1, r.below(3000) as u32 + 1)
                } else {
                    (r.below(400) as u32 + 1, r.below(400) as u32 + 1)
                };
                let mut d = doc.lock().unwrap();
                d.resize(w, h);
                *projection.lock().unwrap() = Projection::new(w, h);
                size = (w, h);
                merge_all_in_bands(&d.layers);
                ui.invalidate_all_thumbnails();
                ops += 1;
            }
            // View: zoom, rotate, pan, fit, like the shortcuts and drags.
            _ => {
                let pivot = (r.range(-500.0, 2000.0), r.range(-500.0, 1500.0));
                match r.below(4) {
                    0 => view.zoom_at(r.range(0.01, 50.0), pivot, size, area),
                    1 => view.rotate_at(r.range(-10.0, 10.0), view.rotation_pivot(size, area), size, area),
                    2 => view.pan_by(r.range(-1e5, 1e5), r.range(-1e5, 1e5)),
                    _ => apply_view(&mut view, VIEW_CMDS[r.below(VIEW_CMDS.len() as u64) as usize], size, area),
                }
                view.update_fit(size, area);
                let p = view.screen_to_canvas(pivot, size, area);
                assert!(view.zoom.is_finite() && view.angle.is_finite() && p.0.is_finite() && p.1.is_finite(),
                    "view went non-finite: {view:?}");
            }
        }
        if step_start.elapsed().as_secs_f64() > 2.0 {
            println!("SLOW step {step} (kind {kind}): {:.1} s, canvas {size:?}", step_start.elapsed().as_secs_f64());
            slow += 1;
        }
        // Keep the run inside this machine's RAM: duplicating folders over
        // and over grows the document exponentially.
        if let Ok(mut d) = doc.try_lock() {
            let bytes = d.layers.layer_count() as u64 * size.0 as u64 * size.1 as u64 * 4;
            if bytes > 400 << 20 || d.layers.layer_count() > 60 {
                *d = Document::new(DocId(1), "fuzz", 200, 200);
                size = (200, 200);
                *projection.lock().unwrap() = Projection::new(200, 200);
                ui.invalidate_all_thumbnails();
                resets += 1;
            }
        }
        if step % progress == 0 && step > 0 {
            println!("{step} steps, {:.0} s, {} MB", t0.elapsed().as_secs_f64(), rss_mb());
        }
    }
    if stroke_open { pt.send(Cmd::End); }
    // Join the paint thread (everything queued is processed: the fuzz
    // queues faster than it paints), taking the merged pieces as the UI
    // does meanwhile: left there, those of the queued filter previews (a
    // whole layer each) filled the memory.
    let drained = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| while !drained.load(Ordering::SeqCst) {
            projection.lock().unwrap().take_patches();
            std::thread::sleep(std::time::Duration::from_millis(20));
        });
        drop(pt);
        drained.store(true, Ordering::SeqCst);
    });
    let steps = done;
    println!("fuzz seed {seed}: {steps} steps in {:.1} s | {ui_frames} UI frames, {strokes} strokes, {ops} other ops, {resets} memory resets, {slow} slow steps | layers {}",
        t0.elapsed().as_secs_f64(), doc.lock().map(|d| d.layers.layer_count()).unwrap_or(0));
    assert!(!PANICKED.load(Ordering::SeqCst), "a panic happened (see above)");
    assert!(!doc.is_poisoned() && !projection.is_poisoned());
}

/// As app::upload_all, without a GPU: the whole image merged band by band.
pub(crate) fn merge_all_in_bands(layers: &aurastudio_layers::LayerStack) {
    let (w, h) = (layers.width, layers.height);
    let mut band = Vec::new();
    for rect in layers.merge_bands((0, 0, w, h), 64) {
        band.resize(w as usize * (rect.3 - rect.1) as usize * 4, 0);
        layers.composite_rect_to(rect, &mut band);
    }
}

/// Take the queued pieces as the UI thread does, checking each would fit
/// the GPU texture (the upload drops a piece that doesn't: a hole on
/// screen).
fn check_patches(projection: &crate::projection::SharedProjection) {
    let mut p = projection.lock().unwrap();
    let (w, h) = (p.width, p.height);
    for (r, rgba) in p.take_patches() {
        assert!(r.0 < r.2 && r.1 < r.3 && r.2 <= w && r.3 <= h, "piece {r:?} outside {w}x{h}");
        assert_eq!(rgba.len(), (r.2 - r.0) as usize * (r.3 - r.1) as usize * 4, "piece {r:?} has a wrong size");
    }
}

/// As App::view_cmd, on a bare view.
fn apply_view(v: &mut CanvasView, cmd: ViewCmd, c: (u32, u32), a: [f32; 4]) {
    let center = (a[0] + a[2] / 2.0, a[1] + a[3] / 2.0);
    let turn = v.rotation_pivot(c, a);
    let step = 15f32.to_radians();
    match cmd {
        ViewCmd::ZoomIn => v.zoom_at(1.25, center, c, a),
        ViewCmd::ZoomOut => v.zoom_at(0.8, center, c, a),
        ViewCmd::ActualSize => v.set_zoom(1.0, center, c, a),
        ViewCmd::Fit => v.fit_to_area(),
        ViewCmd::RotateLeft => v.rotate_at(-step, turn, c, a),
        ViewCmd::RotateRight => v.rotate_at(step, turn, c, a),
        ViewCmd::ResetRotation => v.rotate_at(-v.angle, turn, c, a),
    }
}

/// Resident memory of this process, in MB (Linux; 0 elsewhere).
pub(crate) fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm").ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096 / (1 << 20))
}

/// Resident memory of a new document at print sizes: what each part costs.
///     cargo test --release -p aurastudio memory_per_preset -- --ignored --nocapture
#[test]
#[ignore]
fn memory_per_preset() {
    for (name, w, h) in [("A4 300 dpi", 2480u32, 3508u32), ("B4 600 dpi", 6071, 8598)] {
        let base = rss_mb();
        let mut doc = Document::new(DocId(1), name, w, h);
        let after_doc = rss_mb();
        merge_all_in_bands(&doc.layers);
        let after_proj = rss_mb();
        doc.layers.add_layer("Layer 2");
        let after_layer = rss_mb();
        let id = doc.layers.active_id();
        doc.layers.begin_stroke(id);
        let after_stroke = rss_mb();
        println!("{name}: document (Paper + Layer 1) {} MB, merged image in RAM {} MB, +1 empty layer {} MB, stroke buffers {} MB",
            after_doc - base, after_proj - after_doc, after_layer - after_proj, after_stroke - after_layer);
        drop(doc);
    }
}

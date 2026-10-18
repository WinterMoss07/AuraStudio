# AuraStudio

An all-in-one drawing, painting, animation and comics application written in
Rust — from the first line to the final render. Inspired by Krita, OpenToonz
and Clip Studio Paint, rebuilt from scratch ("inspired rewrite": we study how
those apps work, then write our own idiomatic Rust instead of translating
their code).

The guiding rule is the opposite of bloated paint software: every brick is
measured before the next one starts — memory, latency, CPU at idle — and the
UI stays quiet so the painting gets the room.

> Status: early but usable for painting tests. Windows is the main target
> (pen input via WinTab / Windows Ink); Linux and macOS build and run with
> mouse / touch input.

---

## What works today

### Painting
- **Brush engine** — one engine for all our brushes; a brush is a preset of
  its settings (the round brush is the first one). Dabs with Krita's "wash"
  accumulation (alpha darken): smooth soft edges and clean crossings, no
  build-up within one stroke (crossing a 20 % stroke at 30 % gives 30 %), and
  Krita's averaged opacity when the pressure falls (no beads). Spacing is
  Krita's auto spacing (0.8 × √diameter: straight hard edges). Pressure-driven
  opacity, hardness, per-brush blend mode.
- **MyPaint brushes** — 196 brushes bundled (David Revoy, Ramón Miranda,
  Tanda, the classic MyPaint set…, all CC0), painted natively through
  [hokusai](https://crates.io/crates/hokusai) (a Rust port of libmypaint):
  the stroke works on tiles loaded from the layer, so smudge, blender, eraser
  and textured brushes behave as in MyPaint.
- **Import** — `.myb` files or whole folders of them (Brushes docker →
  Import…); they are copied into the user's data folder and come back at the
  next launch.
- **Per-brush settings** — each brush keeps its own size, opacity, hardness
  and blend mode (CSP style). The color is global.
- **13 blend modes** (W3C formulas, sRGB space like Photoshop / Krita / CSP)
  for brushes and layers.
- **Brush outline cursor** — the OS arrow is replaced by an outline the size
  of the brush at the current zoom (crosshair when tiny), drawn on the GPU so
  it follows the pen even while the UI is busy.
- **Navigation cursors** — open hand (Space), closed hand (panning) and a
  circle of arrows (rotating), drawn by the app: Windows has no hand cursor
  and showed the four-arrow cross instead.

### Layers & document
- New documents start like CSP: a locked white **Paper** layer and an
  empty **Layer 1** above it, active.
- Layer tree with **folders** (Pass Through or isolated with a blend mode),
  visibility, opacity, **lock** (inherited by folder content), duplicate,
  reorder in / out of folders, rename (double-click).
- **Undo / redo** (Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z) for strokes and layer
  clear. A step only stores the rectangle a stroke touched, LZ4-compressed,
  with a memory budget (512 MB per document by default).
- **Documents** — `aurastudio-document` holds several open documents, each
  with its own layers and history (the UI shows one for now).
- Canvas size presets / custom size, content kept centered.

### Viewing
- **Zoom, pan, rotation** on the GPU; the pen goes through the exact inverse
  transform, so strokes land under the pen at any zoom or angle. Sharp pixels
  from 200 %.
- **Display color management** — the monitor's ICC profile (the one Windows
  assigns to it) is read and applied to the canvas and the color docker with a
  3D lookup table on the GPU (moxcms; perceptual or relative colorimetric).
  The UI itself is not converted.

### Interface (egui + egui_dock)
- Neutral dark theme with one blue accent. Menu bar, tool options bar and
  docks are one seamless surface (no borders, no gaps); only the canvas is
  rounded, set right against the bar and the docks. Sliders fill with the
  accent and show their label and value inside (right-click to type a
  value, click elsewhere or Escape to leave). Dock column: Color (RGB / HSV
  gradient sliders, hex field), Tool options, Brushes, Layers, Canvas. Docks
  can be rearranged, resized and detached.

### Performance architecture
- **Paint thread** — brushes paint on their own thread (one thread per
  stroke, never split: see `paint_thread.rs`); the UI thread only reads the
  pen and shows results. Undo / redo / clear go through the same queue, so
  Ctrl+Z right after lifting the pen undoes that very stroke.
- **Projection off the UI thread** (Krita's model): the paint thread merges
  the changed areas into the projection; the UI thread only copies the
  changed rectangles to the GPU. Changed areas travel as a list of rectangles,
  never one growing bounding box. The UI never waits for the brush (it
  redraws the canvas with the last UI when the document is busy).
- **1000 px brushes**: one dab every ~25 px of pen travel, not on
  every pen sample (benchmarks below).
- **Idle = 0 % CPU**: frames are only drawn when something changes, at most
  one per display refresh (a hovering pen must not drive the frame rate).
- Low-latency presentation (Mailbox / Immediate, one frame in flight).

---

## Controls

| Action | Input |
|---|---|
| Paint | pen / left mouse |
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Clear layer | Delete |
| Zoom | mouse wheel (toward the cursor), `+` / `-` |
| 100 % / fit | `1` / `2` |
| Pan | Space + drag, or middle-button drag |
| Rotate | Shift + Space + drag, or Shift + middle drag; `4` / `6` = ±15°, `5` = reset |

---

## Building and running

```
cargo run --release
```

The CI (`.github/workflows/build.yml`) builds Windows, Linux and macOS
binaries on every push.

Environment variables (for testing and comparisons):

| Variable | Effect |
|---|---|
| `RUST_LOG=info` (default) | log: GPU, pen backend, monitor profile, per-stroke timings |
| `AURASTUDIO_BACKEND=dx12\|vulkan\|metal\|gl` | force a GPU backend |
| `AURASTUDIO_PRESENT=mailbox\|immediate\|fifo` | force a present mode |
| `AURASTUDIO_DISPLAY_ICC=<file.icc>` | use this monitor profile instead of the system's |
| `AURASTUDIO_BRUSH_DIR=<folder>` | where imported brushes are kept |

Benchmarks:

```
cargo run --release -p aurastudio-brush  --example bench_stroke      # brush cost per sample
BENCH_PRESSURE=steady BENCH_RADIUS=500 ...                            # filters
cargo run --release -p aurastudio-layers --example bench_composite   # layer merge cost
cargo test  --release -p aurastudio -- --ignored pipeline --nocapture # paint thread + UI model
cargo run --release -p aurastudio-brush  --example myb_sheet -- out.png a.myb b.myb  # MyPaint sample sheet
cargo run --release -p aurastudio-brush  --example spacing_sheet -- out.png        # round brush strokes (SHEET_LOOP=1, SHEET_RAW=1, SHEET_CONST=1)
```

---

## Code layout

| Crate | Role |
|---|---|
| `aurastudio-canvas` | shared types: `Rect`, `PaintSurface`, blend modes, GPU canvas texture |
| `aurastudio-brush` | the brush engine, native MyPaint engine |
| `aurastudio-brushes` | brush library: built-in presets, bundled MyPaint collection, import |
| `aurastudio-layers` | layer tree, folders, lock, compositing |
| `aurastudio-document` | documents, workspace, undo history |
| `aurastudio-color` | monitor ICC profile, sRGB → display transform (3D LUT) |
| `aurastudio-gpu` | wgpu renderer: canvas, view transform, color LUT, brush outline, egui |
| `aurastudio-input` | Windows pen input (WinTab, Windows Ink) |
| `aurastudio-ui` | egui interface: bars, one file per docker, theme, widgets |
| `aurastudio-app` | the application: event loop, paint thread, projection |

Data crates (layers, brushes, document…) know nothing about the UI.
See `NOTICE.md` for third-party code and assets.

---

## To check / next steps

**To verify on a real machine (Windows + pen)**
- [ ] Display color management with the real monitor profile: are grays now
      neutral? (log line `monitor profile: …`; toggle in the View menu)
- [ ] A 1000 px brush on an A4 300 dpi canvas: per-stroke log lines
      (`stroke: … paint avg …`, `max queued`).
- [ ] Rotation with the pen (Shift + Space + drag).
- [ ] MyPaint brushes: smudge / blender behavior, speed-dependent brushes
      (they now get the pen's own timestamps).
- [ ] Brush import dialog (files and folder) and that imports come back at
      the next launch.

**Missing / planned**
- [ ] Save / open documents (own format, PNG / PSD / ORA export).
- [ ] Undo for structural layer changes (add, delete, move, opacity…) and
      canvas resize (history is cleared today).
- [ ] Persist per-brush settings and UI layout between sessions.
- [ ] Brush editor (MyPaint settings and curves); brush tilt / rotation input
      (WinTab orientation is already read).
- [ ] Document tabs (the workspace already handles several documents).
- [ ] Faster MyPaint compositing for very large brushes; mipmaps for zoomed
      out views.
- [ ] Selection tools, transform, fill, eraser tool mode, color picker.
- [ ] Rooms / workspaces (painting, animation…) with lazy loading.
- [ ] Animation timeline (OpenToonz-inspired), comics panels.
- [ ] Native pen input on macOS / Linux (today: winit touch events).

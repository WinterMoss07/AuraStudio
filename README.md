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

**Painting**
- One native brush engine (Krita-style "wash" opacity, auto spacing,
  pressure) plus 196 bundled MyPaint brushes (CC0) painted natively;
  `.myb` import. Each brush keeps its own size / opacity / hardness / blend
  mode; 13 blend modes. GPU brush outline cursor, app-drawn hand / rotate
  cursors.

**Tools** — toolbox in the canvas's top-left corner (Krita's optimized
layout): Brush, Eraser, Fill, Color picker, Rectangle and Lasso selection.
- Fill on the merged image or the layer alone, tolerance, expand under
  anti-aliased lines; an A4 page in ~10–50 ms.
- Painting, fill and Delete stay inside the selection.

**Files**
- `.aurs` documents (layers, folders, 50 undo steps, preview), atomic saves,
  autosave + crash recovery, unsaved-changes prompts.
- Start screen with recent files; New dialog with paper / manga / comics /
  screen presets.
- Open PNG / JPG / WebP / BMP / TIFF / GIF: the picture is the bottom layer
  and an empty layer above it is active (no painting on the picture by
  mistake). Export to PNG / JPG.

**Layers** — Paper + Layer 1 for new documents, folders, opacity, blend,
lock, rename, undo / redo with a memory budget.

**View** — GPU zoom / pan / rotation, monitor ICC color management.

**Under the hood**
- Brushes on their own thread (never split); merge of the changed areas off
  the UI thread, the full image only on the GPU.
- 64 × 64 tiled layers: an empty layer costs nothing.
- Idle = 0 % CPU; strokes far off the canvas pause instead of freezing.
- `fuzz.rs`: headless crash hunt on the real UI and paint thread.

---

## Controls

| Action | Input |
|---|---|
| Paint | pen / left mouse |
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Clear layer (or the selection) | Delete |
| Tools | B brush, E eraser, G fill, I picker, M rectangle, L lasso |
| Pick a color | Alt + click (brush, eraser, fill) |
| Select all / deselect | Ctrl+A / Ctrl+D |
| Zoom | mouse wheel (toward the cursor), `+` / `-`, Ctrl + Space + drag left / right (or Ctrl + middle drag), around where the drag starts |
| 100 % / fit | `1` / `2` |
| Pan | Space + drag, or middle-button drag |
| Rotate | Shift + Space + drag, or Shift + middle drag; `4` / `6` = ±15°, `5` = reset. Turns around the picture's center while it is all in view, else around the middle of the canvas area |

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
| `aurastudio-file` | `.aurs` format, image import / export |
| `aurastudio-ui` | egui interface: bars, toolbox, one file per docker, theme, widgets |
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
- [x] Rotation with the pen (Shift + Space + drag).
- [ ] MyPaint brushes: smudge / blender behavior, speed-dependent brushes
      (they now get the pen's own timestamps).
- [ ] Brush import dialog (files and folder) and that imports come back at
      the next launch.

**Missing / planned**
- [ ] Redesign of the Brushes docker.
- [ ] Redesign of the New document dialog.
- [ ] PSD / ORA export.
- [ ] Undo for structural layer changes (add, delete, move, opacity…) and
      canvas resize (history is cleared today).
- [ ] Persist per-brush settings and UI layout between sessions.
- [ ] Brush editor (MyPaint settings and curves); brush tilt / rotation input
      (WinTab orientation is already read).
- [ ] Document tabs (the workspace already handles several documents).
- [ ] Faster MyPaint compositing for very large brushes; mipmaps for zoomed
      out views.
- [ ] Selection modes (add / subtract), invert, transform; undo for
      selection changes.
- [ ] Rooms / workspaces (painting, animation…) with lazy loading.
- [ ] Animation timeline (OpenToonz-inspired), comics panels.
- [ ] Native pen input on macOS / Linux (today: winit touch events).

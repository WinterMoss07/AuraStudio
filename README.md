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
  pressure) plus 56 MyPaint brushes (CC0, chosen from the 196 of the
  MyPaint collection, `assets/brushes/catalog.txt`) painted natively;
  `.myb` import. Each brush keeps its own size / opacity / hardness / blend
  mode; 13 blend modes. GPU brush outline cursor, app-drawn hand / rotate
  cursors.
- Brushes by medium: Pencil, Ink, Charcoal & pastel, Paint, Watercolor,
  Airbrush, Blend & smudge, Eraser, Texture & FX, Misc (one icon each in
  the Brushes docker; search looks in all; imported brushes get a medium
  from their name).
- Brush previews: each brush as a stroke painted by the brush itself
  (name in the corner; erasers and blenders over a first stroke); Tool
  options shows the selected brush's stroke, repainted as its size,
  hardness, opacity or blend mode change (on a thread of its own).
- Mouse button bounce filtered for everything (a worn switch no longer
  drops a slider drag or ends a stroke); Windows' Alt menu mode is off.

**Tools** — toolbox on the canvas's left edge, centered (Krita's optimized
layout; Lucide icons across the UI): Brush, Eraser, Fill, Color picker, Rectangle and Lasso selection.
- Fill on the merged image or the layer alone, tolerance, expand under
  anti-aliased lines; an A4 page in ~10–50 ms.
- Painting, fill and Delete stay inside the selection.
- Geometry tools (U cycles them), drawn with the current brush: Line
  (Shift: 15° steps), Rectangle and Ellipse (Shift: square / circle, Alt:
  from the center), and **Arc** (like OpenToonz's): drag or click from
  start to end, the cursor then pulls the curve, click to validate; with
  "Chain", arcs follow each other as one stroke, painted when the outline
  is done (Enter, a click on its last point, or an end dropped on the
  first point: closed for coloring). Ctrl+Z takes back the last arc.
- Copy / paste pixels (selection or whole layer, to a new layer; images
  from other apps through the system clipboard).

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

**View** — GPU zoom / pan / rotation, monitor ICC color management;
mipmapped canvas (thin lines stay visible zoomed out; +33 % GPU memory for
the canvas texture).
- High-density screens: 100 % follows the system's display scaling, so a
  picture has the same size on a 3K laptop and an HD monitor; moving the
  window to another monitor keeps it the same size. View menu: "1 image
  pixel = 1 screen pixel" (XnView's option) and Interface size (75–150 %),
  both remembered.

**Under the hood**
- Brushes on their own thread (never split); merge of the changed areas off
  the UI thread, the full image only on the GPU.
- 64 × 64 tiled layers: an empty layer costs nothing.
- Idle = 0 % CPU; strokes far off the canvas pause instead of freezing.
- `fuzz.rs`: headless crash hunt on the real UI and paint thread;
  `painter.rs`: a painting session (sketch, big brushes, smudge, ink,
  fills, lasso copy / paste, undo, saves) with per-stroke timings
  (`PAINT_SECS=1200 cargo test --release -p aurastudio painter_session --
  --ignored --nocapture`).

---

## Controls

| Action | Input |
|---|---|
| Paint | pen / left mouse |
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Clear layer (or the selection) | Delete |
| Tools | B brush, E eraser, G fill, I picker, U line / rectangle / ellipse / arc, M rectangle selection, L lasso |
| Pick a color | Alt + click (brush, eraser, fill) |
| Select all / deselect | Ctrl+A / Ctrl+D |
| Copy / paste pixels | Ctrl+C / Ctrl+V |
| Zoom | mouse wheel (toward the cursor), `+` / `-`, Ctrl + Space + drag left / right (or Ctrl + middle drag), around where the drag starts |
| 100 % / fit | `1` / `2` |
| Pan | Space + drag, or middle-button drag |
| Rotate | Shift + Space + drag, or Shift + middle drag; `4` / `6` = ±15°, `5` = reset. Turns around the picture's center while it is all in view, else around the middle of the canvas area |

---

## Building and running

```
cargo run --release
```

The CI (`.github/workflows/build.yml`) builds Windows and Linux binaries
on every push (macOS on demand). The downloaded executable carries the
version in its name (`aurastudio-0.3.2.exe`), so several versions can be
kept side by side and run at the same time to compare them; each version
has its own autosave file. A local `cargo build` gives a plain
`aurastudio.exe`: copy it under a versioned name to keep it.

Environment variables (for testing and comparisons):

| Variable | Effect |
|---|---|
| `RUST_LOG=info` (default) | log: GPU, pen backend, monitor profile, per-stroke timings |
| `AURASTUDIO_BACKEND=dx12\|vulkan\|metal\|gl` | force a GPU backend |
| `AURASTUDIO_PRESENT=mailbox\|immediate\|fifo` | force a present mode |
| `AURASTUDIO_DISPLAY_ICC=<file.icc>` | use this monitor profile instead of the system's |
| `AURASTUDIO_BRUSH_DIR=<folder>` | where imported brushes are kept |
| `AURASTUDIO_DPI=<dpi>` | pretend the window's monitor has this DPI (96 = 100 %) |

Benchmarks:

```
cargo run --release -p aurastudio-brush  --example bench_stroke      # brush cost per sample
BENCH_PRESSURE=steady BENCH_RADIUS=500 ...                            # filters
cargo run --release -p aurastudio-layers --example bench_composite   # layer merge cost
cargo run --release -p aurastudio-brush  --example bench_myb -- brush.myb 300  # MyPaint brush cost per sample
cargo test  --release -p aurastudio -- --ignored pipeline --nocapture # paint thread + UI model
cargo run --release -p aurastudio-brush  --example myb_sheet -- out.png a.myb b.myb  # MyPaint sample sheet
cargo run --release -p aurastudio-brushes --example preview_sheet -- out.png [filter] # every brush preview
cargo run --release -p aurastudio-brush  --example spacing_sheet -- out.png        # round brush strokes (SHEET_LOOP=1, SHEET_RAW=1, SHEET_CONST=1)
```

---

## Versioning

One version for the whole app (`[workspace.package]` in `Cargo.toml`),
[Semantic Versioning](https://semver.org) MAJOR.MINOR.PATCH, listed in
`CHANGELOG.md` (no git tags or releases: personal project for now):

- **PATCH** (0.2.0 → 0.2.1): bug fixes only.
- **MINOR** (0.2.x → 0.3.0): new features; files from older versions still
  open.
- **MAJOR** (0.x → 1.0, then 1.x → 2.0): 1.0 when AuraStudio is ready for
  everyday work; after that, only for breaking changes (an `.aurs` format
  older versions can't read...).

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
- [x] Display color management with the real monitor profile.
- [x] A 1000 px brush on an A4 300 dpi canvas: no lag.
- [x] Rotation with the pen (Shift + Space + drag).
- [x] MyPaint brushes: smudge / blender behavior.
- [ ] Moving the window between a 3K and an HD screen: same picture size,
      interface follows (log line `display scaling: …`).
- [ ] Brush import dialog (files and folder) and that imports come back at
      the next launch.

**Priority: one tile manager for all the memory**
- [ ] **A single manager for every tile in memory** (Krita's tile data
      store), before the animation timeline (hundreds of drawings in
      memory). Each part keeps its own data (layers, stroke copies, merge
      caches, MyPaint working tiles, undo history), but all of it goes
      through the manager, which:
      - counts all the memory in one place, against a budget (e.g. 4 GB,
        set in the preferences);
      - when over budget, frees what can be rebuilt (merge caches) and
        writes the least recently used tiles to a swap file on disk;
      - shares identical tiles instead of copying them (copy on write: a
        copied tile is really copied only when it is changed).
      Today nothing caps the memory: 1.5 GB with 35 layers in the 20-minute
      painting test.

**Missing / planned**
- [ ] Smudge / blend strength sliders in Tool options (MyPaint brushes
      smudge too weakly by default).
- [x] Brushes docker: list of brush strokes (0.4.0), by medium (0.5.0).
- [ ] Krita brush bundles (.bundle) import.
- [ ] Redesign of the New document dialog.
- [ ] PSD / ORA export.
- [ ] Undo for structural layer changes (add, delete, move, opacity…) and
      canvas resize (history is cleared today).
- [ ] Persist per-brush settings and UI layout between sessions.
- [ ] Brush editor (MyPaint settings and curves); brush tilt / rotation input
      (WinTab orientation is already read).
- [ ] Document tabs (the workspace already handles several documents).
- [ ] Faster large MyPaint smudge brushes (acrylic / water / oil "only
      water", 500–1000 px: still 2–3× slower than the pen on a slow
      2-core machine; the time goes into reading the canvas colors).
- [ ] Pressure setting for the geometry tools (they paint at full pressure).
- [ ] Selection modes (add / subtract), invert, transform; undo for
      selection changes.
- [ ] Rooms / workspaces (painting, animation…) with lazy loading.
- [ ] Animation timeline (OpenToonz-inspired), comics panels.
- [ ] Native pen input on macOS / Linux (today: winit touch events).

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
- **Forge**, the brush engine: libmypaint's model (its ~60 settings, pen
  inputs and curves, stroke path and dab blending) grown with Krita's ways
  (curves that multiply, wash opacity, auto spacing, a dab on press).
  Brush formats are read into it: 56 MyPaint brushes (CC0, chosen from the
  196 of the MyPaint collection, `assets/brushes/catalog.txt`) and `.myb`
  import; Krita bundles next. Pressure and pen tilt. Each brush keeps its
  own size / opacity / hardness / blend mode; 13 blend modes. GPU brush outline cursor, app-drawn hand / rotate
  cursors.
- Toolbox by groups: Draw (pencil, ink, charcoal & pastel, misc), Paint
  (paint, watercolor, airbrush, blend & smudge, texture & FX), Eraser,
  Fill, Picker, Geometry, Selection, Transform. The Tools docker lists the group's
  tools or brushes (by medium); each group remembers its last tool and
  brush; its key pressed again goes to its next tool. Imported brushes
  get a medium from their name. Brushes and medium icons drag and drop;
  + makes a medium of one's own.
- Stroke previews, painted by the brush itself: in the Tools docker, the
  stroke's shape (geometry tools: their shape with the current brush);
  in Tool options, like Clip Studio Paint, a stroke leaving a light gray
  half (erasers cut it, blenders drag it out), repainted as size,
  hardness, opacity or blend mode change (on a thread of its own). A size
  change only grows the dabs, which keep their places and their grain:
  no shimmering.
- Mouse button bounce filtered for everything (a worn switch no longer
  drops a slider drag or ends a stroke); Windows' Alt menu mode is off.

**Tools** — toolbox on the canvas's left edge, centered (Krita's optimized
layout; Lucide icons across the UI): Brush, Eraser, Fill, Color picker,
Geometry, Selection, Transform.
- Fill referring to this layer, all layers or the reference layers
  (drafts left out or not), tolerance, expand under
  anti-aliased lines; an A4 page in ~10–50 ms.
- **Selections** (8-bit, soft edges): rectangle, ellipse, lasso, polygon
  (clicks; Enter, a double click or the first point closes it) and the
  **magic wand** (tolerance, contiguous or every like pixel, this layer /
  all layers / references, grow). New, add (Shift), subtract (Alt),
  intersect (Shift+Alt); anti-aliasing and feather in the options; a drag
  inside the selection moves its outline. Select menu: all, deselect,
  reselect, invert, grow, shrink, feather, border (round, as Photoshop's),
  from layer (its opaque pixels). Every change is undoable. Marching ants
  traced at the zoom's detail. A thin cross shows the mode (+, −, ×).
- **Loupe** (picker and selection tools, in their options): a magnified
  view around a 1-pixel cross, to take one exact pixel's color or to put
  a selection's points on pixel corners; the picker's rim shows the color
  taken, the selection's the mode (+, −, ×).
- Painting, fill, filters, copy and Delete stay inside the selection (a
  soft edge partly).
- **Transform** (T, Ctrl+T): the layer, the layers chosen in the Layers
  docker or a folder's, or only what is selected (the selection moves
  along). Free: move (Shift: 8 directions), scale by the corners
  (proportions kept, Shift: free; Alt: from the pivot) and the edges,
  rotate outside the box (Shift: 15° steps), skew along an edge, Ctrl+corner
  to distort, a pivot to drag; X / Y, size, angle and skew as numbers;
  flips and quarter turns. Perspective: each corner on its own. Warp: a
  grid (1 × 1 to 8 × 8) to bend; a point moves only the cells around it
  (the points around stay pinned). Resampling Bicubic (Photoshop's),
  Bilinear or Nearest (pixel art), with a pyramid for big reductions.
  While dragging, the GPU draws it at once (the layers over it on top);
  on release, the exact result at full quality, on all cores.
  Enter applies (one undo step), Escape cancels, Ctrl+Z steps back inside
  the transform, arrows nudge (Shift: 10 px). On the tool with no
  transform under way, Enter (or Ctrl+T) starts a new one.
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
- Start screen with recent files. New document dialog (laid out like
  PhotoCraft's): Illustration, Animation and Comics tabs of preset cards
  (paper sizes; HD to 8K and the DCI 2K / 4K cinema sizes, flat and
  scope; manga B4 / A4 / B5 / B6 manuscripts at 600 dpi, US comic with
  bleed, BD album, webtoon), units px / mm / cm / in, orientation.
- **Memory report** (Window > Memory report): every megabyte the app
  holds, by name (tiles by layer, history, GPU textures, the driver's
  allocator), beside the system's figures, the start-up timed step by step;
  Copy to paste it. Icons and layer thumbnails share a few atlas pages; stroke previews
  are stacked in a few textures (only the visible ones stay on the GPU) and cached on
  disk between runs.
- **Document types**: the tab a document is made from is its type
  (Illustration, Animation, Comics), kept in the file and shown in the
  menu bar. It turns features on or off, the interface stays the same:
  animation and camera folders only in an Animation document (grayed out
  elsewhere). Files from before it open as Illustration.
- **Animation folders** (Animation documents; the arrow beside the
  New folder button, or Layer > New special folder): one folder per
  element (a character, smoke...). Its drawings are **sheets**; frames are
  the time. Drawing on a frame with no sheet there makes a sheet exposed
  from that frame, with the folder's cel template inside it: genga,
  douga, ink, colo by default. One undo step takes the sheet and its
  first stroke away together. A sheet shows until the next one (hold);
  before the first one, nothing. Sheets are named by their order in time
  (Sheet 1, Sheet 2...: "Sheet 3 at frame 12"). Strokes go to the cel
  chosen in the **Cel template** docker (the same one in every sheet);
  its eye and lock are the sheet's, and **Apply to folder** gives every
  sheet (and those to come) these states. The template (listed like the
  Layers docker, each new cel on top): add, remove (with a question when
  drawings would go), drag to move, rename (double click) a cel, mark the
  reference one, in every sheet at once. The layer list shows the folder
  and its sheets (a click on a sheet goes to its frame), never the cels.
  The vocabulary: **Group → Folder → Sheet → Cel**. A **group** gathers
  animation folders (organisation only, no nesting): right click >
  Group animation folders; one column in the timeline, a double click
  opens it. Nothing else goes into an animation folder; sheets stay in
  theirs. `,` / `.`: previous / next frame. Camera and comics folders are
  listed, grayed out until they come.
- **Animation panel** (Animation documents; Window > Animation): the
  **timeline** under the canvas, or turned into the **X-sheet** beside the
  dockers (one button; the same grid, as OpenToonz's orientations). One
  row / column per animation folder (name, eye, lock; double click to
  rename), one cell per frame: a sheet is a filled cell at the frame it
  is exposed from, a hold a line until the next one; the current frame
  lit; shading, lines and numbers on the beat (every 6 frames at 24 fps,
  every 5 at 25, 30, 60) and a line every second. A click goes to a frame
  (on a cell: that folder too), a drag on the frame numbers scrubs; a drag
  on empty cells draws a box that picks sheets (also Shift range, Ctrl
  add; one folder at a time). **Drag the sheets picked** to move them,
  **Ctrl + drag** to copy them; onto another sheet, a question: replace
  it, swap them (moves), or cancel (the default). **Insert / remove a
  frame** (buttons, or a right click on a cell): the next sheets move one
  frame later / earlier; delete a sheet (the one before holds on). The
  bar: new animation folder, first / previous / **play-pause** / next /
  last frame, loop, fps, current frame / frame of the last sheet.
  Playback keeps time (frames are skipped if the merge can't keep up),
  from the current frame, looping.
- **Onion skin** (the strip above the frame numbers, OpenToonz's way): the
  switch at the current frame (half the "before" color, half the "after"
  one) turns it on or off, and drags through time; the inner lane holds
  mobile marks (sheets or frames from the current one, Preferences >
  Animation), the outer lane fixed marks (blue: a frame whose sheet stays
  shown). Click a mark to set or remove it (yellow: where a click would
  go), drag to set several, right click for its opacity. Sheets before
  are tinted with one color, after with the other, fading with distance.
- **Window** menu: every docker with a check mark, to hide it or show it
  again (beside its usual partner, else in a new dock at the bottom);
  Reset docks.
- Open PNG / JPG / WebP / BMP / TIFF / GIF: the picture is the bottom layer
  and an empty layer above it is active (no painting on the picture by
  mistake). Export to PNG / JPG.

**Filters** — Filter menu: Brightness / Contrast, Levels (per channel,
Auto), Hue / Saturation (six color ranges, colorize), Black & White (six
color weights, tint), Invert, Posterize, Threshold, Sepia, Opacity,
Gaussian blur, Unsharp mask, Thicken / Thin lines (on line art and on
paper), Find edges, Emboss, Drop shadow. Tones and colors behave as
Photoshop's (as PhotoCraft measured them); blur, sepia, invert and opacity
follow the W3C Filter Effects spec.
- Each dialog shows what the filter does: the layer before and after side
  by side (drag the line; zoom and pan, 100 % shows the real pixels),
  the histogram before and after with the tone curve and the clipping,
  the painting's main colors before and after, the hue spectrum as it
  turns, a hard edge or a line as the blur, sharpening or thickening
  leaves it, a lit sample for Emboss and Drop shadow, an angle dial for
  the light. Sliders with colored tracks (Shift: finer, double-click:
  default), values typed in.
- The layer shows the result too (Preview on canvas), inside the
  selection; Apply is one undo step, Cancel or Escape puts the layer back.
  A blur of an A4 300 dpi layer: ~0.4 s on 2 cores.

**Preferences** — Edit > Preferences… (Ctrl+K): interface size, brush
strokes in the Tools docker, open maximized, renderer (Vulkan, DirectX 12
on Windows, OpenGL; another one is used when it can't run, a restart
switches), HiDPI scaling, color management and intent, brush outline
cursor, click debounce, autosave delay. Kept in `settings.txt`.

**Layers** — Paper + Layer 1 for new documents, folders, opacity, blend,
lock, rename, undo / redo with a memory budget (a stroke keeps only the
tiles it changed: undoing a long diagonal stroke is as quick as a short
one). What the Layers docker
does is undoable too (new, delete, move, group, clip, marks, label,
opacity, blend, name, filter layer values), a deleted layer coming back
with its pixels; hiding a layer is not an undo step (Photoshop, CSP).
- **Clipping masks** (CSP / Photoshop): a layer shows only where the
  layer (or folder) under it has pixels; several can clip to the same
  base; the base's opacity fades the whole group (Ctrl+Alt+G). A sky blue
  chain links the rows.
- **Filter layers** (Krita): any filter kept editable over what is under
  it (double-click to change it), clipped too; a whole merge with them
  runs on the paint thread.
- **Reference layers** (CSP's lighthouse): what the fill tool can refer
  to, and what "Stay inside the lines" keeps the brushes in (CSP's
  anti-overflow: paint the colors on their own layer without crossing the
  line art). One per folder: a folder's layers look at its own reference,
  else at the one of the folder around it. **Draft layers** (CSP's pencil): shown, but left out of
  fills, references and exported images.
- **Color labels** (Krita's eight colors), the list filtered by them
  (Krita, Label Box); folders stand out, Group / Ungroup (Ctrl+G,
  Ctrl+Shift+G, one button); a right click on a row for all of it.
- Several layers at once: Ctrl+click and Shift+click select them, to
  group, drag, duplicate, delete, label, clip or mark them together.
  Dragging a layer (or a brush) to the edge of its list scrolls it.

**View** — GPU zoom / pan / rotation, monitor ICC color management.
Canvas quality (Preferences > Canvas & display): **High** (default) shows
in every screen pixel the exact average of the canvas pixels under it,
rotation included (thin lines stay visible zoomed out, no mipmaps, nothing
rebuilt after a stroke); **Standard** uses mipmaps (lighter on old graphics
chips, +33 % GPU memory for the canvas texture). Close up, canvas pixels
stay sharp squares, or are blended ("Smooth zoom above 100 %"). Frames are
shown as soon as drawn (lowest pen latency), or in step with the screen
("V-Sync"). While painting, every pen sample is painted, but the canvas
is merged and shown at the **canvas frame rate** (Preferences > Canvas &
display, 24 to 240 fps, 60 by default): set it to the screen's refresh
rate; more only adds work nobody sees.
- High-density screens: 100 % follows the system's display scaling, so a
  picture has the same size on a 3K laptop and an HD monitor; moving the
  window to another monitor keeps it the same size; the hand, rotate and
  zoom cursors are drawn at the screen's scaling. Preferences: "HiDPI
  image scaling" off = 1 image pixel per screen pixel (XnView's option),
  and Interface size (75–150 %).

**Under the hood**
- Brushes on their own thread (never split); merge of the changed areas off
  the UI thread, the full image only on the GPU.
- 64 × 64 tiled layers: an empty layer costs nothing.
- Idle = 0 % CPU; strokes far off the canvas pause instead of freezing.
- A crash keeps the work: the document goes to the recovery file, offered
  at the next start, and a message says so.
- `fuzz.rs`: headless crash hunt on the real UI and paint thread;
  `painter.rs`: a painting session (sketch, big brushes, smudge, ink,
  fills, lasso copy / paste, undo, saves) with per-stroke timings
  (`PAINT_SECS=1200 cargo test --release -p aurastudio painter_session --
  --ignored --nocapture`). `FUZZ_SECS=60` runs the fuzz test for a minute.
- `pure.rs`: millions of random operations on the model and the paint
  thread, no interface, with checks that must agree (undo n + redo n,
  part of the merge = merge of the part, screen = merge, file = document):
  `PURE_OPS=3000000 cargo test --release -p aurastudio pure_fuzz --
  --ignored --nocapture`. `PAINT_OPS` / `PAINT_SIZE=310x438` run the
  painter session for a number of actions on a smaller page.

---

## Controls

| Action | Input |
|---|---|
| Paint | pen / left mouse |
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Clear layer (or the selection) | Delete |
| Tools | B brush, E eraser, G fill, I picker, U line / rectangle / ellipse / arc, M selection (again: rectangle, ellipse, lasso, polygon, magic wand), T transform |
| Pick a color | Alt + click (brush, eraser, fill) |
| Select all / deselect / reselect / invert | Ctrl+A / Ctrl+D / Ctrl+Shift+D / Ctrl+Shift+I |
| Selection modes | Shift: add, Alt: subtract, Shift+Alt: intersect (held at the press) |
| Transform | Ctrl+T; Enter apply, Escape cancel, arrows nudge; Enter again: a new one |
| Animation | `,` previous frame, `.` next frame; in the timeline / X-sheet: drag a frame to move it, Ctrl + drag to copy it, wheel through time, Shift + wheel across folders |
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
| `RUST_LOG=info` (default) | log: GPU, pen backend, monitor profile, files opened and saved |
| `AURASTUDIO_BACKEND=dx12\|vulkan\|metal\|gl` | force a GPU backend |
| `AURASTUDIO_PRESENT=mailbox\|immediate\|fifo` | force a present mode (default: immediate, fifo with V-Sync) |
| `dxcompiler.dll` next to the program | DirectX 12 compiles shaders with DXC instead of FXC |
| `AURASTUDIO_DISPLAY_ICC=<file.icc>` | use this monitor profile instead of the system's |
| `AURASTUDIO_BRUSH_DIR=<folder>` | where imported brushes are kept |
| `AURASTUDIO_DPI=<dpi>` | pretend the window's monitor has this DPI (96 = 100 %) |

### Profiling (Tracy)

Durations are measured with the [Tracy](https://github.com/wolfpld/tracy)
profiler, in a build of its own:

```
cargo run --release --features profile
```

Without a Rust toolchain: GitHub, Actions, Build, **Run workflow** with
"Also build the Tracy profiler version" ticked; the run's artifacts then
hold `aurastudio-<version>-Windows-profile`, two builds in two folders
(keep each `.pdb` beside its `.exe`): `mimalloc` (the app's default
allocator) and `system` (Windows' allocator), to compare them. The
Memory report's first line names the allocator.

Locally: `--no-default-features` builds with the system allocator.

Then open the Tracy window (**version 0.14.x**, the protocol the Rust
client speaks; Windows builds are on its releases page) and click
Connect. It shows every frame, the main, paint and save threads side by
side with their zones (window events, UI build, docks, egui, canvas
upload, brush samples and dabs, merges, fills, filters, transforms,
playback tiles, saves and opens), and plots: heap MB, the paint thread's
queue, the undo history. Without `--features profile` the zones compile
to nothing. The profile build announces itself on the local network for
Tracy to find it: run it only when profiling.

Memory stays with the Memory report (Window > Memory report): Tracy sees
neither the GPU allocator nor Windows' figures.

A debug build (`cargo run`) has its dependencies optimized: fast enough to
draw with, quick to rebuild.

### Crash reports

A crash writes `crash-<date>.txt` (version, graphics, pen backend, panic
message, backtrace) in `%APPDATA%\AuraStudio\crashes` (Linux:
`~/.local/share/AuraStudio/crashes`); the message shown names the file.
The 20 newest are kept. Release builds keep file and line tables: keep
the `.pdb` beside the `.exe` (the CI build ships it) so the backtrace
names the functions.

Benchmarks:

```
cargo run --release -p aurastudio-forge  --example bench_stroke      # brush cost per sample
BENCH_PRESSURE=steady BENCH_RADIUS=500 ...                            # filters
cargo run --release -p aurastudio-layers --example bench_composite   # layer merge cost
cargo run --release -p aurastudio-forge  --example bench_myb -- brush.myb 300  # MyPaint brush cost per sample
cargo test  --release -p aurastudio -- --ignored pipeline --nocapture # paint thread + UI model
cargo run --release -p aurastudio-forge  --example myb_sheet -- out.png a.myb b.myb  # MyPaint sample sheet
cargo run --release -p aurastudio-forge-library --example preview_sheet -- out.png [filter] # every brush preview
cargo run --release -p aurastudio-forge  --example myb_compare -- a.myb out.ppm     # Forge vs real libmypaint (tools/mypaint-reference/ref.c)
cargo run --release -p aurastudio-forge  --example spacing_sheet -- out.png        # round brush strokes (SHEET_LOOP=1, SHEET_RAW=1, SHEET_CONST=1)
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
| `aurastudio-core` | shared types, no dependencies: `Rect`, `TILE`, `PaintSurface`, blend modes |
| `aurastudio-raster` | the tiles layers are stored in, and their bytes in a file |
| `aurastudio-selection` | selections (masks, polygons, ellipses, boolean modes) |
| `aurastudio-transform` | free transform and mesh warp, resampling |
| `aurastudio-tools` | what tools do with the pen, apart from the window: shapes, multi-arc, transform box |
| `aurastudio-forge` | Forge, the brush engine (`src/forge`), `.myb` reader |
| `aurastudio-forge-library` | brush library: built-in presets, bundled MyPaint collection, import |
| `aurastudio-layers` | layer tree, folders, lock, compositing, fill, sheets and onion skin |
| `aurastudio-animation` | animation tools on the stack: playback cache |
| `aurastudio-document` | documents, undo history, selection commands, the transform under way, `Edit` (every change the interface can make to the layers) |
| `aurastudio-color` | monitor ICC profile, sRGB → display transform (3D LUT) |
| `aurastudio-gpu` | wgpu renderer of the canvas: texture, view transform, color LUT, brush outline, playback; no window system, no interface |
| `aurastudio-input` | Windows pen input (WinTab, Windows Ink), mouse bounce filter |
| `aurastudio-file` | `.aurs` format, image import / export |
| `aurastudio-filters` | Filters: Photoshop's tones and colors, W3C Filter Effects blurs |
| `aurastudio-ui` | egui interface: bars, toolbox, one file per docker, theme, widgets; reads the layers, changes them with `Edit`s |
| `aurastudio-app` | the application: event loop, window (egui over the canvas), paint thread, projection |
| `aurastudio-arch` | tests of the crate rules (who depends on whom, who touches single tiles, edits only through `Edit`) |

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
- [ ] Transform and warp on a big page with the pen: the image and the
      grid stay under the pen (GPU preview; log line `transform lift: … ms`).
- [ ] Loupe with the pen: picking one pixel, selection points on pixel
      corners.
- [ ] Renderer: DirectX 12 and OpenGL chosen in Preferences, after a
      restart (log line `graphics: …`).

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

**Parked: stroke previews, to redo cleanly**
- [ ] Geometry tool previews no longer show (0.7.3).
- [ ] Study PhotoCraft's Brush settings preview (`crates/ui-egui/src/brush_preview.rs`,
      `brush_panel.rs`) and rewrite ours from a clean base: it grew one
      fix at a time.
- [ ] Photoshop's behavior: the Brushes docker's previews show each
      preset as saved and never change; only the Brush settings (Tool
      options) preview follows the sliders.
- [ ] Keep what 0.7.3 measured: dabs in place along the path and grain
      by place (a size change only grows the dabs), no jumps.

**Next: Forge**
- [ ] Advanced tool options popup (gear at the bottom right of Tool
      options, F5): every Forge setting and its curves; Tool options keeps
      the common ones.
- [x] MyPaint brushes compared with the real libmypaint (0.7.1): the 56
      catalog brushes paint like it (`myb_compare`).
- [ ] The 5 Dieterle brushes in pigment mode (Posterizer paints almost
      nothing): compare with libmypaint's pigment path (`stroke_to_2`).
- [ ] A global pressure curve (preferences); the size / hardness sliders
      override the brush's base values today.
- [ ] Krita brushes: .kpp / .bundle reader, brush tips and textures,
      Krita's Color Smudge; then a smudge / blend as strong as CSP's.

**Missing / planned**
- [ ] Smudge / blend strength sliders in Tool options (MyPaint brushes
      smudge too weakly by default).
- [x] Brushes docker: list of brush strokes (0.4.0), by medium (0.5.0), tool groups and Tools docker (0.6.0).
- [ ] Krita brush bundles (.bundle) import.
- [x] Redesign of the New document dialog (0.8.0).
- [ ] PSD / ORA export.
- [x] Undo for the layer changes (0.10.1).
- [ ] Undo for canvas resize (history is cleared today); layer steps are
      not kept in the file (its pixel steps are).
- [ ] Persist per-brush settings and UI layout between sessions.
- [ ] Barrel rotation input (WinTab orientation is already read).
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

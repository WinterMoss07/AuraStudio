# AuraStudio

An all-in-one app for drawing, painting, animation and comics, in Rust,
from the first line to the final render. Ideas from Krita, OpenToonz and
Clip Studio Paint, written anew (we study how they work, we don't
translate their code).

Rule: every part is measured (memory, latency, idle CPU) before the next
one starts. The interface stays quiet; the painting gets the room.

> Status: early, usable for painting tests. Main target: Windows (pen via
> WinTab / Windows Ink). Linux and macOS build and run with mouse / touch.

## Features

- **Brushes**: Forge, a Photoshop-model engine (from PhotoCraft): pressure,
  tilt, 33 blend modes, mixer brushes. Imports Photoshop `.abr` and
  Procreate `.brush` / `.brushset`.
- **Tools**: brush, eraser, fill (this layer, all layers or references),
  picker with loupe, line / rectangle / ellipse / arc, selections
  (rectangle, ellipse, lasso, polygon, magic wand; add, subtract,
  intersect; grow, shrink, feather, border), transform (free,
  perspective, warp grid; GPU preview while dragging).
- **Layers**: folders, opacity, blend, lock, clipping masks, filter
  layers, reference and draft layers, colour labels. Every change is
  undoable.
- **Animation**: animation folders of sheets and cels (Group › Folder ›
  Sheet › Cel), cel template, timeline or X-sheet, onion skin, playback.
- **Filters**: tones and colours (as Photoshop's), blurs (W3C Filter
  Effects), sharpen, thicken / thin lines, edges, emboss, drop shadow;
  live preview, one undo step.
- **Files**: `.aurs` documents (layers, undo steps, preview), autosave and
  crash recovery. Opens PNG / JPG / WebP / BMP / TIFF / GIF, exports PNG /
  JPG.
- **View**: GPU zoom, pan, rotation; monitor ICC colour management; canvas
  quality High (exact average, no mipmaps) or Standard (mipmaps).
- **Memory report** (Window > Memory report): every megabyte held, by name.

## Controls

| Action | Input |
|---|---|
| Undo / redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Clear layer (or selection) | Delete |
| Tools | B brush, E eraser, G fill, I picker, U shapes, M selection, T transform |
| Pick a colour | Alt + click |
| Select all / deselect / reselect / invert | Ctrl+A / Ctrl+D / Ctrl+Shift+D / Ctrl+Shift+I |
| Transform | Ctrl+T; Enter applies, Escape cancels, arrows nudge |
| Previous / next frame | `,` / `.` |
| Copy / paste | Ctrl+C / Ctrl+V |
| Zoom | wheel, `+` / `-`, Ctrl + Space + drag |
| 100 % / fit | `1` / `2` |
| Pan | Space + drag, or middle drag |
| Rotate | Shift + Space + drag; `4` / `6` ±15°, `5` reset |

## Build and run

```
cargo run --release
```

CI (`.github/workflows/build.yml`) builds Windows and Linux on every push
(macOS on demand). The executable carries its version in its name, so
several versions can run side by side; each has its own autosave.

| Variable | Effect |
|---|---|
| `RUST_LOG=info` (default) | log: GPU, pen, monitor profile, files |
| `AURASTUDIO_BACKEND=dx12\|vulkan\|metal\|gl` | force a GPU backend |
| `AURASTUDIO_PRESENT=mailbox\|immediate\|fifo` | force a present mode |
| `AURASTUDIO_DISPLAY_ICC=<file.icc>` | monitor profile to use |
| `AURASTUDIO_BRUSH_DIR=<folder>` | where imported brushes are kept |
| `AURASTUDIO_DPI=<dpi>` | pretend the monitor has this DPI (96 = 100 %) |
| `AURASTUDIO_MEMLOG=1` | log the memory report |

**Profiling**: `cargo run --release --features profile`, then connect
[Tracy](https://github.com/wolfpld/tracy) **0.14.x**. Without a toolchain:
Actions > Build > Run workflow, "Also build the Tracy profiler version".
`--features mimalloc` builds with mimalloc (system allocator by default).

**Crash reports**: `crash-<date>.txt` in `%APPDATA%\AuraStudio\crashes`
(Linux: `~/.local/share/AuraStudio/crashes`), the 20 newest kept. Keep the
`.pdb` beside the `.exe` for named functions in the backtrace.

## Versioning

One version for the whole app (`Cargo.toml`), [SemVer](https://semver.org),
listed in `CHANGELOG.md`: PATCH for fixes, MINOR for features (older files
still open), 1.0 when ready for everyday work.

## Code layout

| Crate | Role |
|---|---|
| `core` | shared vocabulary, small pure helpers |
| `input` | everything that points (pen, mouse, touch) |
| `raster` | pixels of one surface (tiles) |
| `filters` | pixel operations and their parameters |
| `transform` | homography, mesh warp, resampling |
| `tools` | pure logic of geometric tools |
| `selection` | masks, ways to make and change them, outline |
| `color` | display pipeline, profile conversions |
| `layers` | the tree |
| `compose` | the merged image, stroke and folder caches |
| `animation` | what time does to the tree; camera |
| `document` | tree, timeline, history, `apply(Edit)`, clipboard |
| `file` | `.aurs`, image import / export |
| `gpu` | wgpu canvas, display, view, transform preview |
| `ui` | egui interface |
| `app` | the binary: window, event loop, paint thread |
| `forge` | brush engine and presets |
| `diag` / `stress` | diagnostics and fuzzers (never shipped) |
| `arch` | tests of the crate rules |

Data crates know nothing of the GPU or the interface. Third-party code
and assets: `NOTICE.md`.

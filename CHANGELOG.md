# Changelog

Versions follow [Semantic Versioning](https://semver.org): MAJOR.MINOR.PATCH
(see README, Versioning).

## 0.3.1 — 2026-10-18

- Hidden layers show a closed eye.
- "Clear list" for the recent files on the start screen; long file names
  end with "…" on their cards.
- Color docker: HSV / RGB tabs under the color preview.
- Copy without a selection takes only the painted part of the layer (it
  copied the whole page, hundreds of MB on big canvases).
- Arc tool: a click lost after the window lost focus mid-arc.
- New stress test: a 20-minute painting session (`painter.rs`).

## 0.3.0 — 2026-10-18

**New**
- Copy / paste pixels (Ctrl+C / Ctrl+V, Edit menu, toolbar): the selection
  (or the whole layer) of the active layer, pasted in place on a new
  layer; images copied in other apps paste too (system clipboard).
- Fixed toolbar under the menu: New, Open, Save, Copy, Paste (each room
  will get its own); tool settings stay in the Tool options docker.

**Changed / fixed**
- Arc tool works like OpenToonz's: the cursor is the curve's control point
  (drag toward the bottom right, the curve bulges there), dashed control
  lines; chained arcs make one stroke, painted when the outline is done
  (Enter, a click on its last point, or an end dropped on the first point
  to close it); Ctrl+Z takes back the last arc, Escape cancels. Circle arc
  mode removed.
- Brush size: 1 px and odd sizes (3, 5 px...) are possible (the slider
  jumped back to 2 px).
- Layer list background is the canvas area's dark gray.

## 0.2.0 — 2026-10-18

**New**
- Logo (window, taskbar, .exe, start screen); UI accent in the logo's
  blue-violet.
- Mipmapped canvas: thin lines (2–3 px) stay visible when zoomed out.
- Geometry tools drawn with the current brush preset: line, rectangle,
  ellipse, arc (free curve or circle arc, chainable for closed outlines).
- Tools: toolbox on the canvas edge, eraser, fill (merged image or layer,
  tolerance, expand), color picker (Alt+click), rectangle and lasso
  selections that limit painting, fill and Delete.
- High-density screens: 100 % zoom follows the display scaling, also when
  the window moves to another monitor; "HiDPI image scaling" and
  Interface size in the View menu.
- Files: `.aurs` documents with undo history, autosave and crash recovery,
  start screen with recent files, New dialog presets, image import /
  export; opened images get an empty layer above them.
- Tiled layers (empty layers cost no memory), projection kept only on the
  GPU.
- Lucide icons across the UI.

**Fixed**
- Strokes far off the canvas no longer freeze the paint thread.
- Mouse strokes start where the button went down.
- Long layer names no longer run under the lock icon; Color docker scroll
  bar.

## 0.1.0

First painting builds: brush engine and MyPaint brushes, layers and
folders, undo / redo, zoom / pan / rotation, display color management.

# Changelog

Versions follow [Semantic Versioning](https://semver.org): MAJOR.MINOR.PATCH
(see README, Versioning).

## 0.7.3 — 2026-10-19

**Stroke previews: steady while the size changes**
- A size change only grows or shrinks the dabs, like Clip Studio Paint:
  they keep their places along the preview's path (the brush's own
  density, rounded to a spacing of ½, 1, 2, 4... px, with a fade between
  two steps), and each dab's random numbers come from its place: textures
  grow instead of being redrawn. Before, the dabs slid along the stroke
  (it seemed to move back and forth) and MyPaint textures were redrawn at
  every step.
- Brushes whose dynamics paint past their radius get a smaller preview
  size continuously (measured once per brush), instead of jumping to 60 %.
- Geometry previews: 1 to 15 px thick for what the brush really paints
  (a pressure-growing brush no longer draws donuts), no more jumps.
- The previews sit on whole screen pixels, drawn without filtering: no
  more shimmering edges.
- Measured over the whole Size slider (197 steps), steps where the
  preview jumps: Round pen 11 → 0, Liner 63 → 0, Fine charcoal 169 → 7,
  Watercolor 167 → 9, Oil 194 → 47 (its bristles grow in place).
- Forge: `Spacing::Fixed` and `ForgeOptions::seeds_by_place`, used by the
  previews only (strokes on the canvas are unchanged).

## 0.7.2 — 2026-10-19

**New**
- Mediums of the Tools docker: their icons drag and drop to reorder them
  (a red line shows where); the + after the last one makes a medium of
  one's own (named right away; right click: rename, or remove once
  empty). Order and own mediums are kept (`mediums.txt` in the brush
  folder).
- Moving a dock: a red bar shows where it goes between docks, the red tab
  bar the dock it would join as a tab (CSP), instead of egui_dock's drop
  buttons. Docks no longer float out as windows.

**Fixed**
- Without a document, the docks are grayed out like the toolbox.
- AuraStudio opens maximized; restored, its window fits the screen and is
  centered (it opened half off an HD screen at 150 %).

## 0.7.1 — 2026-10-19

**Fixed**
- Big brushes faster (1000 px: ~25 to 35 % less time per dab): the opaque
  core of a dab skips the edge math. Same pixels.
- MyPaint brushes start their stroke from no pressure, like in MyPaint
  (whose brushes always get the pen's hover events at pressure 0): a stroke
  fades in over its first sample instead of starting with a full dab.

**Checked**
- Forge against the real libmypaint (1.6): the 56 catalog brushes paint
  like it (oil, acrylic, wet paint... included). New tools for it:
  `tools/mypaint-reference/ref.c` and the `myb_compare` example. Not
  checked yet: the pigment mode of the 5 Dieterle brushes.

## 0.7.0 — 2026-10-19

**Forge, one brush engine**
- AuraStudio's brush engine is now **Forge** (`aurastudio-brush/src/forge`):
  hokusai's port of libmypaint, moved into the project, becomes the one
  engine, and AuraStudio's own engine (dab engine) is gone. Its model,
  libmypaint's ~60 settings with their pen inputs and curves, is the one
  every brush format is read into; `.myb` files are now only a reader.
- Krita's ways added to it:
  - curve modes per setting: MyPaint's (base + curves) or Krita's (base ×
    curves, combined by product, sum, max, min or difference);
  - wash painting (Krita's indirect painting, with the blend mode) next
    to MyPaint's build-up;
  - Krita's auto spacing; a dab where the pen lands and where it lifts.
- The round brushes (Round pen, Soft round, Round eraser) are Forge
  brushes made the Krita way: opacity × pressure, by multiplying curves.
  They paint as before (a few pixels differ by 1/255).
- The 56 MyPaint brushes paint exactly as before (reference renders).
- Clipping of the path to the canvas moved from the paint thread into
  Forge (the stroke previews get it too); the first sample of a stroke is
  no longer fed twice.
- A per-dab environment check left in release builds (trace of the
  inputs) now runs in debug builds only.

## 0.6.4 — 2026-10-19

**Brush engine, step 1 of the fusion**
- One engine front for every brush format (`aurastudio_brush::BrushEngine`):
  pen samples in, changed areas out, one stroke lifecycle. The paint
  thread and the stroke previews no longer have a code path per engine.
  Krita's Pixel and Color Smudge rules will plug in behind it.
- Pen tilt reaches the brushes: read from WinTab (azimuth / altitude) and
  Windows Ink (tilt x / y), passed to MyPaint brushes (their tilt inputs
  did nothing until now). The mouse sends none.
- Reference renders (`aurastudio-brushes/tests/reference.rs`): every
  catalog brush paints fixed strokes, compared by hash; any pixel the
  coming refactors change shows, by brush.

**Changed**
- Drag and drop: a red line shows where an item lands (CSP); a medium
  icon gets a red frame when a brush is about to go there.
- Geometry stroke previews drawn with the brush again, 1 to 10 px thick
  (logarithmic over 1..1000 px: every size change shows).
- Size slider: 100 px in the middle (1..1000, exponential: each step a
  ratio, finer for small brushes).

## 0.6.3 — 2026-10-19

**New**
- Layers drag and drop: above, below, or into a folder (a line or the
  folder outlined shows where; the name follows the pointer).
- Brushes drag and drop in the Tools docker: to another place in the
  list, or onto another medium's icon (a watercolor brush into Paint).
  The order and mediums are kept (layout.txt in the brush folder).

**Fixed**
- Alt + a letter no longer triggers the letter's shortcut (Alt+E picked
  the eraser).
- Locked layers could be deleted; now refused, also a layer in a locked
  folder or a folder with a locked layer inside ("Locked: unlock it
  first"), and the delete button is greyed out.

**Changed**
- Geometry tools in the Tools docker: fixed thin outlines, like CSP; the
  brush they paint with shows in Tool options (its usual stroke preview).

## 0.6.2 — 2026-10-19

**Fixed**
- Sliders lagging behind the cursor, freezing then jumping: frames waited
  for WM_PAINT, which Windows only sends once the message queue is empty,
  and winit registered raw mouse input (a message per mouse report, 1000
  per second on many mice) that kept it busy. Raw input is off (unused),
  and frames are drawn as soon as they are due instead of waiting for the
  window system; a slider or the view being dragged is redrawn right in
  the pointer event. Frames while painting stay capped at one per 4 ms.

**Changed**
- Tool options: no tool descriptions; a few words on hover instead (the
  tool's name in Tools and Tool options). "Chain" is now "Multipoint".
  Shorter hover texts elsewhere (View menu, New dialog, start screen).
- Code review: dead code removed (MyPaint PNG previews and their decoder,
  unused workspace / document API, undo step labels nobody read, an unused
  MyPaint helper, a test-only merge function), duplicate code merged (one
  notice event, one edit report on the paint thread, one place where the
  tool changes), outdated comments fixed, long comments shortened. The
  brush field `group` is now `medium`.

## 0.6.1 — 2026-10-19

- Stroke preview trembling while the size changed: the dabs were spaced
  by the automatic spacing (√size), so each size moved them along the
  stroke (rings at its tapered ends shifting). Previews now space dabs in
  proportion to the size: the stroke scales smoothly. MyPaint previews
  restart their random sequences each time (same settings, same picture).
- MyPaint brushes showed "Opacity 0 %" (Digital brush...): their own
  opacity is 0 and comes from pen pressure. The Opacity slider is now a
  multiplier of the brush's own opacity: 100 % is the brush as made.
- Mouse click debounce: 20 ms instead of 50 (old mice bounce for 15-20
  ms), set in View > Mouse click debounce (Off to 80 ms). A release
  shorter than 150 ms that got through is logged ("mouse button up for
  N ms only"): a slider letting go then jumping back under the cursor
  is that, a bounce longer than the debounce.
- Saving and autosaving hold the document (the interface waits) only to
  copy the layers; compressing them is done by the save thread.
- Frames over 100 ms and the interface waiting for the document over
  100 ms are logged ("slow frame", "UI waited").

## 0.6.0 — 2026-10-19

**New**
- The toolbox shows groups of tools: Draw, Paint, Eraser, Fill, Picker,
  Geometry, Selection (7 icons instead of 10; new tools join a group).
  The Brushes docker becomes **Tools**: it lists the chosen group's
  brushes (Draw: pencil, ink, charcoal & pastel, misc; Paint: paint,
  watercolor, airbrush, blend & smudge, texture & FX; Eraser) or tools
  (Geometry: line, rectangle, ellipse, arc drawn with the current brush;
  Selection: rectangle, lasso). Each group remembers its last tool and
  brush. Keys: B Draw, P Paint, E Eraser, G Fill, I Picker, U Geometry,
  M Selection; pressed again, the group's next tool.
- A round eraser (AuraStudio engine) in the Eraser group.

**Changed / fixed**
- Stroke previews (renamed from brush previews), like Clip Studio Paint.
  Tools docker: the stroke alone (erasers and blenders shown as if they
  painted: their shape). Tool options: the left half is light gray paint,
  the right half the canvas desk (#181818); the stroke leaves the paint:
  brushes paint over both, erasers cut through, blenders drag it out.
  No more colored first stroke.
- The selected brush's row in the Tools docker lagged behind Tool options
  while dragging a slider (Tool options always went first); they are now
  painted in turn, one request per strip per frame.

## 0.5.0 — 2026-10-18

**New**
- Brushes sorted by medium: Pencil, Ink, Charcoal & pastel, Paint,
  Watercolor, Airbrush, Blend & smudge, Eraser, Texture & FX, Misc. One
  icon per medium above the search field; the list shows that medium's
  brushes; search looks in all of them.
- 58 brushes instead of 198: near duplicates of the MyPaint collection
  left out (three "2B pencil", acrylic / oil / water variants...), clean
  names ("Soft dip pen", not "soft-dip-pen"), the author on hover. The
  choice is `assets/brushes/catalog.txt` (one line per brush, the other
  files stay in the repo). Imported brushes get a medium from their name.
- Mouse button bounce filtered everywhere (sliders, docks, strokes,
  shapes, arcs): a left release is held 50 ms; a press coming back in
  that time means the switch chattered, and the drag goes on (logged:
  "mouse button bounce ignored").

**Changed / fixed**
- Brush previews: erasers and wet brushes cross a first stroke (warm and
  cool segments), blenders scrub along it, instead of the red / green /
  blue bands. The S no longer moves when the size changes (only the
  stroke's width does). Previews are painted on a thread of their own:
  sliders stay smooth with textured brushes.
- Interface text can't be selected or copied any more (text fields still
  can).
- No "AuraStudio" in the menu bar.
- Windows: Alt (or F10) alone, or Alt + a letter, no longer puts the
  window in menu mode (the next key went nowhere). Alt+Space and Alt+F4
  still work.

## 0.4.0 — 2026-10-18

**New**
- Brush previews. The Brushes docker is a list: each brush as an S stroke
  painted by the brush itself (its engine, pressure tapering in and out),
  its name in the bottom right corner, instead of the MyPaint icons.
  Tool options shows the selected brush's stroke in place of its name and
  engine, repainted as its settings change: smaller size, thinner stroke;
  hardness 0, soft stroke; opacity, blend mode, Eraser tool.
- Size in the previews: true size for small brushes, compressed beyond
  (up to 1000 px), so every change shows. Brushes whose dynamics spill
  out of the strip are drawn smaller.
- Smudge, eraser, colorize and posterize brushes are previewed over color
  bands (on empty paper they show nothing).
- The list paints a few previews per frame (no freeze when opening a
  collection of 36 brushes).

## 0.3.4 — 2026-10-18

- Arc tool: a mouse button bouncing (a worn switch opens and closes again
  within a few ms) ended the drag and painted a straight line from the
  start to that point. A release followed by a press within 60 ms is now
  taken as the button held all along (logged: "mouse button bounce
  ignored"). Same on the validating click (it could end the chain).
- The Windows .exe carries its details (product name, description,
  version, author), shown in Properties > Details: an unsigned .exe
  without them is more often flagged by antivirus heuristics.
- The arc tool's logic moved to `arc_tool.rs`, with tests replaying mouse
  sequences (drags, clicks, bounces, random sequences).

## 0.3.3 — 2026-10-18

- The executable built by the CI has the version in its name
  (`aurastudio-0.3.3.exe`): several versions can run side by side.
- One autosave file per version (they no longer overwrite each other).
- README: the single tile manager is the next priority.

## 0.3.2 — 2026-10-18

Faster painting, nothing new to learn:
- While painting, the layers under and over the painted one are merged
  once per stroke and kept (like Krita): on a 30-layer document the
  display update of a big brush went from ~100 ms to ~3 ms.
- Soft round brushes about 1.8× faster (vectorized dab edge and pixel
  writes).
- MyPaint pigment brushes (watery, acrylic, oil) about 30 % faster:
  hokusai's spectral mix computes the brush color's part once per dab,
  same result to the bit (vendored, see NOTICE).
- Layer merging about 2× faster.
- New benchmark: `bench_myb` (MyPaint brush cost per pen sample).

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

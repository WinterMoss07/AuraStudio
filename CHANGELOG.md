# Changelog

Versions follow [Semantic Versioning](https://semver.org): MAJOR.MINOR.PATCH
(see README, Versioning).

## 0.10.3 — 2026-10-20

**Interface**
- Dock tabs: the plain arrow over them; the closed hand only while one
  is dragged (as brushes and layers).
- Layers docker: the arrow next to New layer lists layer types only
  (paint layer, filter layers); New folder has its own button beside it;
  group / ungroup sits right of draft, away from the folder button.
- Tooltips: the name, and the shortcut in parentheses when there is one
  ("Clip to layer below (Ctrl+Alt+G)", "Reference layer"); the long
  explanations are gone, from tooltips and the dialogs' notes too.

**Brush engine (Forge / hokusai)**
- A stroke step writes back only the pixels its dabs reached, not their
  whole 64 × 64 tiles: small brushes paint up to twice as fast (charcoal
  at 2 px: 0.19 → 0.09 ms per pen sample, pencil 0.105 → 0.077).
- Fixed: those whole tiles went through fix15 and back to 8 bits, so
  nearly transparent pixels next to a stroke, untouched by it, shifted by
  one level (the reference renders of 40 brushes changed by that alone).
- The dab mask buffer is kept from dab to dab (no allocation per dab);
  the speed curves' constants are worked out once per pen event; Fixed
  and Auto spacing skip libmypaint's dab count.
- Removed: hokusai's .myb writer and what only it used (unknown
  settings and inputs kept for a round trip, brush metadata), seven
  write-only state fields (one cost a color conversion per dab), unused
  sRGB, fix15 and tile helpers, the debug traces.

**Clean-up**
- Dead code removed elsewhere too (a filter flag, a layer query, three
  unused dependencies of the app). A stale doc fixed.
- Comments cut down: about 4350 lines → 3050, the brush engine's history
  notes gone, module docs to a few lines. The code is 26 600 lines (was
  28 400).

**Checked**
- Every bundled brush paints the same pixels as before the clean-up,
  except for the fix above (reference renders).
- Pure fuzz 300 000 operations, painter 30 000 actions: no failure.

## 0.10.2 — 2026-10-20

**Testing**
- `pure.rs`: a bug hunt without any interface, millions of random
  operations straight on the model and the real paint thread (layer
  tree, history, merges, fill, filters, selections, copy / paste, files,
  strokes with every brush), on small canvases of any shape. After every
  operation the document is checked (ids, active node, sizes, one
  reference per folder, history budget); results that must agree are
  compared: undo n + redo n gives the same document to the pixel, a part
  of the merge equals that part of the whole merge, the screen after a
  whole merge equals the merge, a file saved and opened is the same
  document. 3 000 000 operations (two seeds): no failure after the fix
  below.
- `painter.rs`: PAINT_OPS and PAINT_SIZE (a smaller page, brushes and
  gestures scaled to it); the 0.10 layer work (clipping, marks, labels,
  groups, filter layers, recorded for undo), the pure fuzz's checks after
  every action and an undo n + redo n check every 200. 1 000 000 actions
  (four seeds, A4 at 1/8 scale): no failure.

**Fixed**
- Moving a layer one step up or down into or out of a folder (the arrows
  of 0.10.0) could leave two reference layers in a folder. Found by the
  pure fuzz.

## 0.10.1 — 2026-10-19

**Undo for the layers**
- Everything the Layers docker and the Layer menu do is undoable, one
  step each: new layer / folder / filter layer, delete (the layer comes
  back with its pixels), duplicate, move, group / ungroup, clip,
  reference, draft, color label, lock, opacity, blend mode, name, a filter
  layer's values. A slider dragged or a name typed is one step once
  released. Hiding a layer or folding a folder is not a step (Photoshop,
  CSP). Undoing a paste takes its layer away.
- The Edit menu says what it undoes ("Undo Clip to layer below").
- Moved, never copied: a layer step keeps the tree's settings and order
  only, the pixels of the layers it deleted or made go along with it.

**Layers docker**
- One row of actions (CSP): new layer, and under its arrow the other
  kinds (paint layer, folder, filter layers by group); duplicate; one
  folder button (groups the selection, ungroups a folder selected alone);
  clip, reference, draft; the label filter and delete on the right. Move
  up / down are gone (drag and drop). The color label is a vertical list
  beside the blend mode and opacity.
- Clipping shows as a sky blue chain across the rows it links, in the
  eyes' column; the chain icon in the toolbar.
- Several layers: Ctrl+click adds or removes one, Shift+click takes a
  range. Group, drag, duplicate, delete, label, clip and draft work on
  all of them.
- Dragging a layer to the top or bottom of the list scrolls it, faster
  the farther out; the brush list does the same.

**Reference layers**
- One per folder: marking one unmarks the other there. A layer looks at
  its folder's reference, else the one of the folder around it, up to
  the top level (the fill's "References", "Stay inside the lines").
- A duplicated layer is neither a reference nor a draft (its lock, label
  and clipping are copied).

**Fixed**
- Merging the whole canvas again (a visibility, a clip, an opacity)
  could hold the interface ~200 ms on a big page ("UI waited" in the
  log): the merge goes by short bands and hands the document over
  between two; the interface waits up to 15 ms for it.

## 0.10.0 — 2026-10-19

**Clipping masks** (CSP, Photoshop)
- A layer, folder or filter layer clipped to the one under it shows only
  where that base has pixels; several clipped in a row share the base. As
  Photoshop's "blend clipped layers as group": each clipped layer blends
  over the base with its own mode and opacity, then the group is blended
  down with the base's: lowering the base's opacity fades the group. A
  hidden base hides them; one at the bottom of its folder has no base.
- Docker: the clip icon (and Ctrl+Alt+G, the Layer menu, the row's right
  click); clipped rows are indented with a hooked arrow down to the base.

**Filter layers** (Krita)
- Any of the 16 filters over what is under it, kept editable: New filter
  layer (the docker's sliders icon or the Layer menu) opens the filter's
  dialog (Cancel removes the new layer); a double-click on the layer, or
  the button where the blend mode was, opens it again. In an isolated
  folder it filters that folder's content; clipped, only its base.
- The dialog shows what is under the layer before / after; the canvas
  shows the result live.

**Reference and draft layers** (CSP)
- Reference (lighthouse): the fill tool's new "Refer to: This layer / All
  layers / References"; the brushes' new **Stay inside the lines** (CSP's
  anti-overflow): the stroke stays in the area of the reference lines
  where it starts (free when it starts on a line; with no reference
  layer, the other layers' lines count). Color on its own layer without
  crossing the line art.
- Draft (pencil, a blue line left of the thumbnail): shown, but left out
  of fills (the fill's "Ignore draft layers", on by default), of "Stay
  inside the lines" and of exported PNG / JPG.
- A reference or draft folder marks what it holds.

**Layers docker**
- Color labels (Krita's eight colors): a strip and a tint on the row; the
  funnel shows only some labels (the active layer always shows).
- A row of marks for the active layer: clip, reference, draft, label,
  group, ungroup. Right click on a row: rename, duplicate, delete, label,
  marks, group / ungroup, edit filter.
- Folders stand out (Layer Kit's idea): their own background and border,
  their layer count. Group (Ctrl+G) puts the active layer in a new
  folder, Ungroup (Ctrl+Shift+G) takes a folder's content out.
- New Lucide icons: lighthouse, pencil line, corner arrow, sliders, tag,
  funnel, folder in / out.

**Under the hood**
- A general merge (`compose.rs`) for clipping, filter layers and merges
  of some layers only (references, exports without drafts), by bands with
  the margin the filter layers read; the plain merge stays for documents
  without them. Tested to give the same pixels as the plain merge, and
  the same pixels for a part as for the whole with a blur layer.
- Merging the whole canvas again (visibility, opacity, order, a filter
  layer's values) runs on the paint thread, by bands: the interface no
  longer waits for it.
- Files: format 3 (labels, marks, filter layers); older files open as
  before.

**Fixed**
- Preferences: the window grew taller at every frame until it filled the
  screen (a vertical separator in a row takes all the height it's
  offered). It has a fixed height now, with a test.

## 0.9.1 — 2026-10-19

**Fixed**
1. Apply or Cancel in a filter dialog closed the app: textures egui let go
   of were destroyed before the frame still drawing them went to the GPU.
   They are now freed once a newer frame has been sent (this also covers
   the frames redrawn mid-stroke).
2. The preview's "After" label flickered ("After" / "After …") while a
   value moved: it stays put; three dots show beside it only when a result
   keeps you waiting more than 0.3 s.
3. The dialog's preview froze while a slider moved fast and caught up only
   when it stopped (every result came back already outdated and was
   thrown away): each one is shown as it comes. Zoomed out, the preview
   filters a thumbnail sized to the screen (a copy reduced 2, 4, 8…
   times) instead of a 900 px copy.
4. The hand stayed open while it held something: the filter preview's
   hand closes while dragging (even past the view's edge), and so does
   the cursor while dragging a layer, a brush or a medium.
5. Graphs that told a painter nothing are gone: the edge and line curves
   (Gaussian blur, Unsharp mask, Thicken / Thin lines, Find edges), the
   histogram and curve of Invert and Posterize, Posterize's tone strip,
   Sepia's gray ramp, Black & White's hue strip. Brightness / Contrast
   keeps its histogram (before / after, no curve); Levels, Threshold, Hue
   / Saturation keep theirs (they have handles); the colors before /
   after and the Emboss / Drop shadow samples stay.
6. A layer painted then erased counted as painted (its transparent tiles
   stay in memory): the Filter menu now looks at what shows, and inside
   the selection when there is one.

**Tests**
- The fuzz test opens filter dialogs (random clicks seldom found one) and
  takes a time limit: `FUZZ_SECS=60`.

## 0.9.0 — 2026-10-19

**Filter dialogs that show what they do**
- A preview in each dialog: the layer before and after, side by side on
  either part of a line you drag; Fit, 100 %, zoom (wheel, buttons, a
  double-click) and pan. Zoomed out, a reduced copy is filtered; from
  100 % the layer's own pixels. It runs on a thread of its own: dragging
  a slider never waits for it.
- Pictures for every filter, made by the filter itself:
  - Brightness / Contrast, Invert, Posterize: the histogram before and
    after on one scale, the tone curve, how much is clipped to black or
    white;
  - Levels (as PhotoCraft's): RGB / Red / Green / Blue, the channel's
    histogram, black / midtones / white handles over a gradient, output
    handles, typed values, Auto (0.1 % clipping), the levels cut off
    shaded, which other channels are changed;
  - Threshold: the luminosity histogram split where you drag, the share
    of pixels turning black;
  - Hue / Saturation: Master or one of six color ranges, colored tracks,
    the spectrum before and after, the range's four handles dragged on
    it; Colorize;
  - Black & White: six weights on colored tracks, the gray each hue
    becomes, tint with a color picker;
  - Hue / Saturation, Black & White, Invert, Posterize, Sepia: the
    painting's main colors before and after; Sepia: a gray ramp before
    and after;
  - Gaussian blur, Unsharp mask, Thicken / Thin lines, Find edges: a hard
    edge or a line before and after, in pixels ("a 6 px line becomes
    8 px", halo size);
  - Emboss and Drop shadow: an angle dial for the light, a lit or
    shadowed sample.
- Sliders with a colored track (a spectrum for hue, gray to color for
  saturation, a checkerboard for opacity), the default marked; Shift
  drags finer, a double-click puts the default back; values typed in their
  field. Big ranges (a blur of 0.1 to 250 px) give small values room.
- Information under the controls: clipping, the area and painted pixels
  changed, the shadow's offset...
- The Filter menu says why it is unavailable (layer locked or a folder,
  nothing painted).
- Filters (from 0.8.0's rewrite): Photoshop's Brightness / Contrast
  (modern and legacy), Levels per channel, Hue / Saturation with color
  ranges, Black & White, Posterize, Threshold, Unsharp mask, Minimum /
  Maximum as Thicken / Thin lines (square or round), Find edges, Emboss,
  Drop shadow (angle, distance, spread, size), adapted from PhotoCraft
  (MIT, see NOTICE); the W3C spec's blur, sepia, invert and opacity.

**Fixed**
- A filter reaching farther than 512 px (a 250 px blur, a long shadow)
  was cut off at that margin: the session grows to fit it.
- A filter on an empty layer said "Layer is locked or is a folder".
- Filter sliders rounded every value to a whole number (a 0.5 px blur
  couldn't be set).
- PhotoCraft's own bugs not carried over: its Threshold dialog drew the
  RGB histogram while splitting on luminosity; its histogram skipped the
  first half level and sloped the last one down.
- A file given on the command line (Open with…) opens at start.

## 0.8.0 — 2026-10-19

**Filters**
- A Filter menu, from the W3C Filter Effects Module (the spec of CSS and
  SVG filters, as the blend modes follow Compositing and Blending):
  - Adjust: brightness, contrast, levels, opacity;
  - Color: saturate, hue rotate, grayscale, sepia (the spec's matrices),
    invert, posterize, threshold;
  - Blur & sharpen: Gaussian blur (feGaussianBlur: three box blurs),
    sharpen;
  - Lines & edges: emboss, edge detect (feConvolveMatrix), thicken and
    thin lines (feMorphology dilate / erode);
  - Effects: drop shadow (the brush color).
- A floating dialog with the filter's values; the active layer shows the
  result live (Preview), inside the selection if there is one. Apply is
  one undo step; Cancel or Escape puts the layer back. Painting, undo and
  the like wait until the filter is applied or canceled.
- New crate `aurastudio-filters` (no dependencies): straight colors for
  the color ops, premultiplied 16-bit planes for blurs and convolutions,
  one thread per channel. A4 300 dpi on 2 cores: color ops ~40 ms, blur
  ~0.5 s.

**Preferences**
- Edit > Preferences… (Ctrl+K), by section: Interface (size, brush
  strokes in the Tools docker, open maximized, reset docks), Canvas &
  display (HiDPI scaling, color management, intent), Brushes & input
  (brush outline or a crosshair, click debounce), Files (autosave delay,
  or never). Changes apply at once; "Restore defaults". The View menu
  items they replace are gone. 0.7 settings files are still read.

**New document dialog**
- Redone after PhotoCraft's layout: Painting, Animation and Comics tabs,
  preset cards with the page's shape and size, the details on the right
  (name, width / height in px, mm, cm or in, resolution, orientation,
  what the preset is for, pixel size and memory). Double-click a card or
  Enter creates. The tab last used is remembered.
- Presets: paper sizes, squares, wallpapers, Instagram; animation HD
  720p, Full HD, 4K and 8K UHD, DCI 2K and 4K (full container, flat
  1.85, scope 2.39), 1.66, 4:3, vertical 9:16, square; comics: manga B4
  and A4 manuscripts, B5, A5, B6 tankōbon (600 dpi), US comic with bleed,
  US art board 11 × 17, graphic novel 6 × 9, BD album, A4, webtoon (1×,
  2×), Instagram comic.

**Fixed**
- The hand, rotate and zoom cursors were tiny on high-density screens
  (3K at 200 %): they are drawn at the screen's scaling, and redrawn when
  the window moves to another monitor.

**Parked**
- Stroke previews: to redo from a clean base (README, "Parked"); the
  geometry tools' previews don't show since 0.7.3.

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

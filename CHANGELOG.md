# Changelog

Versions follow [Semantic Versioning](https://semver.org): MAJOR.MINOR.PATCH
(see README, Versioning).

## 0.14.4 — 2026-10-26

**Fixes**
- A new animation folder is made inside a group only while the Animation panel
  has that group open; out of it (or from the Layers docker) it is made outside.
- The mouse wheel over the folders' headers (timeline and X-sheet) goes through
  the folders; over the cells it still goes through time. Shift swaps them.

**Look**
- Locks are the clipping chain's sky blue (no more orange chip); the closed lock,
  the clipping chain and the Clip button have a soft inner glow.

**Tools**
- Memory report: the app's own heap is counted (allocator), so the gap is split
  between "Rust heap not itemised" and "outside the Rust heap" (driver, system
  libraries, stacks); the start is measured step by step (brushes, window,
  graphics device, first frame).

## 0.14.3 — 2026-10-26

**Animation: names and structure**
- One vocabulary everywhere: Group → Folder → Frame → Cel ("sub-cel" is gone).
- Groups gather animation folders (organisation only, no nesting): one column
  in the timeline / X-sheet, a double click opens it (its folders alone, "Back"
  to leave). Right click: "Group animation folders" (Animation), "Group layers
  into folder" (Illustration). Folders are picked in the Animation panel and the
  Layers docker (Ctrl / Shift), dragged to reorder, into a group, out of it (on
  "Back"). The group / ungroup button of the Layers docker is gone.
- New Animation documents start with an animation folder (default template:
  Colo, Ink, Douga, Genga, top to bottom) above the locked Paper.
- Every folder is made collapsed (layers, animation folders, groups, new frames).
- Folders show "x frames - x cels" once; the cel count per frame is gone.

**Cel template docker**
- Drag and drop replaces the up / down arrows; the order is the Layers docker's.
- A cel can be marked Reference (the Ink by default), for every frame.
- Deleting a cel asks first when drawings would go: "Delete cel Ink: 14 frames,
  11 contain a drawing", Cancel by default (Enter / Escape).
- Window menu: the Cel template sits with the Animation panel.

**Animation panel**
- Frames are picked in one folder (click, Shift range, Ctrl add, a box on empty
  cells) and moved together; Ctrl + drag copies; one undo step.
- The Timeline / X-sheet switch is a labelled button with the circle-arrows icon.
- Right click on a frame no longer opens a one-letter-wide menu that flickers
  away (it read the pointer, now on the menu).
- Locked and unlocked are told apart everywhere (orange filled lock).

**Reference**
- Greyed out on animation folders and frames (set on the Ink cel, in the
  template). In Illustration a folder can be the reference: only its visible
  pixels count, one reference per level.

**Delete**
- Suppr over the Layers docker or the Animation panel deletes the frames /
  folders picked there; over the canvas it still clears the layer.
- A frame or folder with a locked cel could not be deleted (the cels' locks of
  "Apply to folder" counted); only its own locks count now.

**Tools**
- Window > Memory report: every megabyte named (tiles by layer, history,
  selection, buffers, GPU textures, the driver's allocator) next to the system's
  figures (working set, private bytes) and what no line names. AURASTUDIO_MEMLOG=1
  prints it to the console.

## 0.14.2 — 2026-10-20

**Look**
- Tighter UI: sliders 18 px high (were 24), stroke preview 38 px (was 56),
  layer rows 34 px with 28 px thumbnails (were 44 / 36), icon buttons
  22×20 with 14 px icons (were 26×24 / 16), toolbox 34 px wide (was 40).
- Smaller text and spacing: body 12.5 px, item spacing 5×4, controls 19 px
  high, dock tabs 25 px, dock margins 6 px; color docker and Cel template
  rows shorter.

**Fixes**
- Copying a reference frame made a second reference in the animation
  folder (found by the fuzz); the copy is no reference, as with Duplicate.

## 0.14.1 — 2026-10-20

**Look**
- Sharper: corners 2.5 px on widgets, rows, tabs and buttons (were 5–6),
  3–4 px on menus, dialogs and the canvas (were 8–10).
- The docks' gray is lighter (#282829, was #1f1f1f); widgets, hover and
  borders one step lighter to keep their contrast; the areas set into the
  docks (#181818) stay as they were.

## 0.14.0 — 2026-10-20

Step 3 of the animation work: the X-sheet / timeline and playback
(after reading OpenToonz's X-sheet: one grid laid out on a frame axis and
a layer axis, turned by an orientation).

**Animation panel** (Animation documents; Window > Animation)
- **Timeline** under the canvas, or **X-sheet** beside the dockers: one
  button turns it, the same grid either way. Columns (X-sheet) left to
  right = bottom to top of the stack, as Toonz; rows (timeline) top to
  bottom = top to bottom, as Krita and CSP.
- Per folder: name (double click: rename), eye, lock, its label color.
  Cells: a drawn frame filled (its number), a hold a line until the next
  one (OpenToonz's continue line), the chosen frame outlined, the
  current frame lit; shades every 6 frames, a line every second.
- Click a cell: go to that frame in that folder (drawing there makes the
  frame). Drag the frame numbers or an empty cell: scrub. Wheel: through
  time; Shift + wheel: across folders; the current frame kept in view.
- **Move** a drawn frame by dragging it, **copy** it with Ctrl. Onto a
  drawn frame: Replace it / Swap them (moves) / Cancel — Cancel is the
  default (Enter, Escape), as it is usually a slip. Onto an empty frame:
  no question. Frames are renamed after their new place.
- **Insert / Remove frame** (bar buttons at the current frame, or a right
  click on a cell): the following frames move one later / earlier, and
  are renamed (1 to 10 drawn, insert at 2: the old 2–10 become 3–11, the
  frame 1 holds on frame 2). Delete a drawing: the one before holds on.
  All undoable.
- **Playback bar**: new animation folder (camera grayed until it comes),
  first / previous / play-pause / next / last frame, loop, fps (24 by
  default), current / last frame. Playback starts at the current frame
  (from the first one at the end), keeps real time (frames are skipped if
  the merge can't keep up), loops over the frames drawn, stops when
  drawing.
- Not yet: clones (linked frames), the onion skin, the camera column, a
  playback cache.

**Checked**
- Tests: moving (onto an empty frame, swap, replace), copying (a true
  copy), insert / remove with renaming, deleting.
- Pure fuzz with moves, copies, inserts, removes and deletes of frames;
  UI fuzz on Animation documents with the panel shown.

## 0.13.0 — 2026-10-20

Step 2 of the animation work: animation folders, frames, cel template.

**Animation folders** (Animation documents)
- New special folders: an arrow beside the New folder button (and Layer >
  New special folder): Animation folder; Camera folder and Comics folder
  listed, grayed out until they come.
- One animation folder per element. Drawing (brush or fill) on a frame
  with nothing drawn makes **Frame N** at the current frame, holding the
  folder's **cel template**: Genga, Douga, Ink, Colo by default. The frame
  and its first stroke are **one undo step** (no empty frame left behind);
  a stroke that paints nothing leaves no frame.
- **Hold**: a frame shows until the next drawn one; drawing over a hold
  makes a new frame there. `,` / `.`: previous / next frame (shown in the
  menu bar).
- Layers docker: the folder (film icon, its number of frames) and one row
  per frame, its number on a card ("shown" for the one on screen); a
  click on a frame goes to it. Sub-cels are never rows. Frames have no
  eye (the current frame decides) and keep their name (their place in
  time).
- Nothing else goes into an animation folder (new layers land above it);
  frames and sub-cels can't be dragged, grouped, duplicated or deleted one
  by one from the list (a frame can be deleted). Frame moves, copies and
  clones come with the X-sheet.

**Cel template docker** (shown with the first animation folder; Window
menu)
- The sub-cels top to bottom: the radio picks where strokes go, in every
  frame (a douga stays a douga from frame to frame); eye and lock are the
  frame's; Apply to folder gives every frame, and the frames to come,
  those eyes and locks. A locked sub-cel can't be painted, and no frame is
  made to paint a locked one.
- Template: add, remove (the pixels kept for undo), move up / down,
  rename (double click), in every frame at once; undoable.
- The colo sub-cel is marked "colo" (palette colors come later; it is a
  plain layer for now).

**Files**
- Format 4 when a document has animation folders (template, chosen
  sub-cel, frames, current frame); other documents are still written as
  format 3, readable by older versions.

**Checked**
- Tests: frames follow the template and hold; frames stay in their
  folder; template edits reach every frame; tree undo keeps frames whole;
  a new frame and its stroke are one step; a frame for nothing goes;
  animation folders survive a save.
- Pure fuzz with animation folders, frames, time, sub-cel choice and
  template edits (invariants checked after every operation); UI fuzz on
  Animation documents with the Cel template docker shown.

## 0.12.0 — 2026-10-20

First step of the animation work (spec agreed with Walid: one animation
folder per element, one layer per frame with its sub-cels, a cel
template per folder, X-sheet, camera in pixels).

**Document types**
- Illustration, Animation, Comics: the New document tab is the type
  ("Painting" renamed "Illustration"). Kept in the file (files from
  before it open as Illustration; a type a newer version made is
  refused rather than half shown) and shown in the menu bar.
- Layer menu and the Layers docker's layer types: New animation folder
  and New camera folder, grayed out outside an Animation document (they
  come with the X-sheet).

**Interface**
- **Window** menu: every docker with a check mark to hide or show it;
  shown again beside its usual partner (Color / Tools, Layers / Canvas),
  else in a new dock at the bottom. Reset docks moved there from View.
- Canvas > Canvas size… shows the Canvas docker again if it was hidden.

**Checked**
- Tests: the type survives a save (all three), old files, unknown type;
  dockers hidden and shown again. Pure fuzz with documents of every type
  (file = document compares the type).

## 0.11.1 — 2026-10-20

**Picker and selection**
- **Loupe** option (picker and selection tools): a magnified view around
  a 1-pixel cross. The picker takes the pixel in the middle (the rim
  shows its color); selection points land on pixel corners.
- Own cursors: an eyedropper for the picker (tip on the pixel taken), a
  thin cross for the selection tools with the mode held: + (Shift),
  − (Alt), × (Shift+Alt); the loupe shows the same sign.

**Transform**
- While a handle is dragged, the GPU draws the transform at once: the
  canvas under the moved layers, what moves, and the layers over it
  (exact with normal blending over it). On release the paint thread's
  exact result replaces it. The image and the warp grid stay under the
  pen.
- Warp: a point moves only the four cells around it; the points around
  and what is past them stay pinned (was a Catmull-Rom spline reaching
  two cells away).
- On the transform tool with no transform under way, Enter (or Ctrl+T)
  starts a new one (no need to change tools after a cancel).

**Checked**
- Pure fuzz 100 000 operations (786 cancels and 607 undos of transforms
  checked), UI fuzz 150 s (49 500 steps), painter 30 000 actions — no
  failure. Test that every layer setting survives a save.

## 0.11.0 — 2026-10-20

**Selections**
- Selections are 8-bit now (soft edges): painting, fill, filters, copy,
  Delete and the transform follow a soft edge partly, a hard one fully.
- New tools in the Selection group (M, again for the next one):
  ellipse, polygon (click the points; Enter, a double click or a click
  on the first point closes it, Backspace takes one back, Escape drops
  it) and the **magic wand**: tolerance, contiguous or every like pixel,
  this layer / all layers / the references (drafts left out or not), grow.
- Modes: new, add (Shift), subtract (Alt), intersect (Shift+Alt), held at
  the press or chosen in the options; anti-aliasing (ellipse, lasso,
  polygon, wand) and feather.
- A drag inside the selection moves its outline (rectangle, ellipse,
  lasso); a click on it deselects.
- **Select** menu: All (Ctrl+A), Deselect (Ctrl+D), Reselect
  (Ctrl+Shift+D), Invert (Ctrl+Shift+I), Grow…, Shrink… (round, exact
  distances), Feather… (a Gaussian, as Photoshop's), Border…, From layer
  (its opaque pixels).
- Every selection change is an undo step (Krita, CSP).
- The marching ants follow any shape (the wand's too), traced at the
  zoom's detail.

**Transform** (T, Ctrl+T, Edit > Transform)
- On the active layer, the layers chosen in the Layers docker, a folder's
  layers, or only what is selected (the rest stays, the selection moves
  along). Applied when leaving the tool or choosing other layers.
- Free: move (Shift: 8 directions), corners scale keeping the proportions
  (Shift: free, Alt: from the pivot; "Keep proportions" in the options),
  edges stretch one way (Shift: both), outside the box rotates (Shift:
  15° steps), a drag along an edge skews (Alt: both edges), Ctrl+corner
  distorts (stops before the box folds over), the pivot can be dragged.
  X / Y, width, height, angle and skew as numbers; flip horizontally /
  vertically, a quarter turn left / right, Reset.
- Perspective: each corner on its own, an edge moves its two corners.
- Warp: a smooth grid (1 × 1 to 8 × 8) bent by its points, on top of the
  free transform.
- Resampling: Bicubic (Photoshop's), Bilinear or Nearest (pixel art),
  premultiplied (no dark fringes), from a pyramid for big reductions (no
  aliasing), anti-aliased edges. While dragging, a fast preview; on
  release, the full quality; on all cores (2 cores: 34 ms for a 1610 ×
  1247 picture turned and scaled).
- Enter applies (one undo step), Escape cancels, Ctrl+Z / Ctrl+Y step
  through the transform's own changes (then the document's), arrows nudge
  (Shift: 10 px). Saving, exporting or closing applies it first.

**Renderer**
- Vulkan missing or failing: DirectX 12 (Windows), then OpenGL take over
  (OpenGL is now built in on Windows too). A message only if none runs.
- Preferences > Canvas & display: Vulkan, DirectX 12 or OpenGL; the
  one in use is shown, "A restart is required" when another is chosen.

**Crash rescue**
- A crash (a panic on any thread) saves the unsaved work to the recovery
  file — offered at the next start — and says so before closing.

**Interface**
- Layers docker: too narrow for its row of buttons, it uses two (new
  layer, folder, duplicate and delete above; clip, reference, draft,
  group and the label filter below): nothing overlaps any more.
- Sliders too narrow for their name and value show the value (the name on
  hover).

**Checked**
- New tests: shapes, modes, grow / shrink / feather / border, outlines,
  soft blending, homographies, resampling (moves exact, halving averages,
  turns keep alpha), the warp grid, transform sessions (cancel, apply,
  undo, the selection along), the box's gestures and numbers.
- `pure.rs`: selection commands and transforms (cancel = as before, undo
  = as before, through the paint thread too), the selection compared by
  undo n + redo n; `painter.rs`: selections and transforms as a painter
  makes them; `fuzz.rs`: the Select menu, selection commands and
  transforms.

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

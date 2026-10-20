# Third-party code

## crates/aurastudio-input/src/lib.rs — Windows Ink / WinTab input handling

Adapted from `efude-input` (https://github.com/852wa/Efude), © 2026 Hakoniwa.
Licensed under MIT OR Apache-2.0.

This file is a trimmed, vendored copy: only the Windows Ink (WM_POINTER) and
WinTab (wintab32.dll) hooks were kept. Parts depending on `efude-core`
(stroke replay/logging, clipboard helpers) were not vendored as they aren't
used by AuraStudio. We vendor instead of depending on the crate so this
input layer can be modified and optimized directly as part of our own
codebase, per the project's "inspired rewrite" approach.

Full original source: https://docs.rs/efude-input/0.1.0/src/efude_input/lib.rs.html

## assets/brushes/mypaint — MyPaint brush collection

The brushes (`.myb` files and their `_prev.png` previews) come from
https://github.com/mypaint/mypaint-brushes and are released under
**CC0 1.0** (public domain dedication), see `assets/brushes/mypaint/LICENSES.dep5`.
Authors: Martin Renold and the MyPaint Development Team (classic,
experimental), David Revoy (deevad), Ramón Miranda (ramon), Marcelo "Tanda"
Cerviño (tanda), Guillaume Loussarévian (kaerhon_v1), Brien Dieterle
(Dieterle). Thank you! AuraStudio ships 56 of them, chosen and renamed in
`assets/brushes/catalog.txt`; the others stay in the folder.

## hokusai — the core of Forge

Forge, AuraStudio's brush engine (`crates/aurastudio-brush/src/forge`),
is built on the code of `hokusai-core` 0.3.0 and `hokusai-brush` 0.3.0
(https://github.com/reearth/hokusai, © Re:Earth and contributors, MIT OR
Apache-2.0), a Rust port of libmypaint (https://github.com/mypaint/libmypaint,
ISC): brush settings, inputs and curves, the stroke path, libmypaint's dab
blending and the `.myb` reader. Changed since: Krita-style curve modes,
wash painting, auto spacing, a dab on press, path clipping to the canvas,
speedups that keep MyPaint brushes' output identical (the spectral mix
takes the brush color's logarithms once per dab, `wgm_mix`; the speed
inputs' constants once per event; the dab mask buffer kept), only the
pixels the dabs reach written back, and hokusai's .myb writer, debug
traces and unused state removed.

The MIT license of hokusai:

> Permission is hereby granted, free of charge, to any person obtaining a
> copy of this software and associated documentation files (the
> "Software"), to deal in the Software without restriction, including
> without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to
> the following conditions:
>
> The above copyright notice and this permission notice shall be included
> in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
> OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
> MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
> IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
> CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
> TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
> SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## vendor/egui — egui 0.30.0, patched

Copy of `egui` 0.30.0 (https://github.com/emilk/egui), © Emil Ernerfeldt
and the egui contributors, licensed under MIT OR Apache-2.0. Two changes:
`Areas::compare_order` takes the upstream fix from egui 0.31 (the 0.30
version is not a total order and made the standard sort panic, found by our
fuzz test), and float literals the compiler would infer as `f32` are written
`_f32` (a warning that becomes an error in future Rust). Used through
`[patch.crates-io]` in the workspace `Cargo.toml`; to drop once egui is
upgraded.

## vendor/egui_dock — egui_dock 0.15.0, patched

Copy of `egui_dock` 0.15.0 (https://github.com/Adanos020/egui_dock), ©
Adam Gąsior and the egui_dock contributors, licensed under MIT (its
LICENSE file is kept). Changes, marked "AuraStudio patch": a dock being
dragged shows a bar on the edge where it would be inserted, and the tab
bar of the dock it would join lights up (`drag_and_drop.rs`); float
literals written `_f32`, as in egui. Used through `[patch.crates-io]`.

## Lucide icons

The interface icons (`crates/aurastudio-ui/icons/*.svg`) come from
[Lucide](https://lucide.dev), ISC license (`crates/aurastudio-ui/icons/LICENSE-lucide`).
Recolored to white and drawn with a thinner stroke. `charcoal.svg` is ours,
drawn in the same style (Lucide has no charcoal stick); `flip-horizontal.svg`
and `flip-vertical.svg` are Lucide's former `flip-horizontal-2` and
`flip-vertical-2`.

## Ideas from other programs (no code copied)

- Clipping masks, reference and draft layers, "Stay inside the lines"
  (anti-overflow): Clip Studio Paint's behavior, from its manual.
- Filter layers, color labels and filtering by label: Krita's, from its
  manual.
- Darker folder rows: the Layer Kit plugin for Krita by tenzindraws
  (https://github.com/10zindraws/Layer-Kit, GPL-3.0); its code is not
  used.

## Krita — how its selection and transform tools work

Read, not copied (Krita is GPL): `plugins/tools/selectiontools`,
`plugins/tools/tool_transform2` (the free transform's matrix order:
center, scale, shear, rotation, translation; handles, shear along the
edges, perspective), `libs/image/kis_selection_filters.cpp` (grow, shrink,
feather as GIMP's), `KisColorSelectionPolicies.h` (the wand's threshold),
`kis_selection_modifier_mapper.cc` (Shift add, Alt subtract, Shift+Alt
intersect). AuraStudio's code for them is its own or PhotoCraft's (below).

## PhotoCraft — filters and their dialogs

Parts of `crates/aurastudio-filters` and of the filter dialogs
(`crates/aurastudio-ui/src/filters/`) are adapted from PhotoCraft
(https://github.com/storytold/photocraft), Copyright (c) 2026 ArtCraft Team
and the PhotoCraft contributors, licensed under MIT OR Apache-2.0 (used
here under MIT):

- `tone.rs` (Brightness / Contrast, Levels, Posterize, Threshold), `color.rs`
  (Hue / Saturation and its color ranges, Black & White), the Unsharp Mask,
  Minimum / Maximum, Find Edges, Emboss and Drop Shadow of `spatial.rs`:
  from `crates/compose/src/adjust.rs`, `crates/compose/src/effects.rs`,
  `crates/doc/src/adjust.rs` and `crates/algo/src/`. Changes: 8-bit
  tables, our value layout, planar 16-bit premultiplied blurs, threads.
- The Levels, Threshold and Hue / Saturation editors, the histogram,
  gradient bar and handle drawing, the preview's view math: from
  `crates/ui-egui/src/adjust_editors.rs`, `tone.rs` and
  `filter_preview_ui.rs`. Changes: before / after histograms on one scale,
  a before / after split view filtered on a thread, our theme.
- Selections and the transform tool (0.11): the anti-aliased polygon fill
  (exact coverage across, 4 rows a pixel), grow / shrink by an exact
  distance transform, Border's soft band, the magic wand's anti-aliasing
  and the outline as merged horizontal / vertical runs (`selection.rs`),
  from `crates/algo/src/selection.rs` and `crates/ui-egui/src/outline.rs`;
  the quad homography, Photoshop's Keys bicubic (a = -0.75),
  premultiplied sampling and the edge coverage (`transform.rs`), from
  `crates/algo/src/transform.rs`; the drag gestures in the box's own
  frame, the pivot carried along and the convex stop of a distort
  (`crates/aurastudio-app/src/xform.rs`), from
  `crates/ui-egui/src/transform_tool.rs`. Changes: 8-bit tiled coverage,
  a pyramid instead of a resize for reductions, our own threads, a
  Catmull-Rom warp grid, Krita's and CSP's modifiers.

The ArtCraft name and logos are not used. PhotoCraft's MIT license:

> Permission is hereby granted, free of charge, to any person obtaining a
> copy of this software and associated documentation files (the
> "Software"), to deal in the Software without restriction, including
> without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to
> the following conditions:
>
> The above copyright notice and this permission notice shall be included
> in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
> OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
> MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
> IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
> CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
> TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
> SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

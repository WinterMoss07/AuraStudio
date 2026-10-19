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
and speedups that keep MyPaint brushes' output identical (the spectral
mix takes the brush color's logarithms once per dab, `wgm_mix`; the trace
checks run in debug builds only).

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

## Lucide icons

The interface icons (`crates/aurastudio-ui/icons/*.svg`) come from
[Lucide](https://lucide.dev), ISC license (`crates/aurastudio-ui/icons/LICENSE-lucide`).
Recolored to white and drawn with a thinner stroke. `charcoal.svg` is ours,
drawn in the same style (Lucide has no charcoal stick).

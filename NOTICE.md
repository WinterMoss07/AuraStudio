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

The 196 brushes (`.myb` files and their `_prev.png` previews) come from
https://github.com/mypaint/mypaint-brushes and are released under
**CC0 1.0** (public domain dedication), see `assets/brushes/mypaint/LICENSES.dep5`.
Authors: Martin Renold and the MyPaint Development Team (classic,
experimental), David Revoy (deevad), Ramón Miranda (ramon), Marcelo "Tanda"
Cerviño (tanda), Guillaume Loussarévian (kaerhon_v1), Brien Dieterle
(Dieterle). Thank you!

## hokusai — MyPaint brush engine

Brush dynamics and dab rendering for MyPaint brushes use the `hokusai`
crates (a Rust port of libmypaint), as a regular crates.io dependency.

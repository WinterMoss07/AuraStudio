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

//! The keyboard: shortcuts, and the keys held that change what the
//! pointer does (Space, Shift, Ctrl).

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

use aurastudio_ui::{EditCmd, FileCmd, Group, LayerCmd, SelectCmd, Tool, ViewCmd};

use super::App;
use crate::window::Screen;

impl App {
    pub(super) fn on_key(&mut self, key: KeyEvent) {
        // Space is tracked on release too (and even while a field
        // has focus, so it can't get stuck down).
        if key.logical_key == Key::Named(NamedKey::Shift) {
            self.shift_down = key.state == ElementState::Pressed;
            if self.space_down { self.update_cursor(); }
        }
        if key.logical_key == Key::Named(NamedKey::Control) {
            self.ctrl_down = key.state == ElementState::Pressed;
            if self.space_down { self.update_cursor(); }
        }
        if key.logical_key == Key::Named(NamedKey::Space) {
            let down = key.state == ElementState::Pressed;
            if down != self.space_down {
                self.space_down = down;
                self.update_cursor();
            }
        }
        // Typing in a docker field (e.g. canvas width) must not
        // erase the layer, undo or move the view.
        let ui_typing = self.gpu.as_ref().is_some_and(Screen::ui_wants_keyboard);
        if key.state != ElementState::Pressed || ui_typing { return; }
        let ctrl  = self.modifiers.control_key() || self.modifiers.super_key();
        let shift = self.modifiers.shift_key();
        // Alt + a key is a combination, never the key's own shortcut
        // (Alt+E must not pick the eraser).
        let alt   = self.modifiers.alt_key();
        if self.ui.has_dialog() { return; }
        // A tool group's key (looked up once).
        let group = match key.logical_key.as_ref() { Key::Character(c) if !ctrl && !alt && !key.repeat => Group::from_key(c), _ => None };
        match key.logical_key.as_ref() {
            Key::Character(c) if ctrl && !key.repeat && ["n", "o", "s", "e", "w"].iter().any(|k| c.eq_ignore_ascii_case(k)) => {
                let cmd = match c.to_ascii_lowercase().as_str() {
                    "n" => Some(FileCmd::New),
                    "w" => Some(FileCmd::Close),
                    "o" => Some(FileCmd::Open),
                    "s" if shift => Some(FileCmd::SaveAs),
                    "s" => Some(FileCmd::Save),
                    "e" if shift => Some(FileCmd::Export),
                    _ => None,
                };
                if let Some(cmd) = cmd { self.file_cmd(cmd); }
            }
            // The start screen has nothing to edit or move.
            _ if self.home => {}
            Key::Named(NamedKey::Delete) if !key.repeat && !alt => {
                // Over the Layers docker or the Animation panel: delete what is picked there.
                if !self.ui.delete_key() { self.clear_active_layer(); }
            }
            // Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y (repeat when held); building an arc
            // outline, Ctrl+Z takes back its last arc.
            Key::Character(c) if ctrl && !shift && c.eq_ignore_ascii_case("z") && self.arc.active() => self.undo_arc(),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
                if shift { self.redo() } else { self.undo() }
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => self.redo(),
            Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("k") => {
                self.ui.open_prefs();
                self.request_redraw();
            }
            // Layers: Ctrl+G group, Ctrl+Shift+G ungroup, Ctrl+Alt+G
            // clip to the layer below (Photoshop's keys).
            Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("g") => {
                let cmd = if alt { LayerCmd::Clip } else if shift { LayerCmd::Ungroup } else { LayerCmd::Group };
                self.ui.layer_shortcut(cmd);
                self.request_redraw();
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("a") => self.select(SelectCmd::All),
            Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("d") => self.select(SelectCmd::Reselect),
            Key::Character(c) if ctrl && shift && c.eq_ignore_ascii_case("i") => self.select(SelectCmd::Invert),
            Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("t") => {
                if self.tool == Tool::Transform { self.start_transform(); } else { self.set_tool(Tool::Transform); }
            }
            Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("c") => self.edit_cmd(EditCmd::Copy),
            Key::Character(c) if ctrl && !key.repeat && c.eq_ignore_ascii_case("v") => self.edit_cmd(EditCmd::Paste),
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("d") => self.select(SelectCmd::None),
            Key::Character(_) if group.is_some() => {
                if let Some(g) = group { self.select_group(g); }
            }
            // Arc outline: Enter paints it, Escape drops it.
            Key::Named(NamedKey::Enter) if self.arc.active() => self.finish_arcs(),
            Key::Named(NamedKey::Escape) if self.arc.active() => self.cancel_arcs(),
            // Transform: Enter keeps it, Escape drops it; arrows nudge.
            Key::Named(NamedKey::Enter) if self.xf.is_some() => self.end_transform(true),
            Key::Named(NamedKey::Escape) if self.xf.is_some() => self.end_transform(false),
            // On the transform tool with none under way: a new one.
            Key::Named(NamedKey::Enter) if self.tool == Tool::Transform && !key.repeat && !self.is_drawing => self.start_transform(),
            Key::Named(k @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) if self.xf.as_ref().is_some_and(|x| x.drag.is_none()) => {
                let d = if shift { 10.0 } else { 1.0 };
                let (dx, dy) = match k { NamedKey::ArrowLeft => (-d, 0.0), NamedKey::ArrowRight => (d, 0.0), NamedKey::ArrowUp => (0.0, -d), _ => (0.0, d) };
                if let Some(xf) = &mut self.xf { xf.nudge(dx, dy); }
                self.send_preview(false);
            }
            // Polygon selection: Enter closes it, Escape drops it,
            // Backspace takes back its last point.
            Key::Named(NamedKey::Enter) if self.poly.is_some() => self.finish_poly(),
            Key::Named(NamedKey::Escape) if self.poly.is_some() => { self.poly = None; self.request_redraw(); }
            Key::Named(NamedKey::Backspace) if self.poly.is_some() => {
                if let Some((pts, _)) = &mut self.poly { pts.pop(); if pts.is_empty() { self.poly = None; } }
                self.request_redraw();
            }
            Key::Named(NamedKey::Escape) if self.gesture.is_some() => { self.gesture = None; self.update_cursor(); self.request_redraw(); }
            // View keys (Krita): 1 = 100 %, 2 = fit, 4 / 6 = rotate
            // by 15 degrees, 5 = reset rotation, + / - = zoom.
            // Digits by the key's place (AZERTY gives "&", "é"... there),
            // the others by what they type.
            Key::Character(_) | Key::Named(_) if !ctrl && !alt && digit_view(&key).is_some() => {
                if let Some(cmd) = digit_view(&key) { self.view_cmd(cmd); }
            }
            Key::Character(c) if !ctrl && !alt => match c {
                "+" | "=" => self.view_cmd(ViewCmd::ZoomIn),
                "-" => self.view_cmd(ViewCmd::ZoomOut),
                // Previous / next frame (OpenToonz).
                "," => self.step_frame(-1),
                "." => self.step_frame(1),
                _ => {}
            },
            _ => {}
        }
    }
}

/// The view command of a digit key, by its place on the keyboard (top row
/// or keypad): 1 = 100 %, 2 = fit, 4 / 6 = rotate, 5 = reset rotation;
/// the keypad's + and - zoom.
fn digit_view(key: &KeyEvent) -> Option<ViewCmd> {
    let PhysicalKey::Code(code) = key.physical_key else { return None };
    Some(match code {
        KeyCode::Digit1 | KeyCode::Numpad1 => ViewCmd::ActualSize,
        KeyCode::Digit2 | KeyCode::Numpad2 => ViewCmd::Fit,
        KeyCode::Digit4 | KeyCode::Numpad4 => ViewCmd::RotateLeft,
        KeyCode::Digit5 | KeyCode::Numpad5 => ViewCmd::ResetRotation,
        KeyCode::Digit6 | KeyCode::Numpad6 => ViewCmd::RotateRight,
        KeyCode::NumpadAdd => ViewCmd::ZoomIn,
        KeyCode::NumpadSubtract => ViewCmd::ZoomOut,
        _ => return None,
    })
}

//! Files and the clipboard: new, open, save, export, the autosave and the
//! recovery after a crash, the recent files; copy and paste; undo.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aurastudio_core::DocType;
use aurastudio_document::{Document, Edit};
use aurastudio_file::{EXTENSION, IMAGE_EXTENSIONS};
use aurastudio_ui::{Dialog, DialogChoice, EditCmd, FileCmd};

use super::{ago, autosave_after, has_extension, recovery_path, save_recent, App, AppEvent, Pending, APP_TITLE, MAX_RECENT, PLACEHOLDER};
use crate::paint_thread::Cmd;
use crate::projection::Projection;

impl App {
    /// Copy / paste pixels, also with other apps through the system
    /// clipboard.
    pub(super) fn edit_cmd(&mut self, cmd: EditCmd) {
        if self.home || self.is_drawing { return; }
        match cmd {
            EditCmd::Copy => {
                let clip = self.doc.lock().unwrap_or_else(|e| e.into_inner()).copy();
                let Some(clip) = clip else {
                    self.ui.notify("Nothing to copy here (empty, or a folder is selected)");
                    self.request_redraw();
                    return;
                };
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let img = arboard::ImageData {
                        width: clip.rect.w() as usize, height: clip.rect.h() as usize,
                        bytes: std::borrow::Cow::Borrowed(clip.px.as_flattened()),
                    };
                    if let Err(e) = cb.set_image(img) { log::warn!("clipboard: {e}"); }
                }
                self.ui.notify("Copied");
                self.clip = Some(clip);
            }
            EditCmd::Paste => match self.paste_source() {
                Some(clip) => self.send_edit(Edit::Paste(clip)),
                None => self.ui.notify("Nothing to paste"),
            },
        }
        self.request_redraw();
    }

    /// What Paste pastes: the system clipboard's image (centered on the
    /// canvas, or in place if it is what we copied last), else our copy.
    pub(super) fn paste_source(&self) -> Option<aurastudio_document::Clip> {
        let image = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_image().ok());
        let Some(img) = image else { return self.clip.clone() };
        if let Some(c) = &self.clip {
            if (c.rect.w() as usize, c.rect.h() as usize) == (img.width, img.height) && c.px.as_flattened() == &img.bytes[..] {
                return Some(c.clone());
            }
        }
        let (w, h) = self.canvas_size();
        let (x, y) = ((w as i64 - img.width as i64) / 2, (h as i64 - img.height as i64) / 2);
        Some(aurastudio_document::Clip {
            rect: aurastudio_core::Rect::from_xywh(x as i32, y as i32, img.width as u32, img.height as u32),
            px: img.bytes.as_chunks::<4>().0.to_vec(),
        })
    }

    pub(super) fn file_cmd(&mut self, cmd: FileCmd) {
        if self.is_drawing || self.ui.has_dialog() { return; }
        self.cancel_filter();
        // What is saved or exported is the transform kept.
        if matches!(cmd, FileCmd::Save | FileCmd::SaveAs | FileCmd::Export | FileCmd::Close) { self.end_transform(true); }
        match cmd {
            FileCmd::New => self.ui.open_new_dialog(),
            FileCmd::Open => self.open_dialog(),
            // Nothing to save or close on the start screen.
            _ if self.home => {}
            FileCmd::Save => self.save(false, None),
            FileCmd::SaveAs => self.save(true, None),
            FileCmd::Export => self.export(),
            FileCmd::Close => self.request(Pending::Close),
        }
        self.request_redraw();
    }

    /// Close the filter dialog, putting the layer back as it was.
    pub(super) fn cancel_filter(&mut self) {
        if self.ui.close_filter() {
            self.send_edit(Edit::FilterCancel);
        }
    }

    /// Do `action`, first asking to save unsaved changes.
    pub(super) fn request(&mut self, action: Pending) {
        self.cancel_filter();
        self.end_transform(true);
        let (modified, name) = {
            let d = self.doc.lock().unwrap();
            (d.is_modified(), d.name.clone())
        };
        if modified {
            self.pending = Some(action);
            self.ui.show_dialog(Dialog::UnsavedChanges { name });
            self.request_redraw();
        } else {
            self.perform(action);
        }
    }

    pub(super) fn perform(&mut self, action: Pending) {
        match action {
            Pending::Create(spec) => {
                let id = self.workspace.reserve_id();
                // An animation starts with its paper and an animation folder.
                let mut doc = Document::new(id, &spec.name, spec.width, spec.height, spec.kind);
                doc.apply(Edit::SetDpi(spec.dpi));
                doc.mark_saved(doc.revision());
                log::info!("new document: {}, {} × {} px, {} dpi", spec.kind.name(), spec.width, spec.height, spec.dpi);
                self.install(doc);
                self.home = false;
            }
            Pending::Open(path) => self.open_path(&path),
            Pending::Close => {
                let id = self.workspace.reserve_id();
                self.install(Document::new(id, "", PLACEHOLDER.0, PLACEHOLDER.1, DocType::Illustration));
                self.home = true;
                self.refresh_home();
                crate::memory::event("document closed");
            }
            Pending::Quit => {
                // Leaving on purpose: no work to recover next time.
                if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); }
                self.quit = true;
                let _ = self.proxy.send_event(AppEvent::Wake);
            }
        }
    }

    pub(super) fn on_dialog(&mut self, choice: DialogChoice) {
        if std::mem::take(&mut self.recovery_offered) {
            match choice {
                DialogChoice::Recover => self.recover(),
                _ => if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); },
            }
            return;
        }
        let Some(action) = self.pending.take() else { return };
        match choice {
            DialogChoice::Save => self.save(false, Some(action)),
            DialogChoice::Discard => self.perform(action),
            DialogChoice::Cancel | DialogChoice::Recover => {}
        }
    }

    pub(super) fn open_dialog(&mut self) {
        let mut all = vec![EXTENSION];
        all.extend(IMAGE_EXTENSIONS);
        let picked = rfd::FileDialog::new()
            .set_title("Open")
            .add_filter("AuraStudio documents and images", &all)
            .add_filter("AuraStudio document", &[EXTENSION])
            .add_filter("Images", &IMAGE_EXTENSIONS)
            .pick_file();
        if let Some(path) = picked { self.request(Pending::Open(path)); }
    }

    /// Open an .aurs document, or an image as a new document.
    pub(super) fn open_path(&mut self, path: &Path) {
        let (id, max) = (self.workspace.reserve_id(), self.max_canvas());
        let opened = if has_extension(path, EXTENSION) {
            aurastudio_file::open(path, max).map(|snap| {
                let mut d = Document::from_snapshot(id, &aurastudio_file::doc_name(path), snap);
                d.path = Some(path.to_path_buf());
                d
            })
        } else {
            aurastudio_file::import_image(path, max, DocType::Illustration.default_tile_size())
                .map(|(name, stack)| Document::from_stack(id, &name, stack, DocType::Illustration))
        };
        match opened {
            Ok(doc) => {
                log::info!("opened {}", path.display());
                self.install(doc);
                self.home = false;
                self.add_recent(path);
            }
            Err(e) => {
                log::warn!("open {}: {e}", path.display());
                self.ui.notify(&format!("Can't open {}: {e}", aurastudio_file::doc_name(path)));
                self.request_redraw();
            }
        }
    }

    /// Show `doc` in place of the current document.
    pub(super) fn install(&mut self, doc: Document) {
        let (w, h) = (doc.stack().width, doc.stack().height);
        // A transform, polygon or playback of the document going away.
        self.play = None;
        self.play_next = None;
        self.xf = None;
        self.lift = None;
        self.xf_pending = false;
        self.poly = None;
        let old = self.doc.lock().unwrap().id();
        self.autosaved_revision = doc.revision();
        self.autosave_at = None;
        self.doc = self.workspace.add(doc);
        crate::rescue::watch(&self.doc);
        self.apply_onion_look();
        self.apply_color_management();
        self.workspace.close(old);
        *self.projection.lock().unwrap() = Projection::new(w, h);
        if let Some(gpu) = &mut self.gpu { gpu.resize_canvas(w, h); }
        self.canvas_edit = (w, h);
        self.ui.invalidate_all_thumbnails();
        self.layers_dirty = true;
        self.request_redraw();
        crate::memory::event("document shown (new / opened)");
    }

    /// Ctrl+S (`save_as`: Ctrl+Shift+S, or no .aurs file yet: ask where).
    /// `then`: done once saved.
    pub(super) fn save(&mut self, save_as: bool, then: Option<Pending>) {
        let (path, name) = {
            let d = self.doc.lock().unwrap();
            (d.path.clone(), d.name.clone())
        };
        let path = match path.filter(|p| !save_as && has_extension(p, EXTENSION)) {
            Some(p) => p,
            None => {
                let picked = rfd::FileDialog::new()
                    .set_title("Save")
                    .add_filter("AuraStudio document", &[EXTENSION])
                    .set_file_name(format!("{name}.{EXTENSION}"))
                    .save_file();
                match picked {
                    Some(p) if has_extension(&p, EXTENSION) => p,
                    Some(p) => p.with_extension(EXTENSION),
                    // Cancelled: whatever was to follow doesn't happen.
                    None => return,
                }
            }
        };
        self.after_save = then;
        self.paint.send(Cmd::Save { doc: Arc::clone(&self.doc), path, origin: None, auto: false });
        self.ui.notify("Saving…");
        self.request_redraw();
    }

    pub(super) fn on_saved(&mut self, doc: aurastudio_document::DocId, path: PathBuf, revision: u64, auto: bool, result: Result<(), String>) {
        {
            let mut d = self.doc.lock().unwrap();
            // A save of a document closed since: nothing left to update.
            if d.id() != doc { return; }
            if !auto && result.is_ok() {
                if d.path.as_deref() != Some(&path) {
                    d.name = aurastudio_file::doc_name(&path);
                    d.path = Some(path.clone());
                }
                d.mark_saved(revision);
            }
        }
        match (auto, result) {
            (true, Ok(())) => {}
            (true, Err(e)) => log::warn!("autosave failed: {e}"),
            (false, Ok(())) => {
                // The work is safe in its own file now.
                if let Some(p) = recovery_path() { let _ = std::fs::remove_file(p); }
                self.autosaved_revision = revision;
                self.add_recent(&path);
                self.ui.notify(&format!("Saved {}", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
                if let Some(action) = self.after_save.take() { self.perform(action); }
            }
            (false, Err(e)) => {
                self.after_save = None;
                self.ui.notify(&format!("Save failed: {e}"));
            }
        }
        self.request_redraw();
    }

    /// The merged image to PNG / JPG (on a thread: big images take a
    /// moment to encode).
    pub(super) fn export(&mut self) {
        let name = self.doc.lock().unwrap().name.clone();
        let picked = rfd::FileDialog::new()
            .set_title("Export")
            .add_filter("PNG image", &["png"])
            .add_filter("JPEG image", &["jpg", "jpeg"])
            .set_file_name(format!("{name}.png"))
            .save_file();
        let Some(mut path) = picked else { return };
        if !aurastudio_file::EXPORT_EXTENSIONS.iter().any(|e| has_extension(&path, e)) {
            path.set_extension("png");
        }
        // The whole merged image is only made now, for the file, on the
        // export thread (the screen shows it from the GPU).
        let (doc, proxy) = (Arc::clone(&self.doc), self.proxy.clone());
        self.ui.notify("Exporting…");
        std::thread::spawn(move || {
            let (pixels, w, h) = {
                let d = doc.lock().unwrap();
                let (w, h) = (d.stack().width, d.stack().height);
                // Without the draft layers (CSP).
                (d.merged(aurastudio_core::Rect::of_size(w, h), aurastudio_layers::Pick::EXPORT), w, h)
            };
            let file = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let msg = match aurastudio_file::export_image(&pixels, w, h, &path) {
                Ok(()) => format!("Exported {file}"),
                Err(e) => format!("Export failed: {e}"),
            };
            let _ = proxy.send_event(AppEvent::Notice(msg));
        });
        self.request_redraw();
    }

    /// Save unsaved work to the recovery file some time after it changed
    /// (on the paint thread's queue, like any save).
    pub(super) fn check_autosave(&mut self) {
        let Ok(d) = self.doc.try_lock() else { return };
        let rev = d.revision();
        // (Not a filter's or a transform's preview: it isn't the document
        // yet.)
        if !d.is_modified() || rev == self.autosaved_revision || d.filtering() || d.transforming() {
            self.autosave_at = None;
            return;
        }
        let now = Instant::now();
        let Some(after) = autosave_after(self.prefs.autosave_minutes) else { return };
        match self.autosave_at {
            None => self.autosave_at = Some(now + after),
            Some(t) if now >= t && !self.is_drawing => {
                if let Some(path) = recovery_path() {
                    let origin = d.path.clone();
                    self.paint.send(Cmd::Save { doc: Arc::clone(&self.doc), path, origin, auto: true });
                }
                self.autosaved_revision = rev;
                self.autosave_at = None;
            }
            _ => {}
        }
    }

    /// At startup: an autosave left by a session that didn't close
    /// normally? Ask whether to recover it.
    pub(super) fn offer_recovery(&mut self) {
        let Some(path) = recovery_path().filter(|p| p.is_file()) else { return };
        let origin = aurastudio_file::peek_origin(&path);
        let name = origin.as_deref().map_or_else(|| "Untitled".to_string(), aurastudio_file::doc_name);
        let saved = std::fs::metadata(&path).and_then(|m| m.modified()).map_or_else(|_| "earlier".into(), ago);
        self.recovery_offered = true;
        self.ui.show_dialog(Dialog::Recover { name, saved });
        self.request_redraw();
    }

    pub(super) fn recover(&mut self) {
        let Some(path) = recovery_path() else { return };
        let (id, max) = (self.workspace.reserve_id(), self.max_canvas());
        match aurastudio_file::open(&path, max) {
            Ok(snap) => {
                let origin = snap.origin.clone();
                let name = origin.as_deref().map_or_else(|| "Untitled".into(), aurastudio_file::doc_name);
                let mut doc = Document::from_snapshot(id, &name, snap);
                doc.path = origin;
                // Unsaved: it isn't in its own file yet.
                doc.touch();
                self.install(doc);
                self.ui.notify("Recovered unsaved work");
            }
            Err(e) => {
                log::warn!("recovery: {e}");
                self.ui.notify(&format!("Can't recover: {e}"));
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Put `path` first in the recent files.
    pub(super) fn add_recent(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        save_recent(&self.recent);
    }

    /// Recent files on the start screen; their thumbnails are decoded on a
    /// thread and arrive one by one.
    pub(super) fn refresh_home(&mut self) {
        self.recent.retain(|p| p.is_file());
        let list = self.recent.iter().map(|p| {
            let ext = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            let when = std::fs::metadata(p).and_then(|m| m.modified()).map_or_else(|_| String::new(), ago);
            aurastudio_ui::RecentFile::new(p.clone(), aurastudio_file::doc_name(p), format!("{ext} · {when}"))
        }).collect();
        self.ui.set_recent(list);
        let (paths, proxy) = (self.recent.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            for path in paths {
                if let Some((w, h, rgba)) = aurastudio_file::thumbnail(&path, 280) {
                    let _ = proxy.send_event(AppEvent::Thumbnail { path, w, h, rgba: rgba.into_flattened() });
                }
            }
        });
        self.request_redraw();
    }

    /// "name* — AuraStudio" (the star: unsaved changes).
    pub(super) fn update_title(&mut self) {
        let Ok(d) = self.doc.try_lock() else { return };
        let title = if self.home {
            APP_TITLE.to_string()
        } else {
            format!("{}{} — {APP_TITLE}", d.name, if d.is_modified() { "*" } else { "" })
        };
        drop(d);
        if title != self.title {
            if let Some(w) = &self.window { w.set_title(&title); }
            self.title = title;
        }
    }

    /// Delete key / Layer > Clear: erase the active layer (white for the
    /// background, transparent for the others), undoable. Done by the paint
    /// thread, after any stroke still being painted.
    pub(super) fn clear_active_layer(&mut self) {
        if self.is_drawing { return; }
        self.send_edit(Edit::ClearActive);
    }

    pub(super) fn undo(&mut self) {
        if self.is_drawing { return; }
        if let Some(xf) = &mut self.xf {
            if xf.drag.is_some() { return; }
            // Inside the transform first.
            if xf.step_back() { self.send_preview(false); return; }
            // Nothing done in it: the document's undo, then a new one.
            self.end_transform(false);
            self.send_edit(Edit::Undo);
            self.start_transform();
            return;
        }
        self.poly = None;
        self.send_edit(Edit::Undo);
    }

    pub(super) fn redo(&mut self) {
        if self.is_drawing { return; }
        if let Some(xf) = &mut self.xf {
            if xf.drag.is_some() { return; }
            if xf.step_forward() { self.send_preview(false); return; }
            if xf.changed() { return; }
            self.end_transform(false);
            self.send_edit(Edit::Redo);
            self.start_transform();
            return;
        }
        self.send_edit(Edit::Redo);
    }
}

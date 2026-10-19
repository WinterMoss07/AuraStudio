//! Documents (layers + their own undo history), each its own
//! `Arc<Mutex<Document>>`, and the workspace holding the open ones.

mod history;

use std::sync::{Arc, Mutex};

use aurastudio_canvas::Rect;
use aurastudio_layers::{FillParams, LayerStack, Selection};

pub use history::{History, SavedStep, DEFAULT_BUDGET};

/// Stable id of an open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// What an undo / redo changed (to recomposite, refresh the thumbnail).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub layer_id: u64,
    pub rect:     Rect,
}

pub struct Document {
    id:         DocId,
    pub name:   String,
    pub layers: LayerStack,
    history:    History,
    /// Print resolution (dots per inch), kept in the file.
    pub dpi:    f32,
    /// File the document was opened from / last saved to.
    pub path:   Option<std::path::PathBuf>,
    /// Bumped by every change; compared with `saved_revision` to know if
    /// there is unsaved work.
    revision:       u64,
    saved_revision: u64,
    /// Pixels painting, filling and clearing may change; None: all.
    pub selection:  Option<Selection>,
    /// A filter being previewed (its dialog is open).
    filter:     Option<FilterSession>,
}

/// A filter shown on the active layer while its dialog is open: the
/// layer's pixels from before, to filter again with other values, to put
/// back (Cancel), or to record for undo (Apply).
struct FilterSession {
    layer:  u64,
    rect:   Rect,
    before: Vec<u8>,
}

/// How far around the painted area (or the selection) a filter may reach
/// (a 100 px blur, a shadow moved 200 px).
const FILTER_MARGIN: u32 = 512;

impl Document {
    pub fn new(id: DocId, name: &str, width: u32, height: u32) -> Self {
        Self::from_parts(id, name, LayerStack::new_document(width, height), History::new(DEFAULT_BUDGET))
    }

    /// A document from its parts (a file being opened). It counts as
    /// saved.
    pub fn from_parts(id: DocId, name: &str, layers: LayerStack, history: History) -> Self {
        Self { id, name: name.to_owned(), layers, history, dpi: DEFAULT_DPI, path: None, revision: 0, saved_revision: 0, selection: None, filter: None }
    }

    /// Something changed (the layer panel edits the stack directly and
    /// calls this; strokes, clear, undo, redo and resize do it here).
    pub fn touch(&mut self) { self.revision += 1; }

    pub fn revision(&self) -> u64 { self.revision }

    /// The document as of `revision` is now on disk.
    pub fn mark_saved(&mut self, revision: u64) { self.saved_revision = revision; }

    pub fn is_modified(&self) -> bool { self.revision != self.saved_revision }

    pub fn id(&self) -> DocId { self.id }

    pub fn history(&self) -> &History { &self.history }

    /// Record a finished stroke on `layer_id`, which changed `rect`. The
    /// pixels from before the stroke are still in the layer stack's stroke
    /// snapshot, so only that rectangle is copied (and compressed).
    pub fn commit_stroke(&mut self, layer_id: u64, rect: Rect) {
        let before = self.layers.stroke_base_rect(rect);
        self.history.push("Brush stroke", layer_id, rect, &before);
        self.touch();
    }

    /// Clear the active layer (white for the background), undoable.
    /// Returns the cleared layer id, or None if it is locked / a folder.
    /// With a selection, only the selected pixels are cleared.
    pub fn clear_active(&mut self) -> Option<(u64, Rect)> {
        if !self.layers.can_paint() { return None; }
        let id = self.layers.active_id();
        let rect = match &self.selection {
            Some(s) => s.bbox(),
            None => (0, 0, self.layers.width, self.layers.height),
        };
        let before = self.layers.layer_rect(id, rect)?;
        match &self.selection {
            Some(s) => { self.layers.clear_selected(id, s)?; }
            None => { self.layers.clear_active(); }
        }
        self.history.push("Clear layer", id, rect, &before);
        self.touch();
        Some((id, rect))
    }

    /// Fill tool at (x, y) on the active layer, undoable. Returns the
    /// layer and the rectangle changed.
    pub fn fill(&mut self, x: u32, y: u32, color: [u8; 4], p: FillParams) -> Option<(u64, Rect)> {
        if !self.layers.can_paint() { return None; }
        let id = self.layers.active_id();
        let area = aurastudio_layers::flood(&self.layers, id, x, y, p, self.selection.as_ref())?;
        let before = self.layers.layer_rect(id, area.bbox)?;
        let rect = self.layers.fill_area(id, &area, color)?;
        self.history.push("Fill", id, rect, &before);
        self.touch();
        Some((id, rect))
    }

    /// Show `filter` on the active layer (None: the layer as it was).
    /// The first call starts a session. Returns the layer and the area to
    /// redraw, or None if the active layer can't be painted.
    pub fn filter_preview(&mut self, filter: Option<&aurastudio_filters::Filter>) -> Option<(u64, Rect)> {
        if self.filter.is_none() {
            if !self.layers.can_paint() { return None; }
            let layer = self.layers.active_id();
            let (w, h) = (self.layers.width, self.layers.height);
            let aurastudio_layers::Node::Layer(l) = self.layers.node(layer)? else { return None };
            // A layer whose empty parts are transparent changes only around
            // what is painted; the paper changes everywhere.
            let area = match (&self.selection, l.tiles.fill()[3], l.tiles.content_bounds()) {
                (Some(s), _, _) => s.bbox(),
                (None, 0, Some(b)) => b,
                (None, 0, None) => return Some((layer, (0, 0, 0, 0))),
                (None, _, _) => (0, 0, w, h),
            };
            let m = FILTER_MARGIN;
            let rect = (area.0.saturating_sub(m), area.1.saturating_sub(m), (area.2 + m).min(w), (area.3 + m).min(h));
            let before = self.layers.layer_rect(layer, rect)?;
            self.filter = Some(FilterSession { layer, rect, before });
        }
        let s = self.filter.as_ref()?;
        let (layer, rect) = (s.layer, s.rect);
        let (rw, rh) = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
        if rw == 0 || rh == 0 { return Some((layer, rect)); }
        let px = match filter {
            None => s.before.clone(),
            Some(f) => {
                let mut px = f.apply(&s.before, rw, rh);
                // Only selected pixels change; a transparent pixel stays
                // the empty value (no memory taken by invisible colors).
                let mut i = 0;
                for y in rect.1..rect.3 {
                    for x in rect.0..rect.2 {
                        let outside = self.selection.as_ref().is_some_and(|sel| !sel.contains(x, y));
                        if outside || (px[i + 3] == 0 && s.before[i + 3] == 0) {
                            px[i..i + 4].copy_from_slice(&s.before[i..i + 4]);
                        }
                        i += 4;
                    }
                }
                px
            }
        };
        self.layers.set_layer_rect(layer, rect, &px);
        Some((layer, rect))
    }

    /// Keep `filter` on the layer (undoable as `name`). Returns what
    /// changed.
    pub fn filter_apply(&mut self, filter: &aurastudio_filters::Filter) -> Option<(u64, Rect)> {
        let changed = self.filter_preview(Some(filter))?;
        let s = self.filter.take()?;
        if changed.1.2 > changed.1.0 {
            self.history.push(filter.kind.name(), s.layer, s.rect, &s.before);
            self.touch();
        }
        Some(changed)
    }

    /// Put the layer back as it was before the filter's dialog opened.
    pub fn filter_cancel(&mut self) -> Option<(u64, Rect)> {
        let s = self.filter.take()?;
        self.layers.set_layer_rect(s.layer, s.rect, &s.before);
        Some((s.layer, s.rect))
    }

    /// A filter dialog is open on this document.
    pub fn filtering(&self) -> bool { self.filter.is_some() }

    pub fn undo(&mut self) -> Option<Change> {
        let c = self.history.undo(&mut self.layers);
        if c.is_some() { self.touch(); }
        c
    }

    pub fn redo(&mut self) -> Option<Change> {
        let c = self.history.redo(&mut self.layers);
        if c.is_some() { self.touch(); }
        c
    }

    /// New canvas size. The saved rectangles no longer match the layers,
    /// so the history is dropped (Krita does the same without an undoable
    /// resize command).
    pub fn resize(&mut self, width: u32, height: u32) {
        // A filter being previewed is dropped first (its saved pixels
        // are for the old size).
        self.filter_cancel();
        self.layers.resize(width, height);
        self.history.clear();
        self.selection = None;
        self.touch();
    }
}

/// Pixels copied from a layer: RGBA rows, and where they were (canvas
/// pixels, may lie partly off the canvas when pasted elsewhere).
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub x:      i64,
    pub y:      i64,
    pub width:  u32,
    pub height: u32,
    pub rgba:   Vec<u8>,
}

impl Document {
    /// The selected pixels of the active layer (outside the selection:
    /// transparent), or the whole layer. None on a folder, or with nothing
    /// there to copy.
    pub fn copy(&self) -> Option<Clip> {
        let id = self.layers.active_id();
        // Only where the layer has content: copying a whole page that is
        // mostly empty would put hundreds of MB on the clipboard. (The
        // Paper's content is its white: it copies whole.)
        let aurastudio_layers::Node::Layer(l) = self.layers.node(id)? else { return None };
        let full = (0, 0, self.layers.width, self.layers.height);
        let content = if l.background { Some(full) } else { l.tiles.content_bounds() };
        let r = match (&self.selection, content) {
            (_, None) => return None,
            (None, Some(c)) => c,
            (Some(s), Some(c)) => {
                let b = s.bbox();
                let r = (b.0.max(c.0), b.1.max(c.1), b.2.min(c.2), b.3.min(c.3));
                if r.0 >= r.2 || r.1 >= r.3 { return None; }
                r
            }
        };
        let mut rgba = self.layers.layer_rect(id, r)?;
        if let Some(sel) = &self.selection {
            let mut i = 0;
            for y in r.1..r.3 {
                for x in r.0..r.2 {
                    if !sel.contains(x, y) { rgba[i + 3] = 0; }
                    i += 4;
                }
            }
        }
        Some(Clip { x: r.0 as i64, y: r.1 as i64, width: r.2 - r.0, height: r.3 - r.1, rgba })
    }

    /// Paste on a new layer above the active one (which becomes active),
    /// undoable. Returns the new layer and the area painted.
    pub fn paste(&mut self, clip: &Clip) -> Option<(u64, Rect)> {
        if clip.rgba.len() != clip.width as usize * clip.height as usize * 4 { return None; }
        let (w, h) = (self.layers.width as i64, self.layers.height as i64);
        let (x0, y0) = (clip.x.max(0), clip.y.max(0));
        let (x1, y1) = ((clip.x + clip.width as i64).min(w), (clip.y + clip.height as i64).min(h));
        let id = self.layers.add_layer("Pasted");
        self.layers.set_active(id);
        self.touch();
        if x0 >= x1 || y0 >= y1 { return Some((id, (0, 0, 0, 0))); }
        let rect = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut px = Vec::with_capacity(rw * rh * 4);
        for y in y0..y1 {
            let row = ((y - clip.y) as usize * clip.width as usize + (x0 - clip.x) as usize) * 4;
            px.extend_from_slice(&clip.rgba[row..row + rw * 4]);
        }
        let before = self.layers.layer_rect(id, rect)?;
        self.layers.set_layer_rect(id, rect, &px);
        self.history.push("Paste", id, rect, &before);
        Some((id, rect))
    }
}

/// Resolution of a new document until the New dialog sets one.
pub const DEFAULT_DPI: f32 = 300.0;

pub type SharedDoc = Arc<Mutex<Document>>;

/// Every open document (ids kept beside them: finding one never locks it).
pub struct Workspace {
    docs:    Vec<(DocId, SharedDoc)>,
    next_id: u64,
}

impl Default for Workspace {
    fn default() -> Self { Self::new() }
}

impl Workspace {
    pub fn new() -> Self {
        Self { docs: Vec::new(), next_id: 1 }
    }

    /// Open a new blank document.
    pub fn new_document(&mut self, name: &str, width: u32, height: u32) -> SharedDoc {
        let id = self.reserve_id();
        self.add(Document::new(id, name, width, height))
    }

    /// Id for a document about to be built (a file being opened); add it
    /// with `add`.
    pub fn reserve_id(&mut self) -> DocId {
        let id = DocId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a document (its id from `reserve_id`).
    pub fn add(&mut self, doc: Document) -> SharedDoc {
        self.next_id = self.next_id.max(doc.id().0 + 1);
        let id = doc.id();
        let doc = Arc::new(Mutex::new(doc));
        self.docs.push((id, Arc::clone(&doc)));
        doc
    }

    /// Close a document; its memory (layers + history) goes with it.
    pub fn close(&mut self, id: DocId) -> bool {
        let Some(i) = self.docs.iter().position(|(d, _)| *d == id) else { return false };
        self.docs.remove(i);
        true
    }

    pub fn len(&self) -> usize { self.docs.len() }

    pub fn is_empty(&self) -> bool { self.docs.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_layers::Node;
    use aurastudio_canvas::PaintSurface;

    fn paint(doc: &mut Document, id: u64, rect: Rect, rgba: [u8; 4]) {
        // Simulates a stroke: snapshot, change pixels, commit.
        assert!(doc.layers.begin_stroke(id));
        // Like the brush engines: the area is prepared before painting.
        doc.layers.layer_surface(id).unwrap().prepare(rect);
        let n = ((rect.2 - rect.0) * (rect.3 - rect.1)) as usize;
        assert!(doc.layers.set_layer_rect(id, rect, &rgba.repeat(n)));
        doc.commit_stroke(id, rect);
    }

    fn px(doc: &Document, id: u64, x: u32, y: u32) -> [u8; 4] {
        let Some(Node::Layer(l)) = doc.layers.node(id) else { panic!() };
        l.tiles.pixel(x, y)
    }

    #[test]
    fn filters_preview_cancel_apply_and_undo() {
        use aurastudio_filters::{Filter, Kind};
        let mut doc = Document::new(DocId(1), "a", 64, 64);
        let id = doc.layers.add_layer("ink");
        doc.layers.set_active(id);
        paint(&mut doc, id, (10, 10, 20, 20), [200, 100, 50, 255]);
        let inv = Filter::new(Kind::Invert);
        let empty = px(&doc, id, 40, 40);
        doc.filter_preview(Some(&inv)).unwrap();
        assert_eq!(px(&doc, id, 15, 15), [55, 155, 205, 255]);
        assert!(doc.filtering());
        // Empty pixels stay the layer's empty value.
        assert_eq!(px(&doc, id, 40, 40), empty);
        doc.filter_cancel();
        assert_eq!(px(&doc, id, 15, 15), [200, 100, 50, 255]);
        assert!(!doc.filtering());

        let undo_before = doc.history().undo_len();
        doc.filter_preview(Some(&Filter::new(Kind::Grayscale)));
        doc.filter_apply(&inv).unwrap();
        assert_eq!(px(&doc, id, 15, 15), [55, 155, 205, 255]);
        assert_eq!(doc.history().undo_len(), undo_before + 1);
        doc.undo();
        assert_eq!(px(&doc, id, 15, 15), [200, 100, 50, 255]);

        // A selection limits it; the paper (opaque fill) changes too.
        doc.selection = Selection::rect(64, 64, (0, 0, 15, 64));
        doc.layers.set_active(1);
        assert!(doc.filter_apply(&inv).is_none(), "the paper is locked");
        *doc.layers.node_mut(1).unwrap().locked_mut() = false;
        doc.filter_apply(&inv).unwrap();
        let Some(Node::Layer(paper)) = doc.layers.node(1) else { panic!() };
        assert_eq!(paper.tiles.pixel(5, 5), [0, 0, 0, 255]);
        assert_eq!(paper.tiles.pixel(30, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn undo_redo_restore_exact_pixels() {
        let mut doc = Document::new(DocId(1), "a", 64, 64);
        let id = doc.layers.add_layer("ink");
        paint(&mut doc, id, (10, 10, 20, 20), [255, 0, 0, 255]);
        paint(&mut doc, id, (15, 15, 30, 30), [0, 0, 255, 255]);
        assert_eq!(px(&doc, id, 16, 16), [0, 0, 255, 255]);

        let c = doc.undo().unwrap();
        assert_eq!((c.layer_id, c.rect), (id, (15, 15, 30, 30)));
        assert_eq!(px(&doc, id, 16, 16), [255, 0, 0, 255]);
        assert_eq!(px(&doc, id, 25, 25), [255, 255, 255, 0]);
        doc.undo().unwrap();
        assert_eq!(px(&doc, id, 12, 12), [255, 255, 255, 0]);
        assert!(doc.undo().is_none());

        doc.redo().unwrap();
        doc.redo().unwrap();
        assert_eq!(px(&doc, id, 16, 16), [0, 0, 255, 255]);
        assert_eq!(px(&doc, id, 12, 12), [255, 0, 0, 255]);
        assert!(doc.redo().is_none());
    }

    #[test]
    fn new_action_drops_redo() {
        let mut doc = Document::new(DocId(1), "a", 32, 32);
        let id = doc.layers.add_layer("ink");
        paint(&mut doc, id, (0, 0, 4, 4), [1, 2, 3, 255]);
        doc.undo().unwrap();
        paint(&mut doc, id, (8, 8, 12, 12), [4, 5, 6, 255]);
        assert!(doc.redo().is_none());
        assert_eq!(doc.history().undo_len(), 1);
    }

    #[test]
    fn clear_is_undoable_and_respects_lock() {
        let mut doc = Document::new(DocId(1), "a", 32, 32);
        let id = doc.layers.add_layer("ink");
        paint(&mut doc, id, (0, 0, 32, 32), [9, 9, 9, 255]);
        assert_eq!(doc.clear_active().map(|c| c.0), Some(id));
        assert_eq!(px(&doc, id, 5, 5)[3], 0);
        doc.undo().unwrap();
        assert_eq!(px(&doc, id, 5, 5), [9, 9, 9, 255]);

        *doc.layers.node_mut(id).unwrap().locked_mut() = true;
        assert_eq!(doc.clear_active(), None);
    }

    #[test]
    fn budget_drops_oldest_steps() {
        let mut doc = Document::new(DocId(1), "a", 256, 256);
        doc.history = History::new(300 * 1024);
        let id = doc.layers.add_layer("ink");
        // Noise does not compress: each full-canvas step is ~256 KB.
        for k in 0..4u8 {
            assert!(doc.layers.begin_stroke(id));
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ k as u64;
            let data: Vec<u8> = (0..256 * 256 * 4).map(|_| {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17; // xorshift64
                (x >> 24) as u8
            }).collect();
            doc.layers.layer_surface(id).unwrap().prepare((0, 0, 256, 256));
            doc.layers.set_layer_rect(id, (0, 0, 256, 256), &data);
            doc.commit_stroke(id, (0, 0, 256, 256));
        }
        assert!(doc.history().bytes() <= 300 * 1024, "{}", doc.history().bytes());
        assert!(doc.history().undo_len() < 4);
    }

    #[test]
    fn undo_skips_deleted_layers() {
        let mut doc = Document::new(DocId(1), "a", 32, 32);
        let keep = doc.layers.add_layer("keep");
        paint(&mut doc, keep, (0, 0, 4, 4), [1, 1, 1, 255]);
        let gone = doc.layers.add_layer("gone");
        paint(&mut doc, gone, (0, 0, 4, 4), [2, 2, 2, 255]);
        doc.layers.set_active(gone);
        doc.layers.remove_active();
        let c = doc.undo().unwrap();
        assert_eq!(c.layer_id, keep);
    }

    #[test]
    fn documents_have_separate_histories() {
        let mut ws = Workspace::new();
        let a = ws.new_document("a", 32, 32);
        let b = ws.new_document("b", 32, 32);
        {
            let mut a = a.lock().unwrap();
            let id = a.layers.add_layer("ink");
            paint(&mut a, id, (0, 0, 4, 4), [1, 1, 1, 255]);
        }
        assert!(b.lock().unwrap().undo().is_none());
        assert!(a.lock().unwrap().undo().is_some());
        let b_id = b.lock().unwrap().id();
        assert!(ws.close(b_id));
        assert_eq!(ws.len(), 1);
    }

    /// Copy the selected pixels, paste them: same place, on a new active
    /// layer, outside the selection left transparent; undo empties it.
    #[test]
    fn copy_paste_selected_pixels() {
        let mut d = Document::new(DocId(1), "t", 64, 64);
        let id = d.layers.active_id();
        assert!(d.layers.set_layer_rect(id, (0, 0, 64, 64), &[9u8, 8, 7, 255].repeat(64 * 64)));
        d.selection = aurastudio_layers::Selection::polygon(64, 64, &[(10.0, 10.0), (30.0, 10.0), (10.0, 30.0)]);
        let clip = d.copy().unwrap();
        assert_eq!((clip.x, clip.y, clip.width, clip.height), (10, 10, 20, 20));
        let (new, r) = d.paste(&clip).unwrap();
        assert_ne!(new, id);
        assert_eq!(d.layers.active_id(), new);
        assert_eq!(r, (10, 10, 30, 30));
        let px = |d: &Document, x, y| d.layers.layer_rect(new, (x, y, x + 1, y + 1)).unwrap();
        assert_eq!(px(&d, 12, 12), vec![9, 8, 7, 255]);
        assert_eq!(px(&d, 28, 28)[3], 0, "outside the lasso");
        // Partly off the canvas: clipped, no panic.
        let off = Clip { x: -5, y: 60, ..clip.clone() };
        assert!(d.paste(&off).is_some());
        d.undo();
        d.undo();
        assert_eq!(px(&d, 12, 12)[3], 0);
    }
}

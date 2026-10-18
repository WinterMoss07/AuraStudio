//! Documents: one open image each (its layers + its own undo history), and
//! the workspace holding every open document.
//!
//! Each document is its own `Arc<Mutex<Document>>`: painting, undo or a
//! resize in one document never locks or touches another, and Ctrl+Z only
//! ever walks back the history of the document it is pressed in.

mod history;

use std::sync::{Arc, Mutex};

use aurastudio_canvas::Rect;
use aurastudio_layers::LayerStack;

pub use history::{History, DEFAULT_BUDGET};

/// Stable id of an open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// What an undo / redo changed, so the app can recomposite and refresh the
/// layer thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub layer_id: u64,
    pub rect:     Rect,
    /// Name of the undone / redone action ("Brush stroke"...).
    pub label:    &'static str,
}

pub struct Document {
    id:         DocId,
    pub name:   String,
    pub layers: LayerStack,
    history:    History,
}

impl Document {
    pub fn new(id: DocId, name: &str, width: u32, height: u32) -> Self {
        Self {
            id,
            name: name.to_owned(),
            layers: LayerStack::new(width, height),
            history: History::new(DEFAULT_BUDGET),
        }
    }

    pub fn id(&self) -> DocId { self.id }

    pub fn history(&self) -> &History { &self.history }

    /// Record a finished stroke on `layer_id`, which changed `rect`. The
    /// pixels from before the stroke are still in the layer stack's stroke
    /// snapshot, so only that rectangle is copied (and compressed).
    pub fn commit_stroke(&mut self, layer_id: u64, rect: Rect) {
        let before = self.layers.stroke_base_rect(rect);
        self.history.push("Brush stroke", layer_id, rect, &before);
    }

    /// Clear the active layer (white for the background), undoable.
    /// Returns the cleared layer id, or None if it is locked / a folder.
    pub fn clear_active(&mut self) -> Option<u64> {
        if !self.layers.can_paint() { return None; }
        let id = self.layers.active_id();
        let full = (0, 0, self.layers.width, self.layers.height);
        let before = self.layers.layer_rect(id, full)?;
        self.layers.clear_active();
        self.history.push("Clear layer", id, full, &before);
        Some(id)
    }

    pub fn undo(&mut self) -> Option<Change> {
        self.history.undo(&mut self.layers)
    }

    pub fn redo(&mut self) -> Option<Change> {
        self.history.redo(&mut self.layers)
    }

    /// New canvas size. The saved rectangles no longer match the layers,
    /// so the history is dropped (Krita does the same without an undoable
    /// resize command).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.layers.resize(width, height);
        self.history.clear();
    }
}

pub type SharedDoc = Arc<Mutex<Document>>;

/// Every open document, and which one is shown.
pub struct Workspace {
    /// Ids kept next to the documents: finding one never locks them (a
    /// document can be busy being painted on the paint thread).
    docs:    Vec<(DocId, SharedDoc)>,
    active:  usize,
    next_id: u64,
}

impl Default for Workspace {
    fn default() -> Self { Self::new() }
}

impl Workspace {
    pub fn new() -> Self {
        Self { docs: Vec::new(), active: 0, next_id: 1 }
    }

    /// Open a new blank document and make it the active one.
    pub fn new_document(&mut self, name: &str, width: u32, height: u32) -> SharedDoc {
        let id = DocId(self.next_id);
        self.next_id += 1;
        let doc = Arc::new(Mutex::new(Document::new(id, name, width, height)));
        self.docs.push((id, Arc::clone(&doc)));
        self.active = self.docs.len() - 1;
        doc
    }

    /// The document being shown (None when nothing is open).
    pub fn active(&self) -> Option<SharedDoc> {
        self.docs.get(self.active).map(|(_, d)| Arc::clone(d))
    }

    pub fn set_active(&mut self, id: DocId) -> bool {
        match self.docs.iter().position(|(d, _)| *d == id) {
            Some(i) => { self.active = i; true }
            None => false,
        }
    }

    /// Close a document; its memory (layers + history) goes with it.
    pub fn close(&mut self, id: DocId) -> bool {
        let Some(i) = self.docs.iter().position(|(d, _)| *d == id) else { return false };
        self.docs.remove(i);
        self.active = self.active.min(self.docs.len().saturating_sub(1));
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
        let i = ((y * doc.layers.width + x) * 4) as usize;
        [l.pixels[i], l.pixels[i + 1], l.pixels[i + 2], l.pixels[i + 3]]
    }

    #[test]
    fn undo_redo_restore_exact_pixels() {
        let mut doc = Document::new(DocId(1), "a", 64, 64);
        let id = doc.layers.add_layer("ink");
        paint(&mut doc, id, (10, 10, 20, 20), [255, 0, 0, 255]);
        paint(&mut doc, id, (15, 15, 30, 30), [0, 0, 255, 255]);
        assert_eq!(px(&doc, id, 16, 16), [0, 0, 255, 255]);

        let c = doc.undo().unwrap();
        assert_eq!((c.layer_id, c.rect, c.label), (id, (15, 15, 30, 30), "Brush stroke"));
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
        assert_eq!(doc.clear_active(), Some(id));
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
        assert_eq!(ws.active().unwrap().lock().unwrap().name, "a");
    }
}

//! Every open document, each its own `Arc<Mutex<Document>>` (ids kept
//! beside them: finding one never locks it).

use std::sync::{Arc, Mutex};

use aurastudio_core::DocType;

use crate::{DocId, Document};

pub type SharedDoc = Arc<Mutex<Document>>;

pub struct Workspace {
    docs:    Vec<(DocId, SharedDoc)>,
    next_id: u64,
}

impl Default for Workspace {
    fn default() -> Self { Self::new() }
}

impl Workspace {
    pub fn new() -> Self { Self { docs: Vec::new(), next_id: 1 } }

    /// Open a new blank document.
    pub fn new_document(&mut self, name: &str, width: u32, height: u32, kind: DocType) -> SharedDoc {
        let id = self.reserve_id();
        self.add(Document::new(id, name, width, height, kind))
    }

    /// Id for a document about to be built (a file being opened).
    pub fn reserve_id(&mut self) -> DocId {
        let id = DocId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn add(&mut self, doc: Document) -> SharedDoc {
        self.next_id = self.next_id.max(doc.id().0 + 1);
        let id = doc.id();
        let doc = Arc::new(Mutex::new(doc));
        self.docs.push((id, Arc::clone(&doc)));
        doc
    }

    pub fn get(&self, id: DocId) -> Option<SharedDoc> { self.docs.iter().find(|d| d.0 == id).map(|d| Arc::clone(&d.1)) }

    pub fn ids(&self) -> Vec<DocId> { self.docs.iter().map(|d| d.0).collect() }

    /// Close a document; its memory (layers, history) goes with its last
    /// holder. The app gives memory back to the system after.
    pub fn close(&mut self, id: DocId) -> bool {
        let Some(i) = self.docs.iter().position(|(d, _)| *d == id) else { return false };
        self.docs.remove(i);
        true
    }

    pub fn len(&self) -> usize { self.docs.len() }
    pub fn is_empty(&self) -> bool { self.docs.is_empty() }
}

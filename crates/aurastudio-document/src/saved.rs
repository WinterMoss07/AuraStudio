//! The document as `file` writes and reads it: a neutral snapshot (shared
//! tiles: taking it is quick, the document's lock is short).

use std::collections::HashMap;
use std::path::Path;

use aurastudio_animation::Onion;
use aurastudio_color::Profile;
use aurastudio_core::{Rect, Rgba8};
use aurastudio_file::{OnionMarks, Snapshot, Step as SavedStep, SAVED_UNDO_STEPS};
use aurastudio_layers::LayerStack;
use aurastudio_raster::{legacy, Image, Patch};
use aurastudio_selection::Selection;

use crate::history::{History, Imported, DEFAULT_BUDGET};
use crate::{DocId, Document};

/// Size of the preview saved in files.
const PREVIEW: u32 = 512;

/// What a save takes from the document under its lock: shared tiles and
/// patches. The preview merge and the packing of the steps (the slow part)
/// are done by `finish`, off the lock.
pub struct SnapshotParts {
    snap: Snapshot,
    undo: Vec<Imported>,
    redo: Vec<Imported>,
}

impl SnapshotParts {
    pub fn revision(&self) -> u64 { self.snap.revision }

    pub fn finish(self) -> Snapshot {
        aurastudio_core::zone!("document snapshot finish");
        let Self { mut snap, undo, redo } = self;
        let steps = |v: Vec<Imported>| v.into_iter().map(|i| SavedStep { label: i.label, layer: i.layer, rect: None, data: i.patch.to_bytes(), joined: i.joined, sheet: i.sheet }).collect();
        snap.undo = steps(undo);
        snap.redo = steps(redo);
        snap.preview = Some(aurastudio_compose::preview(&snap.stack, PREVIEW));
        snap
    }
}

impl Document {
    /// What a save writes: the document without the previews under way.
    pub fn snapshot(&self, origin: Option<&Path>) -> Snapshot { self.snapshot_parts(origin).finish() }

    /// The tree as it would be without the filter or transform previews
    /// under way (shared tiles).
    pub fn stack_for_file(&self) -> LayerStack {
        let mut stack = self.stack.clone();
        if let Some(f) = &self.filter { if let Some(i) = stack.image_mut(f.layer()) { *i = f.before().clone(); } }
        if let Some(t) = &self.transform { for (id, before) in t.befores() { if let Some(i) = stack.image_mut(id) { *i = before.clone(); } } }
        stack
    }

    /// `snapshot`, but only what needs the lock.
    pub fn snapshot_parts(&self, origin: Option<&Path>) -> SnapshotParts {
        aurastudio_core::zone!("document snapshot");
        let stack = self.stack_for_file();
        let (undo, redo) = self.history.export(SAVED_UNDO_STEPS);
        let take = |v: Vec<crate::history::Exported<'_>>| v.into_iter().map(|e| Imported { label: e.label.to_owned(), layer: e.layer, patch: e.patch.clone(), joined: e.joined, sheet: e.sheet }).collect();
        let t = &self.timeline;
        let onion = (t.onion != Onion::default()).then(|| OnionMarks { enabled: t.onion.enabled, mobile: t.onion.mobile.iter().map(|(k, v)| (*k, *v)).collect(), fixed: t.onion.fixed.iter().map(|(k, v)| (*k, *v)).collect() });
        let snap = Snapshot {
            dpi: self.dpi, kind: self.kind, depth: self.depth, frame: t.frame(), fps: t.fps, onion,
            profile: (!self.profile.is_srgb()).then(|| (self.profile.name().to_owned(), self.profile.icc().to_vec())),
            selection: self.selection.as_ref().map(|s| s.as_mask().clone()),
            undo: Vec::new(), redo: Vec::new(), preview: None, origin: origin.map(Path::to_path_buf), revision: self.revision,
            stack,
        };
        SnapshotParts { snap, undo: take(undo), redo: take(redo) }
    }

    /// A document from an opened file (it counts as saved). Old rect-based
    /// and 0.19 tile steps become patches of the document's tiles.
    pub fn from_snapshot(id: DocId, name: &str, snap: Snapshot) -> Document {
        aurastudio_core::zone!("document from snapshot");
        let Snapshot { stack, dpi, kind, depth, frame, fps, onion, profile, selection, undo, redo, .. } = snap;
        let undo = import(&stack, undo);
        let redo = import(&stack, redo);
        let mut doc = Document::from_parts(id, name, stack, kind, History::import(DEFAULT_BUDGET, undo, redo));
        doc.dpi = dpi;
        doc.depth = depth;
        // (A profile this version can't read: sRGB, the pixels as they are.)
        doc.profile = profile.and_then(|(n, icc)| Profile::from_icc(&icc, &n).ok()).unwrap_or_else(Profile::srgb);
        doc.selection = selection.and_then(Selection::of);
        doc.timeline.fps = fps;
        if let Some(o) = onion {
            doc.timeline.onion = Onion { enabled: o.enabled, mobile: o.mobile.into_iter().collect(), fixed: o.fixed.into_iter().collect() };
        }
        doc.timeline.set_frame(&mut doc.stack, frame);
        doc.timeline.take_dirty();
        doc.tree = doc.stack.tree_state();
        doc
    }
}

/// Saved steps as patches: new ones read as they are; old ones (pixels of
/// rects) replayed, newest first from the layers as they are now, each
/// giving the patch from its state to the one before it.
fn import(stack: &aurastudio_layers::LayerStack, steps: Vec<SavedStep>) -> Vec<Imported> {
    let frame = stack.frame();
    let mut state: HashMap<u64, Image> = HashMap::new();
    // Undo: the last applies first; redo: the last is the next to redo.
    let mut out: Vec<(bool, Option<Imported>)> = Vec::with_capacity(steps.len());
    for s in steps.into_iter().rev() {
        let joined = s.joined;
        let Some(img) = stack.image(s.layer) else { out.push((joined, None)); continue };
        let step = if Patch::<Rgba8>::is_patch(&s.data) {
            Patch::from_bytes(&s.data).filter(|p| p.check()).map(|patch| Imported { label: s.label, layer: s.layer, patch, joined: s.joined, sheet: s.sheet })
        } else {
            let parts = old_parts(&s, frame);
            parts.and_then(|parts| {
                let now = state.entry(s.layer).or_insert_with(|| img.clone());
                let mut then = now.clone();
                for (r, px) in parts {
                    match px { Some(px) => then.write(r, &px), None => { let f = then.fill(); then.fill_rect(r, f) } }
                }
                then.compact(frame.snapped_out(then.tile_size()));
                let patch = now.diff(&then.snapshot());
                *now = then;
                (!patch.is_empty()).then_some(Imported { label: s.label, layer: s.layer, patch, joined: false, sheet: None })
            })
        };
        out.push((joined, step));
    }
    // A step with a part that can't be read goes whole (its other parts
    // would come back alone).
    out.reverse();
    let mut kept: Vec<Imported> = Vec::with_capacity(out.len());
    let mut group: Vec<Imported> = Vec::new();
    let mut broken = false;
    for (joined, step) in out {
        if !joined {
            if !broken { kept.append(&mut group); }
            group.clear();
            broken = false;
        }
        match step { Some(s) => group.push(s), None => broken = true }
    }
    if !broken { kept.append(&mut group); }
    kept
}

/// An old step's pixels: (rect within the frame, pixels or `None` for the
/// layer's fill).
fn old_parts(s: &SavedStep, frame: Rect) -> Option<Vec<(Rect, Option<Vec<Rgba8>>)>> {
    if legacy::is_old_patch(&s.data) {
        let tiles = legacy::read_old_patch(&s.data)?;
        Some(tiles.into_iter().filter_map(|(r, px)| {
            let c = r.intersect(&frame);
            if c.is_empty() { return None; }
            let px = px.map(|px| {
                let mut out = Vec::with_capacity(c.area() as usize);
                for y in c.y0..c.y1 {
                    let row = ((y - r.y0) as u32 * r.w()) as usize;
                    out.extend_from_slice(&px[row + (c.x0 - r.x0) as usize..row + (c.x1 - r.x0) as usize]);
                }
                out
            });
            Some((c, px))
        }).collect())
    } else {
        let [x0, y0, x1, y1] = s.rect?;
        let r = Rect::new(x0, y0, x1, y1);
        if r.is_empty() || !frame.contains_rect(&r) { return None; }
        let raw = lz4_flex::decompress_size_prepended(&s.data).ok()?;
        if raw.len() != r.area() as usize * 4 { return None; }
        Some(vec![(r, Some(raw.as_chunks::<4>().0.to_vec()))])
    }
}

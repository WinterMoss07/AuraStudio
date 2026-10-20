//! Bug hunt without any interface: millions of random operations straight
//! on the model (layer tree, history, merges, fill, filters, selections,
//! copy / paste, files) and on the real paint thread (strokes with every
//! brush), on small canvases of any shape so each one is quick.
//!
//! Not only "no panic": after every operation the document is checked
//! (unique ids, an active node, sizes, one reference per folder, the
//! history's budget), and now and then results that must agree are
//! compared:
//! - undo n steps then redo them: the same tree and the same pixels;
//! - the plain merge and the general merge; a part of the merge and the
//!   same part of the whole merge (filter layers' reach);
//! - what the paint thread sends to the screen after a whole merge, and
//!   the merge itself;
//! - a file saved and opened: the same document.
//!
//!     PURE_OPS=3000000 PURE_SEED=1 cargo test --release -p aurastudio pure_fuzz -- --ignored --nocapture

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aurastudio_brush::PenSample;
use aurastudio_brushes::Library;
use aurastudio_canvas::{BlendMode, Rect};
use aurastudio_document::{DocId, Document, SharedDoc};
use aurastudio_filters::{Filter, GROUPS};
use aurastudio_layers::{FillParams, FolderBlend, LayerStack, Node, Pick, Place, Refer, Selection};

use crate::fuzz::{rss_mb, Rng};
use crate::paint_thread::{Cmd, PaintThread};
use crate::projection::{Projection, SharedProjection};

static PANICKED: AtomicBool = AtomicBool::new(false);

/// Most nodes a document may hold (duplicating folders grows fast).
const MAX_NODES: usize = 48;

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// A random canvas size: mostly small, sometimes thin or tiny.
fn canvas_size(r: &mut Rng) -> (u32, u32) {
    match r.below(10) {
        0 => (1 + r.below(3) as u32, 1 + r.below(300) as u32),
        1 => (1 + r.below(300) as u32, 1 + r.below(3) as u32),
        2 => (60 + r.below(5) as u32, 60 + r.below(9) as u32),
        3 => (300 + r.below(300) as u32, 200 + r.below(300) as u32),
        _ => (8 + r.below(200) as u32, 8 + r.below(200) as u32),
    }
}

fn all_nodes(nodes: &[Node], out: &mut Vec<u64>) {
    for n in nodes {
        out.push(n.id());
        if let Node::Folder(f) = n { all_nodes(&f.children, out); }
    }
}

/// Everything that must hold after any operation.
pub(crate) fn check(d: &Document) {
    let l = &d.layers;
    fn walk(nodes: &[Node], size: (u32, u32), ids: &mut HashSet<u64>) {
        assert!(nodes.iter().filter(|n| n.props().reference).count() <= 1, "two references in one folder");
        for n in nodes {
            assert!(n.id() != 0 && ids.insert(n.id()), "id {} zero or twice", n.id());
            assert!(n.opacity().is_finite() && (0.0..=1.0).contains(&n.opacity()), "opacity {}", n.opacity());
            match n {
                Node::Layer(x) => assert_eq!((x.tiles.width(), x.tiles.height()), size, "layer {} size", x.id),
                Node::Folder(f) => walk(&f.children, size, ids),
                Node::Filter(_) => {}
            }
        }
    }
    let mut ids = HashSet::new();
    walk(&l.root, (l.width, l.height), &mut ids);
    assert!(l.layer_count() >= 1, "no paint layer left");
    assert!(ids.contains(&l.active_id()), "active {} not in the tree", l.active_id());
    for row in l.rows() { assert!(ids.contains(&row.id)); }
    if let Some(r) = l.reference_for(l.active_id()) { assert!(ids.contains(&r) && r != l.active_id()); }
    let h = d.history();
    assert!(h.undo_len() <= 200);
    assert!(h.undo_len() <= 1 || h.bytes() <= aurastudio_document::DEFAULT_BUDGET, "history over budget");
}

/// The document as undo must give it back: tree settings and order,
/// visibility, every layer's pixels.
#[derive(PartialEq, Debug)]
pub(crate) struct Print {
    tree:    aurastudio_layers::TreeState,
    visible: Vec<(u64, bool)>,
    pixels:  Vec<(u64, u64)>,
    size:    (u32, u32),
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for c in bytes.chunks(8) {
        let mut w = [0u8; 8];
        w[..c.len()].copy_from_slice(c);
        h = (h ^ u64::from_le_bytes(w)).wrapping_mul(0x0100_0000_01b3).rotate_left(29);
    }
    h
}

pub(crate) fn print(l: &LayerStack) -> Print {
    let mut ids = Vec::new();
    all_nodes(&l.root, &mut ids);
    ids.sort_unstable();
    Print {
        tree: l.tree_state(),
        visible: ids.iter().map(|&id| (id, l.node(id).unwrap().visible())).collect(),
        pixels: ids.iter().filter_map(|&id| match l.node(id) {
            Some(Node::Layer(x)) => Some((id, hash(&x.tiles.to_rgba()))),
            _ => None,
        }).collect(),
        size: (l.width, l.height),
    }
}

fn random_filter(r: &mut Rng) -> Filter {
    let kinds: Vec<_> = GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
    let mut f = Filter::new(kinds[r.below(kinds.len() as u64) as usize]);
    for v in f.values.iter_mut() {
        match r.below(6) {
            0 => *v = 0.0,
            1 => *v *= r.range(0.0, 3.0),
            2 => *v = -*v,
            _ => {}
        }
    }
    f.color = [r.below(256) as u8, r.below(256) as u8, r.below(256) as u8];
    f
}

fn random_rect(r: &mut Rng, w: u32, h: u32) -> Rect {
    let x0 = r.below(w as u64) as u32;
    let y0 = r.below(h as u64) as u32;
    (x0, y0, x0 + 1 + r.below((w - x0) as u64) as u32, y0 + 1 + r.below((h - y0) as u64) as u32)
}

fn random_selection(r: &mut Rng, w: u32, h: u32) -> Option<Selection> {
    let (fw, fh) = (w as f32, h as f32);
    match r.below(5) {
        0 => None,
        1 => Some(Selection::all(w, h)),
        2 => Selection::rect(w, h, (r.below(w as u64 + 20) as u32, r.below(h as u64 + 20) as u32, r.below(w as u64 * 2 + 2) as u32, r.below(h as u64 * 2 + 2) as u32)),
        _ => {
            let pts: Vec<(f32, f32)> = (0..r.below(30)).map(|_| (r.range(-0.4, 1.4) * fw, r.range(-0.4, 1.4) * fh)).collect();
            Selection::polygon(w, h, &pts)
        }
    }
}

fn random_fill(r: &mut Rng) -> FillParams {
    FillParams {
        tolerance: r.below(256) as u8,
        refer: [Refer::Layer, Refer::All, Refer::References][r.below(3) as usize],
        skip_drafts: r.chance(0.5),
        expand: r.below(5) as u32,
    }
}

struct Run {
    r:     Rng,
    lib:   Library,
    doc:   SharedDoc,
    pt:    PaintThread,
    proj:  SharedProjection,
    t:     f64,
    /// Operations done, by name; checks done, by name.
    ops:   BTreeMap<&'static str, u64>,
    checks: BTreeMap<&'static str, u64>,
    /// Slowest operations: (ms, name, canvas).
    slow:  Vec<(f64, &'static str, (u32, u32))>,
}

impl Run {
    fn sync(&self) {
        let (tx, rx) = channel();
        self.pt.send(Cmd::Ping(tx));
        rx.recv_timeout(Duration::from_secs(300)).expect("paint thread stalled for 5 minutes");
    }

    fn new_doc(&mut self) {
        let (w, h) = canvas_size(&mut self.r);
        let mut d = Document::new(DocId(1), "pure", w, h);
        if self.r.chance(0.3) { d.layers = LayerStack::new(w, h); d = Document::from_parts(DocId(1), "pure", d.layers, aurastudio_document::History::new(aurastudio_document::DEFAULT_BUDGET)); }
        self.doc = Arc::new(Mutex::new(d));
        *self.proj.lock().unwrap() = Projection::new(w, h);
    }

    fn stroke(&mut self) {
        let (w, h) = { let d = self.doc.lock().unwrap(); (d.layers.width as f32, d.layers.height as f32) };
        let p = &self.lib.presets[self.r.below(self.lib.presets.len() as u64) as usize];
        let mut brush = p.settings.clone();
        brush.radius = match self.r.below(10) { 0 => self.r.range(0.3, 1.0), 1 => self.r.range(60.0, 250.0), _ => self.r.range(1.0, 25.0) };
        brush.erase = self.r.chance(0.15);
        brush.opacity = self.r.range(0.05, 1.0);
        brush.color = [self.r.below(256) as u8, self.r.below(256) as u8, self.r.below(256) as u8];
        let forge = Arc::clone(&p.brush);
        let limit = self.r.chance(0.2).then(|| random_fill(&mut self.r));
        self.pt.send(Cmd::Begin { doc: Arc::clone(&self.doc), brush, forge, limit });
        let n = 1 + self.r.below(40);
        let (mut x, mut y) = (self.r.range(-0.2, 1.2) * w, self.r.range(-0.2, 1.2) * h);
        for _ in 0..n {
            if self.r.chance(0.02) { x = self.r.range(-1e5, 1e5); y = self.r.range(-1e5, 1e5); }
            else { x += self.r.range(-0.15, 0.15) * w.max(20.0); y += self.r.range(-0.15, 0.15) * h.max(20.0); }
            self.t += self.r.range(0.0, 0.02) as f64;
            let pressure = if self.r.chance(0.05) { 0.0 } else { self.r.f() };
            self.pt.send(Cmd::Sample(PenSample::new(x, y, pressure, self.t)));
        }
        // (A Begin without End, now and then: the next one ends it.)
        if self.r.chance(0.95) { self.pt.send(Cmd::End); }
        self.sync();
    }

    fn layers_op(&mut self) -> &'static str {
        let mut d = self.doc.lock().unwrap();
        let r = &mut self.r;
        let mut ids = Vec::new();
        all_nodes(&d.layers.root, &mut ids);
        let any = |r: &mut Rng| ids[r.below(ids.len() as u64) as usize];
        let full = ids.len() >= MAX_NODES;
        let name = match r.below(22) {
            0 if !full => { let before: HashSet<u64> = ids.iter().copied().collect(); let id = d.layers.add_layer("L"); assert!(!before.contains(&id), "id reused"); "add layer" }
            1 if !full => { d.layers.add_folder("F"); "add folder" }
            2 if !full => { d.layers.add_filter_layer(random_filter(r)); "add filter layer" }
            3 => { d.layers.remove_active(); "remove" }
            4 if !full => { d.layers.duplicate_active(); "duplicate" }
            5 => { d.layers.move_active(r.chance(0.5)); "move up / down" }
            6 => { let (a, b) = (any(r), any(r)); d.layers.move_node(a, b, [Place::Above, Place::Below, Place::Inside][r.below(3) as usize]); "move node" }
            7 => {
                let some: Vec<u64> = ids.iter().copied().filter(|_| r.chance(0.3)).collect();
                if let Some(&first) = some.first() { let t = any(r); d.layers.move_nodes(&some, first, t, [Place::Above, Place::Below, Place::Inside][r.below(3) as usize]); }
                "move several"
            }
            8 if !full => { d.layers.group_active("G"); "group" }
            9 if !full => { let some: Vec<u64> = ids.iter().copied().filter(|_| r.chance(0.3)).collect(); d.layers.group_nodes(&some, "G"); "group several" }
            10 => { d.layers.ungroup_active(); "ungroup" }
            11 | 12 => { let id = any(r); d.layers.set_active(id); "select" }
            13 => { let id = any(r); if let Some(n) = d.layers.node_mut(id) { *n.visible_mut() ^= true; } "visibility" }
            14 => { let id = any(r); if let Some(n) = d.layers.node_mut(id) { *n.locked_mut() ^= true; } "lock" }
            15 => { let v = if r.chance(0.2) { 0.0 } else { r.f() }; *d.layers.active_node_mut().opacity_mut() = v; "opacity" }
            16 => {
                let m = BlendMode::ALL[r.below(BlendMode::ALL.len() as u64) as usize];
                match d.layers.active_node_mut() {
                    Node::Layer(l) => l.blend = m,
                    Node::Folder(f) => f.blend = if r.chance(0.4) { FolderBlend::PassThrough } else { FolderBlend::Isolated(m) },
                    Node::Filter(f) => f.filter = random_filter(r),
                }
                "blend / filter"
            }
            17 => { let id = any(r); if let Some(n) = d.layers.node_mut(id) { n.props_mut().clip ^= true; } "clip" }
            18 => { let id = any(r); if let Some(n) = d.layers.node_mut(id) { n.props_mut().draft ^= true; } "draft" }
            19 => { let id = any(r); let on = !d.layers.node(id).unwrap().props().reference; d.layers.set_reference(id, on); "reference" }
            20 => { let id = any(r); if let Some(n) = d.layers.node_mut(id) { n.props_mut().label = r.below(9) as u8; *n.name_mut() = format!("n{}", r.below(99)); } "label / name" }
            _ => { if let Node::Folder(f) = d.layers.active_node_mut() { f.collapsed ^= true; } "fold" }
        };
        // Recorded as the app does once the edit is over, or later with
        // the next ones.
        if r.chance(0.7) { d.record_tree("pure"); }
        name
    }

    /// Undo n, redo them: the document as it was.
    fn undo_redo_check(&mut self) {
        let n = 1 + self.r.below(6);
        let mut d = self.doc.lock().unwrap();
        // (A layer edit not recorded yet is recorded by the undo: it counts.)
        let before = print(&d.layers);
        // (After the undo has recorded it.)
        let mut redo_before = None;
        let mut done = 0;
        for _ in 0..n {
            let ok = d.undo().is_some();
            // What could be redone before (a recorded edit empties it).
            if redo_before.is_none() { redo_before = Some(d.history().redo_len() - usize::from(ok)); }
            if ok { done += 1; }
        }
        check(&d);
        let mut back = 0;
        for _ in 0..done { if d.redo().is_some() { back += 1; } }
        assert_eq!(back, done, "undid {done} steps, redid {back}");
        assert_eq!(Some(d.history().redo_len()), redo_before, "not the same redo steps as before");
        let after = print(&d.layers);
        if before != after {
            let what = if before.tree != after.tree { "tree" } else if before.visible != after.visible { "visibility" } else if before.pixels != after.pixels { "pixels" } else { "size" };
            panic!("undo {done} then redo {back}: the {what} differs\nbefore {before:?}\nafter {after:?}");
        }
        *self.checks.entry("undo n + redo n = same").or_default() += 1;
    }

    fn merge_check(&mut self) {
        let d = self.doc.lock().unwrap();
        let l = &d.layers;
        let (w, h) = (l.width, l.height);
        let whole = l.composite_rect((0, 0, w, h));
        assert_eq!(whole.len(), (w * h * 4) as usize);
        // (Color dodge / burn divide by 1 - color: the plain merge's 8-bit
        // steps between layers grow through them; not comparable.)
        fn divides(nodes: &[Node]) -> bool {
            nodes.iter().any(|n| match n {
                Node::Layer(l) => matches!(l.blend, BlendMode::ColorDodge | BlendMode::ColorBurn),
                Node::Folder(f) => matches!(f.blend, FolderBlend::Isolated(BlendMode::ColorDodge | BlendMode::ColorBurn)) || divides(&f.children),
                Node::Filter(_) => false,
            })
        }
        if !l.needs_general_merge() && !divides(&l.root) {
            let g = l.composite_pick((0, 0, w, h), Pick { skip: u64::MAX, ..Pick::ALL }, false);
            let worst = whole.iter().zip(&g).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            // The plain merge rounds to 8 bits after each layer (Krita does
            // too in 8 bits): up to half a level per layer, where nearly
            // transparent layers pile up. The general one keeps floats.
            let layers = l.layer_ids().len() as u8;
            let worst_ok = 1 + layers.div_ceil(2);
            let e = self.checks.entry("max plain/general gap (levels)").or_default();
            *e = (*e).max(worst as u64);
            if worst > worst_ok {
                let i = whole.iter().zip(&g).position(|(a, b)| a.abs_diff(*b) == worst).unwrap() / 4;
                let (x, y) = (i as u32 % w, i as u32 / w);
                let mut tree = String::new();
                fn describe(nodes: &[Node], x: u32, y: u32, depth: usize, out: &mut String) {
                    for n in nodes.iter().rev() {
                        let px = match n { Node::Layer(l) => format!("{:?}", l.tiles.pixel(x, y)), _ => String::new() };
                        let kind = match n { Node::Layer(l) => format!("{:?}", l.blend), Node::Folder(f) => format!("{:?}", f.blend), Node::Filter(f) => format!("{:?}", f.filter.kind) };
                        out.push_str(&format!("{}{} vis {} op {:.3} {kind} {:?} {px}\n", "  ".repeat(depth), n.id(), n.visible(), n.opacity(), n.props()));
                        if let Node::Folder(f) = n { describe(&f.children, x, y, depth + 1, out); }
                    }
                }
                describe(&l.root, x, y, 0, &mut tree);
                panic!("plain and general merge differ by {worst} at ({x}, {y}): {:?} vs {:?}\n{tree}", &whole[i * 4..i * 4 + 4], &g[i * 4..i * 4 + 4]);
            }
            *self.checks.entry("plain merge = general merge").or_default() += 1;
        }
        // A part of the merge is that part of the whole.
        let r = random_rect(&mut self.r, w, h);
        let part = l.composite_rect(r);
        for y in r.1..r.3 {
            let a = &whole[((y * w + r.0) * 4) as usize..((y * w + r.2) * 4) as usize];
            let b = &part[((y - r.1) * (r.2 - r.0) * 4) as usize..((y - r.1 + 1) * (r.2 - r.0) * 4) as usize];
            if a != b {
                let x = a.chunks(4).zip(b.chunks(4)).position(|(p, q)| p != q).unwrap() as u32 + r.0;
                panic!("part {r:?} of the merge differs at ({x}, {y}) on a {w}x{h} canvas, general merge {}, margin {}", l.needs_general_merge(), l.merge_margin());
            }
        }
        *self.checks.entry("part of merge = merge of part").or_default() += 1;
        // Bands cover it once.
        let bands = l.merge_bands((0, 0, w, h), 1 + self.r.below(70) as u32);
        let mut y = 0;
        for b in &bands { assert_eq!((b.0, b.1, b.2), (0, y, w)); y = b.3; }
        assert_eq!(y, h);
        // Any pick.
        let ids = l.layer_ids();
        let pick = Pick {
            skip_drafts: self.r.chance(0.5),
            reference: self.r.chance(0.5).then(|| ids[self.r.below(ids.len() as u64) as usize]),
            skip: if self.r.chance(0.3) { ids[self.r.below(ids.len() as u64) as usize] } else { 0 },
            until: if self.r.chance(0.3) { ids[self.r.below(ids.len() as u64) as usize] } else { 0 },
        };
        assert_eq!(l.composite_pick(r, pick, self.r.chance(0.5)).len(), ((r.2 - r.0) * (r.3 - r.1) * 4) as usize);
    }

    /// The paint thread's whole merge, as the screen gets it, is the merge.
    fn screen_check(&mut self) {
        self.proj.lock().unwrap().take_patches();
        self.pt.send(Cmd::Recomposite(Arc::clone(&self.doc)));
        self.sync();
        let d = self.doc.lock().unwrap();
        let (w, h) = (d.layers.width, d.layers.height);
        let mut screen = vec![0u8; (w * h * 4) as usize];
        let mut covered = vec![false; (w * h) as usize];
        for (r, rgba) in self.proj.lock().unwrap().take_patches() {
            assert!(r.2 <= w && r.3 <= h, "piece {r:?} past the {w}x{h} canvas");
            assert_eq!(rgba.len(), ((r.2 - r.0) * (r.3 - r.1) * 4) as usize);
            for y in r.1..r.3 {
                let src = &rgba[((y - r.1) * (r.2 - r.0) * 4) as usize..][..((r.2 - r.0) * 4) as usize];
                screen[((y * w + r.0) * 4) as usize..((y * w + r.2) * 4) as usize].copy_from_slice(src);
                for x in r.0..r.2 { covered[(y * w + x) as usize] = true; }
            }
        }
        assert!(covered.iter().all(|c| *c), "the whole merge left holes on screen");
        assert!(screen == d.layers.composite_rect((0, 0, w, h)), "the screen differs from the merge");
        *self.checks.entry("screen = merge").or_default() += 1;
    }

    fn file_check(&mut self) {
        let back = {
            let d = self.doc.lock().unwrap();
            let mut buf = std::io::Cursor::new(Vec::new());
            aurastudio_file::write(&aurastudio_file::snapshot(&d, None), &mut buf).unwrap();
            buf.set_position(0);
            let back = aurastudio_file::read(buf, DocId(1), "back", 1 << 15).expect("a saved file must open").doc;
            assert_eq!(print(&back.layers), print(&d.layers), "the file gives another document");
            assert_eq!(back.layers.active_id(), d.layers.active_id());
            assert_eq!(back.history().redo_len(), d.history().export(usize::MAX).1.len());
            back
        };
        *self.checks.entry("file = document").or_default() += 1;
        // Go on with the opened one (its history read from the file).
        if self.r.chance(0.3) { self.doc = Arc::new(Mutex::new(back)); }
    }

    fn op(&mut self) -> &'static str {
        let k = self.r.below(1000);
        match k {
            0..=279 => self.layers_op(),
            280..=409 => { self.stroke(); "stroke" }
            410..=469 => {
                let mut d = self.doc.lock().unwrap();
                let (w, h) = (d.layers.width, d.layers.height);
                let (x, y) = (self.r.below(w as u64) as u32, self.r.below(h as u64) as u32);
                let color = [self.r.below(256) as u8, self.r.below(256) as u8, self.r.below(256) as u8, self.r.below(256) as u8];
                let p = random_fill(&mut self.r);
                d.fill(x, y, color, p);
                "fill"
            }
            470..=499 => { self.doc.lock().unwrap().clear_active(); "clear" }
            500..=559 => { let mut d = self.doc.lock().unwrap(); let (w, h) = (d.layers.width, d.layers.height); d.selection = random_selection(&mut self.r, w, h); "selection" }
            560..=599 => {
                let mut d = self.doc.lock().unwrap();
                if d.layers.layer_count() < MAX_NODES / 2 {
                    if let Some(mut clip) = d.copy() {
                        clip.x += self.r.range(-1.5, 1.5) as i64 * d.layers.width as i64 / 2;
                        clip.y += self.r.range(-1.5, 1.5) as i64 * d.layers.height as i64 / 2;
                        d.paste(&clip);
                    }
                }
                "copy / paste"
            }
            600..=649 => {
                let mut d = self.doc.lock().unwrap();
                for _ in 0..1 + self.r.below(3) {
                    let f = self.r.chance(0.9).then(|| random_filter(&mut self.r));
                    d.filter_preview(f.as_ref());
                }
                if self.r.chance(0.5) { let f = random_filter(&mut self.r); d.filter_apply(&f); } else { d.filter_cancel(); }
                "filter"
            }
            650..=759 => {
                let mut d = self.doc.lock().unwrap();
                for _ in 0..1 + self.r.below(4) { if self.r.chance(0.6) { d.undo(); } else { d.redo(); } }
                "undo / redo"
            }
            760..=799 => { self.undo_redo_check(); "check: undo + redo" }
            800..=839 => { self.merge_check(); "check: merges" }
            840..=854 => { self.screen_check(); "check: screen" }
            855..=864 => { self.file_check(); "check: file" }
            865..=869 => {
                let (w, h) = canvas_size(&mut self.r);
                self.doc.lock().unwrap().resize(w, h);
                *self.proj.lock().unwrap() = Projection::new(w, h);
                "resize"
            }
            870..=871 => { self.new_doc(); "new document" }
            _ => {
                // Copy / paste / undo through the paint thread's queue.
                let doc = Arc::clone(&self.doc);
                match self.r.below(4) {
                    0 => self.pt.send(Cmd::Undo(doc)),
                    1 => self.pt.send(Cmd::Redo(doc)),
                    2 => self.pt.send(Cmd::Clear(doc)),
                    _ => {
                        let (w, h) = { let d = self.doc.lock().unwrap(); (d.layers.width, d.layers.height) };
                        let (x, y) = (self.r.below(w as u64) as u32, self.r.below(h as u64) as u32);
                        let params = random_fill(&mut self.r);
                        self.pt.send(Cmd::Fill { doc, x, y, color: [9, 99, 199, 255], params });
                    }
                }
                self.sync();
                "paint thread command"
            }
        }
    }
}

#[test]
#[ignore]
fn pure_fuzz() {
    let total = env("PURE_OPS", 100_000);
    let seed = env("PURE_SEED", 1);
    let progress = env("PURE_PROGRESS", 100_000);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| { PANICKED.store(true, Ordering::SeqCst); default_hook(info); }));

    let proj: SharedProjection = Arc::new(Mutex::new(Projection::new(1, 1)));
    let pt = PaintThread::spawn(Arc::clone(&proj), |_| {});
    let mut run = Run {
        r: Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
        lib: Library::load(),
        doc: Arc::new(Mutex::new(Document::new(DocId(1), "pure", 1, 1))),
        pt, proj, t: 0.0,
        ops: BTreeMap::new(), checks: BTreeMap::new(), slow: Vec::new(),
    };
    run.new_doc();
    let t0 = Instant::now();
    for i in 1..=total {
        let size = { let d = run.doc.lock().unwrap(); (d.layers.width, d.layers.height) };
        let t = Instant::now();
        let name = run.op();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms > 200.0 {
            run.slow.push((ms, name, size));
            run.slow.sort_by(|a, b| b.0.total_cmp(&a.0));
            run.slow.truncate(10);
        }
        *run.ops.entry(name).or_default() += 1;
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&run.doc.lock().unwrap()))) {
            panic!("after operation {i} ({name}): {:?}", e.downcast_ref::<String>().map(String::as_str).or(e.downcast_ref::<&str>().copied()));
        }
        assert!(!PANICKED.load(Ordering::SeqCst), "a panic happened at operation {i} (see above)");
        // Keep it small: a new document when it grew big.
        let big = { let d = run.doc.lock().unwrap(); d.layers.layer_count() as u64 * d.layers.width as u64 * d.layers.height as u64 > 6_000_000 };
        if big { run.new_doc(); }
        run.proj.lock().unwrap().take_patches();
        if i % progress == 0 {
            println!("{i} operations, {:.0} s, {:.0} per second, {} MB", t0.elapsed().as_secs_f64(), i as f64 / t0.elapsed().as_secs_f64(), rss_mb());
        }
    }
    run.sync();
    println!("\npure fuzz, seed {seed}: {total} operations in {:.0} s, no panic", t0.elapsed().as_secs_f64());
    println!("operations:");
    for (k, v) in &run.ops { println!("  {k:<22} {v}"); }
    println!("checks passed:");
    for (k, v) in &run.checks { println!("  {k:<30} {v}"); }
    println!("slowest operations:");
    for (ms, name, size) in &run.slow { println!("  {ms:>8.0} ms  {name} on {}x{}", size.0, size.1); }
    assert!(!PANICKED.load(Ordering::SeqCst));
}

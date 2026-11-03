use aurastudio_core::{Rect, Rgba8};
use aurastudio_raster::{Image, Mask};

use crate::fill::{flood, similar_all};
use crate::*;

fn stack(w: u32, h: u32) -> LayerStack { LayerStack::new(w, h, 32, Start::Background) }
fn doc(w: u32, h: u32) -> LayerStack { LayerStack::new(w, h, 32, Start::Paper) }
fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect { Rect::new(x0, y0, x1, y1) }

fn paint(s: &mut LayerStack, name: &str, rect: Rect, c: Rgba8) -> u64 {
    let id = s.add_layer(name);
    assert!(s.set_layer_rect(id, rect, &vec![c; rect.area() as usize]));
    id
}

fn names_top_down(s: &LayerStack) -> Vec<String> {
    s.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), s.node(r.id).unwrap().name())).collect()
}

fn open(s: &mut LayerStack, id: u64) { if let Some(Node::Folder(f)) = s.node_mut(id) { f.collapsed = false; } }

#[test]
fn starts() {
    let s = doc(2, 2);
    assert_eq!(names_top_down(&s), ["Layer 1", "Paper"]);
    assert!(s.can_paint());
    let paper = s.rows().last().unwrap().id;
    assert!(s.is_locked(paper));
    assert_eq!(stack(2, 2).layer_count(), 1);
    let mut img = Image::new(4, 4, 32, [0; 4]);
    img.fill_rect(r(0, 0, 4, 4), [9, 9, 9, 255]);
    let p = LayerStack::new(4, 4, 32, Start::Image { name: "photo".into(), surface: img });
    assert_eq!(names_top_down(&p), ["Layer 1", "photo"]);
}

#[test]
fn footprints() {
    let mut s = stack(200, 150);
    let a = paint(&mut s, "a", r(10, 10, 40, 30), [255, 0, 0, 200]);
    let base = paint(&mut s, "base", r(100, 50, 140, 90), [0, 0, 255, 255]);
    let clipped = paint(&mut s, "clipped", r(0, 0, 200, 150), [0, 255, 0, 255]);
    s.node_mut(clipped).unwrap().props_mut().clip = true;
    let empty = s.add_layer("empty");
    assert_eq!(s.footprint(empty), Footprint::Nothing);
    assert_eq!(s.footprint(1), Footprint::All, "the background has a fill");
    assert!(matches!(s.footprint(a), Footprint::Area(x) if x.contains_rect(&r(10, 10, 40, 30))));
    // Removing the base: the clipped layer would clip to `a`.
    assert!(matches!(s.footprint(base), Footprint::Area(x) if x.contains_rect(&r(0, 0, 200, 150))));
    let f = s.add_filter_layer(aurastudio_filters::Filter::new(aurastudio_filters::Kind::Invert));
    assert_eq!(s.footprint(f), Footprint::All);
}

#[test]
fn drag_and_drop_moves_nodes() {
    let mut s = stack(8, 8);
    let a = s.add_layer("A");
    let f = s.add_folder("F");
    let b = s.add_layer("B"); // inside F
    let ids = |s: &LayerStack| s.rows().iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(&s), vec![f, b, a, 1]);
    assert!(s.move_node(a, f, Place::Inside));
    assert_eq!(ids(&s), vec![f, a, b, 1]);
    assert!(s.move_node(a, 1, Place::Below));
    assert_eq!(ids(&s), vec![f, b, 1, a]);
    assert!(s.move_node(a, f, Place::Above));
    assert_eq!(ids(&s), vec![a, f, b, 1]);
    assert!(!s.move_node(f, b, Place::Above), "a folder into itself");
    assert_eq!(s.active_id(), a);
}

#[test]
fn new_folders_start_folded_and_new_layers_go_in_the_active_folder() {
    let mut s = stack(2, 2);
    let f = s.add_folder("F");
    assert!(matches!(s.node(f).unwrap(), Node::Folder(x) if x.collapsed));
    s.add_layer("A");
    s.add_layer("B");
    assert_eq!(names_top_down(&s), ["F", "  B", "  A", "Background"]);
}

#[test]
fn move_enters_and_leaves_folders() {
    let mut s = stack(2, 2);
    s.add_folder("F");
    s.set_active(1);
    let a = s.add_layer("A");
    assert_eq!(names_top_down(&s), ["F", "A", "Background"]);
    s.move_active(true);
    assert_eq!(names_top_down(&s), ["F", "  A", "Background"]);
    s.move_active(true);
    assert_eq!(names_top_down(&s), ["A", "F", "Background"]);
    s.move_active(false);
    assert_eq!(names_top_down(&s), ["F", "  A", "Background"]);
    s.move_active(false);
    assert_eq!(names_top_down(&s), ["F", "A", "Background"]);
    assert_eq!(s.active_id(), a);
}

#[test]
fn locks_block_painting_and_clearing() {
    let mut s = stack(2, 2);
    let f = s.add_folder("F");
    let l = paint(&mut s, "L", r(0, 0, 2, 2), [0, 0, 0, 255]);
    assert!(s.can_paint());
    *s.node_mut(f).unwrap().locked_mut() = true;
    assert!(!s.can_paint() && !s.clear_active());
    assert_eq!(s.image(l).unwrap().pixel(0, 0), [0, 0, 0, 255]);
    *s.node_mut(f).unwrap().locked_mut() = false;
    *s.node_mut(l).unwrap().locked_mut() = true;
    assert!(!s.can_paint());
    // Folders aren't painted on.
    let g = s.add_folder("G");
    assert!(!s.can_paint() && s.begin_stroke(g, None).is_none());
}

#[test]
fn duplicate_copies_content_with_new_ids_and_drops_marks() {
    let mut s = stack(2, 2);
    let f = s.add_folder("F");
    paint(&mut s, "L", r(0, 0, 1, 1), [1, 2, 3, 255]);
    s.set_active(f);
    s.duplicate_active();
    assert_eq!(names_top_down(&s), ["F copy", "  L", "F", "  L", "Background"]);
    let ids = s.layer_ids();
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());
    let copy_l = s.rows()[1].id;
    assert_eq!(s.image(copy_l).unwrap().pixel(0, 0), [1, 2, 3, 255]);
    let mut t = doc(4, 4);
    let a = t.active_id();
    let p = t.node_mut(a).unwrap().props_mut();
    (p.reference, p.draft, p.clip, p.label) = (true, true, true, 3);
    let c = t.duplicate_active();
    let p = t.node(c).unwrap().props();
    assert!(!p.reference && !p.draft && p.clip && p.label == 3);
    // A copy of the Paper is a plain layer.
    t.set_active(t.rows().last().unwrap().id);
    let pc = t.duplicate_active();
    assert!(!t.layer(pc).unwrap().background);
}

#[test]
fn remove_keeps_a_valid_active_and_at_least_one_layer() {
    let mut s = stack(2, 2);
    assert!(!s.remove_active(), "the last paint layer");
    let f = s.add_folder("F");
    s.add_layer("L");
    s.set_active(f);
    assert!(s.remove_active());
    assert_eq!(s.take_removed().len(), 1);
    let a = s.add_layer("A");
    *s.node_mut(a).unwrap().locked_mut() = true;
    assert_eq!(s.removal_refused(), Some("Locked: unlock it first"));
    assert!(!s.remove_active());
    *s.node_mut(a).unwrap().locked_mut() = false;
    assert!(s.remove_active());
    assert_eq!(names_top_down(&s), ["Background"]);
    assert_eq!(s.active_id(), 1);
}

#[test]
fn clear_resets_to_white_or_transparent() {
    let mut s = stack(2, 2);
    s.set_layer_rect(1, r(0, 0, 2, 2), &[[0, 0, 0, 255]; 4]);
    s.clear_active();
    assert_eq!(s.image(1).unwrap().pixel(0, 0), [255; 4]);
    let l = paint(&mut s, "L", r(0, 0, 2, 2), [0, 0, 0, 255]);
    s.clear_active();
    assert_eq!(s.image(l).unwrap().pixel(0, 0), [255, 255, 255, 0]);
    // A soft selection clears partly.
    let l2 = paint(&mut s, "M", r(0, 0, 2, 1), [0, 0, 0, 255]);
    let mut m = Mask::new(2, 2, 32, 0);
    m.write(r(0, 0, 2, 1), &[255, 128]);
    assert_eq!(s.clear_selected(l2, &m), Some(r(0, 0, 2, 1)));
    let im = s.image(l2).unwrap();
    assert_eq!(im.pixel(0, 0)[3], 0);
    assert!((120..=135).contains(&im.pixel(1, 0)[3]));
}

#[test]
fn strokes_go_through_raster_and_undo_by_patch() {
    use aurastudio_core::PaintSurface;
    let mut s = stack(256, 256);
    let id = s.add_layer("ink");
    let before = s.image(id).unwrap().clone();
    let mut patch = {
        let mut st = s.begin_stroke(id, None).unwrap();
        st.prepare(r(0, 0, 10, 10));
        let sp = st.span_mut(3, 3, 1);
        sp.pixels[0] = [255, 0, 0, 255];
        sp.mask[0] = 255;
        st.end()
    };
    assert_eq!(s.image(id).unwrap().pixel(3, 3), [255, 0, 0, 255]);
    assert!(s.swap_patch(id, &mut patch));
    assert_eq!(*s.image(id).unwrap(), before);
    assert!(!s.swap_patch(999, &mut patch));
}

#[test]
fn resize_keeps_content_centered_and_nothing_is_cut() {
    let mut s = stack(4, 4);
    s.add_folder("F");
    let l = paint(&mut s, "L", r(1, 1, 2, 2), [0, 0, 0, 255]);
    s.resize(8, 8);
    assert_eq!(s.image(l).unwrap().pixel(3, 3), [0, 0, 0, 255]);
    assert_eq!(s.image(l).unwrap().pixel(0, 0), [255, 255, 255, 0]);
    assert_eq!(s.image(1).unwrap().pixel(0, 0), [255; 4]);
    s.resize(2, 2);
    assert_eq!(s.image(l).unwrap().pixel(0, 0), [0, 0, 0, 255], "moved, not cut");
}

#[test]
fn group_ungroup_and_clipped_rows() {
    let mut s = doc(64, 64);
    let a = s.active_id();
    let b = s.add_layer("b");
    s.node_mut(b).unwrap().props_mut().clip = true;
    let rows = s.rows();
    assert!(rows.iter().find(|r| r.id == b).unwrap().clipped);
    assert!(!rows.iter().find(|r| r.id == a).unwrap().clipped);
    s.set_active(b);
    let g = s.group_active("g");
    open(&mut s, g);
    assert_eq!(s.active_id(), g);
    assert!(!s.rows().iter().find(|r| r.id == b).unwrap().clipped);
    assert_eq!(s.clip_base(b), None);
    s.node_mut(g).unwrap().props_mut().reference = true;
    assert!(s.rows().iter().find(|r| r.id == b).unwrap().reference_by_parent);
    assert!(s.ungroup_active());
    assert_eq!(s.active_id(), b);
    assert!(s.node(g).is_none());
    assert_eq!(s.clip_base(b), Some(a));
    assert!(!s.ungroup_active(), "not a folder");
    let f = s.add_filter_layer(aurastudio_filters::Filter::new(aurastudio_filters::Kind::Invert));
    assert!(!s.can_paint() && s.layer_rect(f, r(0, 0, 1, 1)).is_none());
    let d = s.duplicate_active();
    assert!(matches!(s.node(d), Some(Node::Filter(x)) if x.filter.kind == aurastudio_filters::Kind::Invert));
    assert!(!s.layer_ids().contains(&f));
    // A selection mask: a row, no pixels drawn, not paintable.
    let m = s.add_mask("sel", Mask::new(64, 64, 32, 0));
    assert!(s.rows().iter().any(|r| r.id == m && r.mask));
    assert_eq!(s.footprint(m), Footprint::Nothing);
    assert!(!s.can_paint());
}

#[test]
fn one_reference_per_folder_and_each_folder_its_own() {
    let mut s = doc(32, 32);
    let ink = s.active_id();
    let color = s.add_layer("color");
    let other = s.add_layer("other");
    s.set_reference(ink, true);
    s.set_reference(other, true);
    assert!(!s.node(ink).unwrap().props().reference, "one per folder");
    assert_eq!(s.reference_for(color), Some(other));
    assert_eq!(s.reference_for(other), None, "never itself");
    let g = s.group_nodes(&[ink, color], "g").unwrap();
    s.set_reference(ink, true);
    assert!(s.node(other).unwrap().props().reference, "another folder: both stay");
    assert_eq!(s.reference_for(color), Some(ink));
    assert_eq!(s.reference_for(other), None);
    assert_eq!(s.reference_for(g), Some(other));
    s.set_reference(ink, false);
    assert_eq!(s.reference_for(color), Some(other));
    *s.node_mut(other).unwrap().visible_mut() = false;
    assert_eq!(s.reference_for(color), None);
    *s.node_mut(other).unwrap().visible_mut() = true;
    s.set_reference(ink, true);
    assert!(s.move_node(other, color, Place::Above));
    assert!(s.node(other).unwrap().props().reference && !s.node(ink).unwrap().props().reference);
    let mut t = doc(16, 16);
    let a = t.active_id();
    t.set_reference(a, true);
    let f = t.add_folder("f");
    let b = t.add_layer("b");
    t.set_reference(b, true);
    assert!(t.move_node(f, a, Place::Below));
    t.set_active(a);
    t.move_active(false);
    assert_eq!(t.rows().iter().filter(|r| t.node(r.id).unwrap().props().reference).count(), 1);
    assert!(t.node(a).unwrap().props().reference, "the one moved stays one");
}

#[test]
fn several_layers_group_and_move_together() {
    let mut s = doc(32, 32);
    let one = s.active_id();
    let two = s.add_layer("two");
    let three = s.add_layer("three");
    let names = |s: &LayerStack| s.rows().iter().map(|r| s.node(r.id).unwrap().name().to_owned()).collect::<Vec<_>>();
    let g = s.group_nodes(&[one, three], "g").unwrap();
    open(&mut s, g);
    assert_eq!(names(&s), ["g", "three", "Layer 1", "two", "Paper"]);
    assert_eq!(s.active_id(), g);
    assert!(!s.move_nodes(&[one, three], one, three, Place::Below));
    assert!(s.move_nodes(&[three, one], one, two, Place::Below));
    assert_eq!(names(&s), ["g", "two", "three", "Layer 1", "Paper"]);
    assert_eq!(s.active_id(), one);
    let f = s.group_nodes(&[two], "f").unwrap();
    open(&mut s, f);
    assert!(s.move_nodes(&[f, two], f, 1, Place::Below));
    assert_eq!(names(&s), ["g", "three", "Layer 1", "Paper", "f", "two"]);
}

#[test]
fn tree_undo_keeps_the_pixels() {
    let mut s = doc(16, 16);
    let a = s.active_id();
    s.set_layer_rect(a, r(0, 0, 4, 4), &[[9, 9, 9, 255]; 16]);
    let before = s.tree_state();
    let f = s.group_active("g");
    s.set_active(f);
    s.duplicate_active();
    let after = s.tree_state();
    assert_ne!(before, after);
    let left = s.restore_tree(&before, Vec::new());
    assert_eq!(s.tree_state(), before);
    assert_eq!(s.image(a).unwrap().pixel(1, 1), [9, 9, 9, 255]);
    let back = s.restore_tree(&after, left);
    assert!(back.is_empty());
    assert_eq!(s.tree_state(), after);
}

#[test]
fn from_parts_checks() {
    let s = doc(8, 8);
    let ok = LayerStack::from_parts(8, 8, 32, s.root.clone(), s.active_id()).unwrap();
    assert_eq!(ok.active_id(), s.active_id());
    let mut dup = s.root.clone();
    dup.push(dup[0].clone());
    assert!(LayerStack::from_parts(8, 8, 32, dup, 1).is_err());
    assert!(LayerStack::from_parts(8, 8, 32, Vec::new(), 1).is_err());
    assert!(LayerStack::from_parts(9, 8, 32, s.root.clone(), 1).is_err(), "another frame");
}

#[test]
fn add_take_and_place_nodes() {
    let mut s = stack(8, 8);
    let f = s.add_folder("F");
    let l = s.add_layer("L");
    assert_eq!(s.parent_of(l), Some(f));
    assert_eq!(s.place_of(f), Some((0, 1)));
    let n = s.take_node(l).unwrap();
    assert!(s.node(l).is_none() && s.node(s.active_id()).is_some());
    assert!(s.add_node(n, f, 0));
    assert_eq!(s.parent_of(l), Some(f));
    assert!(!s.add_node(s.node(1).unwrap().clone(), l, 0), "a layer holds nothing");
    let mut count = 0;
    s.walk(|_, _| count += 1);
    assert_eq!(count, 3);
}

/// A plain Normal merge of the layers, for the fill tests (the real one
/// is `compose`'s).
struct Merged<'a>(&'a LayerStack, bool);

impl FillSource for Merged<'_> {
    fn pixels(&self, rect: Rect) -> Vec<Rgba8> {
        let mut out = vec![[255u8; 4]; rect.area() as usize];
        self.0.walk(|n, _| if let Node::Layer(l) = n {
            if !l.visible || (self.1 && l.props.draft) { return; }
            for (o, p) in out.iter_mut().zip(l.surface.read(rect)) {
                if p[3] == 255 { *o = p; }
            }
        });
        out
    }
}

#[test]
fn fill_refers_to_what_it_is_given() {
    let mut s = stack(64, 64);
    let ink = paint(&mut s, "ink", r(32, 0, 34, 64), [0, 0, 0, 255]);
    let sketch = paint(&mut s, "sketch", r(16, 0, 17, 64), [0, 0, 255, 255]);
    s.node_mut(sketch).unwrap().props_mut().draft = true;
    let frame = s.frame();
    // Drafts skipped: the ink stops it, not the sketch.
    let a = flood(&Merged(&s, true), frame, 5, 5, 10, 0, None).unwrap();
    assert!(a.contains(20, 5) && !a.contains(40, 5));
    assert_eq!(a.bbox(), r(0, 0, 32, 64));
    // Drafts counted: the sketch stops it too.
    let a = flood(&Merged(&s, false), frame, 5, 5, 10, 0, None).unwrap();
    assert!(!a.contains(20, 5));
    // The ink layer alone (transparent around: one color everywhere).
    let a = flood(s.image(ink).unwrap(), frame, 5, 5, 10, 1, None).unwrap();
    assert!(a.contains(31, 5) && a.contains(32, 5), "grown under the line");
    assert!(!a.contains(40, 5));
    // Inside a selection.
    let mut m = Mask::new(64, 64, 32, 0);
    m.fill_rect(r(0, 0, 10, 10), 255);
    let a = flood(&Merged(&s, true), frame, 5, 5, 10, 3, Some(&m)).unwrap();
    assert!(a.contains(9, 9) && !a.contains(12, 5), "the expansion stays in it");
    assert!(flood(&Merged(&s, true), frame, 20, 20, 10, 0, Some(&m)).is_none());
    // Paint it.
    let c = s.add_layer("color");
    let a = flood(&Merged(&s, true), frame, 5, 5, 10, 0, None).unwrap();
    assert_eq!(s.fill_area(c, &a, [200, 10, 10, 255], None, false), Some(a.bbox()));
    assert_eq!(s.image(c).unwrap().pixel(20, 5), [200, 10, 10, 255]);
    assert_eq!(s.image(c).unwrap().pixel(40, 5)[3], 0);
    // Everywhere alike (no contiguity): both sides of the ink.
    let all = similar_all(&Merged(&s, false), frame, 40, 5, 10, 0).unwrap();
    assert!(all.contains(50, 5) && !all.contains(32, 5));
    // References: the pick names the reference.
    s.node_mut(ink).unwrap().props_mut().reference = true;
    let p = FillParams { refer: Refer::References, ..FillParams::default() };
    assert_eq!(p.pick(&s, c).and_then(|k| k.reference), Some(ink));
    assert_eq!(FillParams { refer: Refer::Layer, ..p }.pick(&s, c), None);
}

/// The scanline fill against a plain one, on random walls.
#[test]
fn flood_matches_a_plain_flood() {
    let mut rng = aurastudio_core::Rng::new(3);
    for _ in 0..20 {
        let (w, h) = (rng.below(150) as i32 + 5, rng.below(100) as i32 + 5);
        let mut img = Image::new(w as u32, h as u32, 16, [255, 255, 255, 0]);
        for _ in 0..rng.below(30) {
            let (x, y) = (rng.below(w as u64) as i32, rng.below(h as u64) as i32);
            img.fill_rect(Rect::new(x, y, x + rng.below(20) as i32 + 1, y + rng.below(3) as i32 + 1), [0, 0, 0, 255]);
        }
        let b = Rect::new(0, 0, w, h);
        let (sx, sy) = (rng.below(w as u64) as i32, rng.below(h as u64) as i32);
        let got = flood(&img, b, sx, sy, 0, 0, None);
        // Plain 4-neighbour flood.
        let seed = img.pixel(sx, sy);
        let mut seen = vec![false; (w * h) as usize];
        let mut st = vec![(sx, sy)];
        while let Some((x, y)) = st.pop() {
            if x < 0 || y < 0 || x >= w || y >= h || seen[(y * w + x) as usize] || img.pixel(x, y) != seed { continue; }
            seen[(y * w + x) as usize] = true;
            st.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
        let a = got.unwrap();
        for y in 0..h { for x in 0..w { assert_eq!(a.contains(x, y), seen[(y * w + x) as usize], "{x},{y}"); } }
    }
}

#[test]
fn ghosts_report_where_they_change() {
    let mut s = stack(64, 64);
    let f = s.add_folder("sheetlike");
    let l = paint(&mut s, "drawing", r(10, 10, 20, 20), [0, 0, 0, 255]);
    let _ = l;
    let g = Ghost { sheet: f, tint: [1.0, 0.0, 0.0], opacity: 0.5 };
    let changed = s.set_ghosts(vec![(1, vec![g])]).unwrap();
    assert!(changed.contains_rect(&r(10, 10, 20, 20)));
    assert!(s.set_ghosts(vec![(1, vec![g])]).is_none(), "the same ghosts");
    assert_eq!(s.ghosts_of(1).unwrap().len(), 1);
    assert_eq!(g.tinted([0.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
    assert_eq!(g.tinted([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
}

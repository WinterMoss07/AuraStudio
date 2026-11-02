use aurastudio_compose::{merge, Opts};
use aurastudio_core::{Point, Rect, Rgba8, TransferCurve};
use aurastudio_layers::{Cel, LayerStack, Node, Onto, Place, Shift, Start, Target};

use crate::*;

fn doc(w: u32, h: u32) -> LayerStack { LayerStack::new(w, h, 32, Start::Paper) }
fn bare(w: u32, h: u32) -> LayerStack { LayerStack::new(w, h, 32, Start::Background) }
fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect { Rect::new(x0, y0, x1, y1) }
fn names(s: &LayerStack, sheet: u64) -> Vec<String> { s.node(sheet).unwrap().as_folder().unwrap().children.iter().rev().map(|c| c.name().to_owned()).collect() }
fn screen(s: &LayerStack, rect: Rect) -> Vec<Rgba8> { merge(s, rect, Opts { ghosts: true, ..Default::default() }) }

/// Make a sheet and settle, as the document does after each edit.
fn sheet(s: &mut LayerStack, t: &mut Timeline, folder: u64, n: u32) -> u64 { let c = s.make_sheet(folder, n).unwrap(); t.settle(s); c }

#[test]
fn sheets_follow_the_template_and_hold() {
    let mut s = doc(64, 64);
    let mut t = Timeline::default();
    let a = s.add_animation_folder("Perso A");
    assert_eq!(t.paint_target(&s), Some(Target::NewSheet { folder: a, frame: 1 }));
    assert_eq!(t.paint_id(&s), None);
    let cel = sheet(&mut s, &mut t, a, 1);
    let f1 = s.active_id();
    assert_eq!(s.node(f1).unwrap().name(), "Sheet 1");
    assert_eq!(names(&s, f1), ["Colo", "Ink", "Douga", "Genga"]);
    assert_eq!(s.node(cel).unwrap().name(), "Genga");
    assert_eq!(t.paint_target(&s), Some(Target::Layer(cel)));
    s.set_active(cel);
    assert_eq!(s.active_id(), f1, "a cel made active: its sheet is");
    t.set_frame(&mut s, 5);
    assert_eq!(t.exposed(&s, a), Some(f1), "held");
    assert_eq!(t.paint_target(&s), Some(Target::NewSheet { folder: a, frame: 5 }));
    s.set_cel(a, 2);
    let douga = sheet(&mut s, &mut t, a, 5);
    assert_eq!(s.node(douga).unwrap().name(), "Douga");
    let f5 = s.active_id();
    assert!(s.node(f5).unwrap().visible() && !s.node(f1).unwrap().visible());
    for k in [1, 4] { t.set_frame(&mut s, k); assert_eq!(t.exposed(&s, a), Some(f1)); }
    t.set_frame(&mut s, 9);
    assert_eq!(t.exposed(&s, a), Some(f5));
    assert_eq!(s.sheets(a), vec![(1, f1), (5, f5)]);
    s.check_animation(t.frame()).unwrap();
    assert!(s.make_sheet(a, 5).is_none(), "taken twice");
}

#[test]
fn sheets_stay_in_their_folder() {
    let mut s = doc(32, 32);
    let mut t = Timeline::default();
    let ink = s.active_id();
    let a = s.add_animation_folder("A");
    sheet(&mut s, &mut t, a, 1);
    let f1 = s.active_id();
    let l = s.add_layer("L");
    assert!(s.anim_of(l).is_none(), "a new layer with a sheet active: outside");
    assert!(!s.move_node(ink, a, Place::Inside));
    assert!(!s.move_node(ink, f1, Place::Above));
    assert!(!s.move_node(f1, ink, Place::Above));
    assert!(s.move_node(ink, a, Place::Above));
    s.set_active(f1);
    assert_eq!(s.group_active("G"), f1);
    assert_eq!(s.duplicate_active(), f1);
    t.settle(&mut s);
    s.check_animation(t.frame()).unwrap();
    s.set_active(f1);
    assert!(s.remove_active());
    t.settle(&mut s);
    assert!(s.sheets(a).is_empty());
    s.check_animation(t.frame()).unwrap();
}

/// Redoing only the rectangle the time changed gives the merge of the new
/// frame whole, ghosts included, whatever the scene.
#[test]
fn the_dirty_rect_of_time_is_enough() {
    let (w, h) = (192i32, 128i32);
    let mut rng = aurastudio_core::Rng::new(77);
    for scene in 0..10 {
        let mut s = bare(w as u32, h as u32);
        let mut t = Timeline::default();
        let a = s.add_animation_folder("A");
        let mut starts: Vec<u32> = (0..2 + rng.below(5)).map(|_| 1 + rng.below(14) as u32).collect();
        starts.sort_unstable();
        starts.dedup();
        for &n in &starts {
            let cel = sheet(&mut s, &mut t, a, n);
            for _ in 0..1 + rng.below(3) {
                let (x, y) = (rng.below(150) as i32, rng.below(90) as i32);
                let rect = r(x, y, x + 4 + rng.below(30) as i32, y + 4 + rng.below(30) as i32);
                let c = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8, 40 + rng.below(216) as u8];
                assert!(s.set_layer_rect(cel, rect, &vec![c; rect.area() as usize]));
            }
        }
        let b = s.add_layer("Back");
        s.set_layer_rect(b, r(120, 100, 130, 110), &[[0, 0, 255, 255]; 100]);
        t.look = OnionLook { by_frames: rng.chance(0.5), ..OnionLook::default() };
        let mut mobile = std::collections::BTreeMap::new();
        for off in [-3, -2, -1, 1, 2, 3] { if rng.chance(0.5) { mobile.insert(off, if rng.chance(0.3) { Some(0.5) } else { None }); } }
        let first = s.sheet_at(a, starts[0]).unwrap();
        s.set_active(first);
        t.set_onion(&mut s, Onion { enabled: !rng.chance(0.2), mobile, fixed: Default::default() });
        t.set_frame(&mut s, 1);
        t.take_dirty();
        let all = r(0, 0, w, h);
        let mut canvas = screen(&s, all);
        for _ in 0..30 {
            let k = 1 + rng.below(16) as u32;
            t.set_frame(&mut s, k);
            if let Some(d) = t.take_dirty() {
                let d = d.intersect(&all);
                let px = screen(&s, d);
                for y in d.y0..d.y1 {
                    for x in d.x0..d.x1 { canvas[(y * w + x) as usize] = px[((y - d.y0) * d.w() as i32 + x - d.x0) as usize]; }
                }
            }
            assert!(canvas == screen(&s, all), "scene {scene}, frame {k}");
        }
    }
}

#[test]
fn time_reports_when_the_merge_changes() {
    let mut s = doc(32, 32);
    let mut t = Timeline::default();
    assert!(!t.set_frame(&mut s, 5), "no animation folder: the canvas stays");
    let a = s.add_animation_folder("A");
    for n in [1, 4] { t.set_frame(&mut s, n); sheet(&mut s, &mut t, a, n); }
    assert!(t.set_frame(&mut s, 1));
    assert!(!t.set_frame(&mut s, 2) && !t.set_frame(&mut s, 3), "sheet 1 held");
    assert!(t.set_frame(&mut s, 4));
    assert!(!t.set_frame(&mut s, 9));
    t.take_dirty();
    assert!(!t.set_frame(&mut s, 9));
    assert_eq!(t.take_dirty(), None);
}

#[test]
fn ghosts_follow_the_marks() {
    let mut s = bare(80, 30);
    let mut t = Timeline::default();
    let folder = s.add_animation_folder("A");
    for (n, x) in [(1u32, 10), (4, 30), (6, 50)] {
        let cel = sheet(&mut s, &mut t, folder, n);
        s.set_layer_rect(cel, r(x, 10, x + 10, 20), &[[0, 0, 0, 255]; 100]);
    }
    t.set_frame(&mut s, 5);
    let f4 = s.sheet_at(folder, 4).unwrap();
    s.set_active(f4);
    t.settle(&mut s);
    let px = |s: &LayerStack, x: i32| screen(s, r(x, 15, x + 1, 16))[0];
    assert_eq!(px(&s, 15), [255; 4]);
    t.set_onion(&mut s, Onion { enabled: true, mobile: [(-1, None), (1, None)].into(), fixed: Default::default() });
    assert_eq!(px(&s, 15), [255, 120, 120, 255]);
    assert_eq!(px(&s, 55), [120, 255, 120, 255]);
    assert_eq!(px(&s, 35), [0, 0, 0, 255]);
    // By frames: −1 is frame 4, the current sheet: nothing; −2 and −3
    // both show sheet 1: once.
    t.set_look(&mut s, OnionLook { by_frames: true, ..OnionLook::default() });
    t.set_onion(&mut s, Onion { enabled: true, mobile: [(-1, None), (-2, None), (-3, None), (1, None)].into(), fixed: Default::default() });
    assert_eq!(s.ghosts()[0].1.len(), 2);
    // A fixed mark at full opacity.
    t.set_look(&mut s, OnionLook::default());
    t.set_onion(&mut s, Onion { enabled: true, mobile: Default::default(), fixed: [(2, Some(1.0))].into() });
    assert_eq!(px(&s, 15), [255, 0, 0, 255]);
    // Paused (playback): gone, marks kept.
    assert!(t.pause_onion(&mut s, true));
    assert_eq!(px(&s, 15), [255; 4]);
    assert_eq!(t.onion.fixed.len(), 1);
    assert!((OnionLook::default().fade(1) - 0.53).abs() < 1e-4);
}

#[test]
fn template_edits_reach_every_sheet() {
    let mut s = doc(32, 32);
    let mut t = Timeline::default();
    let a = s.add_animation_folder("A");
    let f: Vec<u64> = (1..=3).map(|n| { t.set_frame(&mut s, n * 2); sheet(&mut s, &mut t, a, n * 2); s.active_id() }).collect();
    s.set_cel(a, 0);
    s.add_cel(a, 1, Cel::new("Layout", false));
    s.rename_cel(a, 0, "Key");
    s.move_cel(a, 4, 0);
    t.settle(&mut s);
    for &fr in &f { assert_eq!(names(&s, fr), ["Genga", "Key", "Layout", "Ink", "Douga"]); }
    assert_eq!(s.anim(a).unwrap().template[s.anim(a).unwrap().cel].name, "Key");
    assert!(s.remove_cel(a, 2));
    for &fr in &f { assert_eq!(names(&s, fr), ["Genga", "Key", "Ink", "Douga"]); }
    assert_eq!(s.take_removed().len(), 3);
    s.check_animation(t.frame()).unwrap();
    let key = |s: &LayerStack, fr: u64| s.node(fr).unwrap().as_folder().unwrap().children[2].id();
    let k = key(&s, f[0]);
    *s.node_mut(k).unwrap().locked_mut() = true;
    s.apply_to_folder(f[0]);
    for &fr in &f { assert!(s.node(key(&s, fr)).unwrap().locked()); }
    t.set_frame(&mut s, 6);
    s.set_active(f[2]);
    s.set_cel(a, 1);
    assert_eq!(t.paint_target(&s), None, "locked");
    t.set_frame(&mut s, 7);
    assert_eq!(t.paint_target(&s), None, "a new sheet would start locked");
    s.set_cel(a, 2);
    assert!(matches!(t.paint_target(&s), Some(Target::NewSheet { frame: 7, .. })));
    for _ in 0..3 { assert!(s.remove_cel(a, 0)); }
    assert!(!s.remove_cel(a, 0));
    t.settle(&mut s);
    s.check_animation(t.frame()).unwrap();
}

#[test]
fn timing_moves_copies_and_shifts() {
    let mut s = doc(16, 16);
    let mut t = Timeline::default();
    let a = s.add_animation_folder("A");
    for n in [1, 3, 5] { sheet(&mut s, &mut t, a, n); }
    let frames = |s: &LayerStack| s.sheets(a).iter().map(|x| x.0).collect::<Vec<_>>();
    assert!(s.move_sheet(a, 3, 4, Onto::Refuse));
    assert_eq!(frames(&s), [1, 4, 5]);
    assert!(!s.move_sheet(a, 4, 5, Onto::Refuse));
    assert!(s.move_sheet(a, 4, 5, Onto::Swap));
    assert_eq!(frames(&s), [1, 4, 5]);
    assert!(s.copy_sheet(a, 1, 2, Onto::Refuse));
    assert_eq!(frames(&s), [1, 2, 4, 5]);
    assert!(s.move_sheets(a, &[4, 5], 2, false, Onto::Refuse));
    assert_eq!(frames(&s), [1, 2, 6, 7]);
    assert!(s.shift_frames(a, 2, Shift::Insert));
    assert_eq!(frames(&s), [1, 3, 7, 8]);
    assert!(s.shift_frames(a, 3, Shift::Remove));
    assert_eq!(frames(&s), [1, 6, 7]);
    assert!(s.delete_sheet(a, 6));
    assert_eq!(frames(&s), [1, 7]);
    t.settle(&mut s);
    s.check_animation(t.frame()).unwrap();
    assert_eq!(s.last_frame(), 7);
}

#[test]
fn groups_hold_animation_folders() {
    let mut s = doc(16, 16);
    let mut t = Timeline::default();
    let a = s.add_animation_folder("A");
    let b = s.add_animation_folder("B");
    let g = s.group_animation_folders(&[a, b], "G").unwrap();
    t.settle(&mut s);
    assert_eq!(s.group_members(g), vec![a, b]);
    assert_eq!(s.group_of(a), Some(g));
    assert_eq!(s.columns(None), vec![aurastudio_layers::Column::Group(g)]);
    let c = s.add_animation_folder_in("C", Some(g));
    assert_eq!(s.group_of(c), Some(g));
    let d = s.add_animation_folder_in("D", None);
    assert_eq!(s.group_of(d), None);
    // Emptied: the group goes at the settle.
    for id in [a, b, c] { s.set_active(id); assert!(s.remove_active()); }
    t.settle(&mut s);
    assert!(s.node(g).is_none());
    s.check_animation(t.frame()).unwrap();
    let mut n = doc(8, 8);
    n.start_animation();
    t.settle(&mut n);
    assert_eq!(n.animation_folders().len(), 1);
    assert_eq!(n.root.len(), 2);
    let _ = Node::is_animation;
}

#[test]
fn tree_undo_keeps_sheets_whole() {
    let mut s = doc(32, 32);
    let mut t = Timeline::default();
    let a = s.add_animation_folder("A");
    let before = s.tree_state();
    sheet(&mut s, &mut t, a, 1);
    s.set_cel(a, 2);
    let after = s.tree_state();
    assert_ne!(before, after);
    let kept = s.restore_tree(&before, Vec::new());
    t.settle(&mut s);
    assert!(s.sheets(a).is_empty());
    s.restore_tree(&after, kept);
    t.settle(&mut s);
    assert_eq!(s.sheets(a).len(), 1);
    assert_eq!(s.anim(a).unwrap().cel, 2);
    s.check_animation(t.frame()).unwrap();
    let ts = s.tree_state();
    s.set_cel(a, 0);
    assert_eq!(s.tree_state(), ts, "choosing a cel is no edit");
}

#[test]
fn the_camera_eases_between_keys() {
    let mut c = Camera::default();
    assert!(c.at(3.0).is_none());
    c.set_key(CameraKey { frame: 1, center: Point::new(0.0, 0.0), zoom: 1.0, angle: 0.0, ease: TransferCurve::linear() });
    c.set_key(CameraKey { frame: 11, center: Point::new(100.0, 0.0), zoom: 4.0, angle: 90.0, ease: TransferCurve::linear() });
    let (p, z, a) = c.at(6.0).unwrap();
    assert!((p.x - 50.0).abs() < 1e-3 && (z - 2.0).abs() < 1e-3 && (a - 45.0).abs() < 1e-3, "{p:?} {z} {a}");
    assert_eq!(c.at(0.0).unwrap().0, Point::new(0.0, 0.0), "held before");
    assert_eq!(c.at(50.0).unwrap().0, Point::new(100.0, 0.0), "held after");
    let m = c.transform(1.0, 200.0, 100.0);
    let q = m.apply(Point::new(0.0, 0.0));
    assert!((q.x - 100.0).abs() < 1e-9 && (q.y - 50.0).abs() < 1e-9, "the center in the middle");
    assert!(c.remove_key(11) && !c.remove_key(11));
}

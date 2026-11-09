use std::io::{self, Write};

use aurastudio_animation::{Sheets, SheetEdit, Timeline};
use aurastudio_core::{BlendMode, DocType, Fps, PixelDepth, Rect, Rgba8};
use aurastudio_filters::{Filter, Kind};
use aurastudio_layers::*;
use aurastudio_raster::Mask;
use zip::write::SimpleFileOptions;

use crate::schema::*;
use crate::*;

fn paint(s: &mut LayerStack, id: u64, r: Rect, c: Rgba8) { s.image_mut(id).unwrap().fill_rect(r, c); }

fn sample() -> (Snapshot, u64, u64) {
    let mut s = LayerStack::new(96, 64, 32, Start::Paper);
    let ink = s.active_id();
    paint(&mut s, ink, Rect::new(10, 10, 40, 30), [200, 10, 10, 255]);
    // Past the frame too, in the margin.
    paint(&mut s, ink, Rect::new(-9, -6, -1, 5), [1, 2, 3, 255]);
    let folder = s.add_folder("Shading");
    let shade = s.add_layer("Shade");
    if let Some(Node::Layer(l)) = s.node_mut(shade) { (l.blend, l.opacity) = (BlendMode::Multiply, 0.5); }
    if let Some(Node::Folder(f)) = s.node_mut(folder) { (f.blend, f.visible) = (FolderBlend::Isolated(BlendMode::Screen), false); }
    paint(&mut s, shade, Rect::new(0, 0, 20, 20), [0, 0, 255, 128]);
    let mut snap = Snapshot::of(s);
    snap.dpi = 600.0;
    snap.undo = vec![
        Step { label: "Brush".into(), layer: ink, data: vec![1, 2, 3], sheet: Some([7, 3, 1]), ..Step::default() },
        Step { label: "Fill".into(), layer: shade, data: vec![9; 5000], joined: true, ..Step::default() },
    ];
    snap.redo = vec![Step { label: "Erase".into(), layer: ink, ..Step::default() }];
    (snap, ink, shade)
}

fn bytes(snap: &Snapshot) -> Vec<u8> {
    let mut buf = io::Cursor::new(Vec::new());
    write(snap, &mut buf).unwrap();
    buf.into_inner()
}

fn roundtrip(snap: &Snapshot) -> Snapshot { read(io::Cursor::new(bytes(snap)), 16384).unwrap() }

/// Everything a node keeps, its pixels as a fingerprint.
fn describe(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        out.push(match n {
            Node::Layer(l) => {
                let r = l.surface.pixel_bounds().unwrap_or(Rect::of_size(1, 1)).union(&l.surface.rect());
                format!("layer {} {:?} vis={} op={} {:?} lock={} bg={} {:?} px={:x} fill={:?}", l.id, l.name, l.visible, l.opacity, l.blend, l.locked, l.background, l.props, l.surface.fingerprint(r), l.surface.fill())
            }
            Node::Filter(f) => format!("filter {} {:?} vis={} op={} lock={} {:?} {:?}", f.id, f.name, f.visible, f.opacity, f.locked, f.props, f.filter),
            Node::Folder(f) => format!("folder {} {:?} vis={} op={} {:?} lock={} fold={} {:?} {:?} [{}]", f.id, f.name, f.visible, f.opacity, f.blend, f.locked, f.collapsed, f.props, f.kind, f.children.len()),
            Node::Mask(m) => format!("mask {} {:?} vis={} lock={} {:?} px={:x}", m.id, m.name, m.visible, m.locked, m.props, m.mask.fingerprint(m.mask.rect())),
        });
        if let Node::Folder(f) = n { describe(&f.children, out); }
    }
}

fn tree(s: &LayerStack) -> Vec<String> { let mut v = Vec::new(); describe(&s.root, &mut v); v }

fn merged(s: &LayerStack) -> Vec<Rgba8> { aurastudio_compose::merge(s, s.frame(), Default::default()) }

/// Pixels past the margin (an older, unbounded file) are cut on open.
#[test]
fn pixels_past_the_margin_are_cut_on_open() {
    let (mut snap, ink, _) = sample();
    paint(&mut snap.stack, ink, Rect::new(-50, -20, -10, 5), [1, 2, 3, 255]);
    let back = roundtrip(&snap);
    let area = back.stack.area();
    let Some(Node::Layer(l)) = back.stack.node(ink) else { panic!("the layer") };
    let b = l.surface.pixel_bounds().expect("pixels");
    assert!(area.contains_rect(&b), "{b:?} past {area:?}");
    assert_eq!(l.surface.pixel(-9, -6), [1, 2, 3, 255], "the margin is kept");
}

#[test]
fn save_and_open_give_the_same_document() {
    let (snap, ink, _) = sample();
    let back = roundtrip(&snap);
    assert_eq!((back.stack.width, back.stack.height, back.stack.tile, back.dpi), (96, 64, 32, 600.0));
    assert_eq!(tree(&back.stack), tree(&snap.stack));
    assert_eq!(back.stack.active_id(), snap.stack.active_id());
    // Pixels in the margin came along.
    assert_eq!(back.stack.image(ink).unwrap().pixel(-5, 0), [1, 2, 3, 255]);
    assert_eq!(back.undo, snap.undo);
    assert_eq!(back.redo, snap.redo);
    assert_eq!(merged(&back.stack), merged(&snap.stack));
    let Some(Node::Layer(paper)) = back.stack.root.first() else { panic!() };
    assert!(paper.locked && paper.background);
    // New layers get ids the file didn't use.
    let mut b = back.stack;
    let new = b.add_layer("new");
    assert!(!snap.stack.layer_ids().contains(&new));
}

#[test]
fn settings_selection_profile_and_masks_come_back() {
    let (mut snap, ..) = sample();
    snap.kind = DocType::Comics;
    snap.depth = PixelDepth::U16;
    snap.fps = Fps::new(30000, 1001);
    snap.frame = 7;
    snap.origin = Some("/x/orig.aurs".into());
    snap.profile = Some(("Display P3".into(), vec![7; 3000]));
    let mut sel = Mask::new(96, 64, 32, 0);
    sel.fill_rect(Rect::new(5, 6, 50, 40), 200);
    snap.selection = Some(sel.clone());
    let mut kept = Mask::new(96, 64, 32, 0);
    kept.fill_rect(Rect::new(-10, 0, 30, 30), 255);
    snap.stack.add_mask("Saved selection", kept);
    let back = roundtrip(&snap);
    assert_eq!((back.kind, back.depth, back.fps, back.frame), (DocType::Comics, PixelDepth::U16, Fps::new(30000, 1001), 7));
    assert_eq!(back.origin, snap.origin);
    assert_eq!(back.profile, snap.profile);
    let bs = back.selection.unwrap();
    assert_eq!(bs.fingerprint(bs.rect()), sel.fingerprint(sel.rect()));
    assert_eq!(tree(&back.stack), tree(&snap.stack));
    assert_eq!(peek_origin_of(&snap), snap.origin);
}

fn peek_origin_of(snap: &Snapshot) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("aurs-peek-{}", std::process::id()));
    let path = dir.join("a.aurs");
    save(snap, &path, &mut SaveCaches::default()).unwrap();
    let o = peek_origin(&path);
    std::fs::remove_dir_all(dir).unwrap();
    o
}

/// Every setting of every kind of node, folders three deep, every blend
/// mode, label and filter: all of it comes back.
#[test]
fn every_layer_setting_comes_back() {
    let (w, h, tile) = (130, 70, 16);
    let mut seed = 7u32;
    let mut rnd = move || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); seed >> 8 };
    let mut id = 0;
    let mut next = || { id += 1; id };
    let layer = |id: u64, name: &str, background: bool, rnd: &mut dyn FnMut() -> u32| {
        let mut l = Layer::new(w, h, tile, background);
        (l.id, l.name) = (id, name.into());
        // Some tiles painted, the others left to the fill.
        let px: Vec<Rgba8> = (0..70 * h).map(|_| { let v = rnd(); [(v & 255) as u8, (v >> 8) as u8, 7, (v >> 16) as u8] }).collect();
        l.surface.write(Rect::new(0, 0, 70, h as i32), &px);
        l
    };
    let props = |label: u8, clip, reference, draft| Props { label, clip, reference, draft, alpha_lock: draft };
    let mut paper = layer(next(), "Paper", true, &mut rnd);
    paper.locked = true;
    let mut lines = layer(next(), "Lines", false, &mut rnd);
    lines.props = props(1, false, true, false);
    let base = layer(next(), "Base", false, &mut rnd);
    let mut shadow = layer(next(), "Shadow", false, &mut rnd);
    (shadow.blend, shadow.opacity, shadow.props) = (BlendMode::Multiply, 0.33, props(3, true, false, false));
    let soft = FilterLayer { id: next(), name: "Soft".into(), visible: false, opacity: 0.6, locked: true, props: props(4, true, false, false), filter: Filter { values: vec![2.75], ..Filter::new(Kind::GaussianBlur) } };
    let mut sketch = layer(next(), "Sketch", false, &mut rnd);
    (sketch.visible, sketch.locked, sketch.props) = (false, true, props(5, false, true, true));
    let odd = layer(next(), "Ink é 漢字 🎨 \"quoted\"", false, &mut rnd);
    let deep = Folder { id: next(), opacity: 0.9, blend: FolderBlend::Isolated(BlendMode::Screen), collapsed: true, children: vec![Node::Layer(odd)], props: props(6, false, false, false), ..Folder::new("Deep") };
    let sketch_id = sketch.id;
    let nested = Folder { id: next(), visible: false, locked: true, collapsed: false, children: vec![Node::Layer(sketch), Node::Folder(deep)], props: props(0, false, false, true), ..Folder::new("Nested") };
    let colors = Folder { id: next(), opacity: 0.7, blend: FolderBlend::Isolated(BlendMode::Multiply), children: vec![Node::Layer(base), Node::Layer(shadow), Node::Filter(soft), Node::Folder(nested)], props: props(2, false, false, false), ..Folder::new("Colors") };
    let mut root = vec![Node::Layer(paper), Node::Layer(lines), Node::Folder(colors)];
    for (i, mode) in BlendMode::ALL.iter().enumerate() {
        let mut l = layer(next(), mode.name(), false, &mut rnd);
        (l.blend, l.opacity) = (*mode, (i as f32 + 1.0) / 14.0);
        l.props.label = (i % LABELS.len()) as u8;
        l.props.clip = i % 3 == 1;
        root.push(Node::Layer(l));
    }
    for kind in Kind::all() {
        let mut f = Filter::new(kind);
        for v in f.values.iter_mut() { *v *= 1.25; }
        f.color = [(rnd() & 255) as u8, 40, 200];
        root.push(Node::Filter(FilterLayer { id: next(), name: kind.name().into(), visible: true, opacity: 0.5, locked: false, props: Props::default(), filter: f }));
    }
    let mut m = Mask::new(w, h, tile, 0);
    m.fill_rect(Rect::new(3, 3, 60, 50), 128);
    root.push(Node::Mask(MaskLayer { id: next(), name: "Kept".into(), visible: false, locked: true, props: props(7, false, false, false), mask: m }));
    let stack = LayerStack::from_parts(w, h, tile, root, sketch_id).unwrap();
    let mut snap = Snapshot::of(stack);
    (snap.dpi, snap.kind) = (350.0, DocType::Animation);
    let back = roundtrip(&snap);
    let (a, b) = (tree(&snap.stack), tree(&back.stack));
    for (x, y) in a.iter().zip(&b) { assert_eq!(x, y); }
    assert_eq!(a.len(), b.len());
    assert_eq!(back.stack.rows(), snap.stack.rows());
    assert_eq!((back.stack.active_id(), back.dpi, back.kind), (sketch_id, 350.0, DocType::Animation));
    assert_eq!(merged(&back.stack), merged(&snap.stack));
}

/// Animation folders come back: template, chosen cel, sheets with their
/// cels and pixels, groups, the cel reference, the onion marks; older
/// spellings ("frame" for a sheet, "sub" for the cel) still read.
#[test]
fn animation_folders_and_groups_come_back() {
    let mut s = LayerStack::new(64, 48, 16, Start::Paper);
    let a = s.add_animation_folder("Perso A");
    s.add_cel(a, 1, Cel { locked: true, ..Cel::new("Layout", false) });
    s.set_cel(a, 2);
    for n in [1u32, 4] {
        let cel = match s.sheets(a).iter().find(|x| x.0 == n) { Some(_) => None, None => s.make_sheet(a, n) };
        let cel = cel.or_else(|| { let mut t = Timeline::default(); t.set_frame(&mut s, n); t.paint_id(&s) }).unwrap();
        paint(&mut s, cel, Rect::new(n as i32 * 5, 5, n as i32 * 5 + 10, 20), [n as u8 * 40, 0, 0, 255]);
    }
    let b = s.add_animation_folder("B");
    let c = s.add_animation_folder("C");
    let g = s.group_animation_folders(&[a, b], "Chars").unwrap();
    s.set_cel_reference(c, 2, true);
    s.set_cel(c, 3);
    let mut t = Timeline::default();
    t.set_frame(&mut s, 3);
    s.check_animation(3).unwrap();
    let mut snap = Snapshot::of(s);
    (snap.kind, snap.frame) = (DocType::Animation, 3);
    snap.onion = Some(OnionMarks { enabled: true, mobile: vec![(-1, None), (2, Some(0.25))], fixed: vec![(4, None)] });
    let check = |back: &Snapshot| {
        back.stack.check_animation(3).unwrap();
        assert_eq!(back.frame, 3);
        assert_eq!(tree(&back.stack), tree(&snap.stack));
        assert_eq!(back.stack.sheets(a), snap.stack.sheets(a));
        assert_eq!(back.stack.anim(c).unwrap().cel, 3);
        assert!(back.stack.anim(c).unwrap().template[2].reference);
        assert_eq!(back.stack.group_members(g), snap.stack.group_members(g));
        assert_eq!(back.stack.group_of(b), Some(g));
        assert_eq!(back.onion, snap.onion);
        assert_eq!(merged(&back.stack), merged(&snap.stack));
    };
    check(&roundtrip(&snap));
    // Older spellings.
    fn old(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(o) => {
                if let Some(x) = o.remove("sheet") { o.insert("frame".into(), x); }
                if o.contains_key("template") { if let Some(x) = o.remove("cel") { o.insert("sub".into(), x); } }
                o.values_mut().for_each(old);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(old),
            _ => {}
        }
    }
    let file = rewrite_json(&bytes(&snap), old);
    check(&read(io::Cursor::new(file), 16384).unwrap());
}

/// The archive with document.json changed by `f`, every other entry as
/// it was.
fn rewrite_json(file: &[u8], mut f: impl FnMut(&mut serde_json::Value)) -> Vec<u8> {
    use std::io::Read;
    let mut zin = zip::ZipArchive::new(io::Cursor::new(file)).unwrap();
    let mut out = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for i in 0..zin.len() {
        let mut e = zin.by_index(i).unwrap();
        let name = e.name().unwrap().into_owned();
        let mut data = Vec::new();
        e.read_to_end(&mut data).unwrap();
        if name == "document.json" {
            let mut j: serde_json::Value = serde_json::from_slice(&data).unwrap();
            f(&mut j);
            data = serde_json::to_vec(&j).unwrap();
        }
        out.start_file(name, SimpleFileOptions::default()).unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

/// The document type comes back; files from before it open as
/// Illustration; a type this version doesn't know is refused; so is a
/// newer version.
#[test]
fn the_document_type_and_version_are_checked() {
    let (mut snap, ..) = sample();
    for kind in DocType::ALL {
        snap.kind = kind;
        assert_eq!(roundtrip(&snap).kind, kind);
    }
    let f = rewrite_json(&bytes(&snap), |j| { j.as_object_mut().unwrap().remove("kind"); });
    assert_eq!(read(io::Cursor::new(f), 16384).unwrap().kind, DocType::Illustration);
    let f = rewrite_json(&bytes(&snap), |j| { j["kind"] = "Storyboard".into(); });
    assert!(read(io::Cursor::new(f), 16384).err().is_some_and(|e| e.contains("newer version")));
    let f = rewrite_json(&bytes(&snap), |j| { j["version"] = (FORMAT_VERSION + 1).into(); });
    assert!(read(io::Cursor::new(f), 16384).err().is_some_and(|e| e.contains("newer version")));
}

/// A 0.19 file (version 5: 64 px tiles, rect-based undo steps) opens,
/// re-cut into the document type's tiles; and so does a 0.1 file
/// (version 1: whole LZ4 layers).
#[test]
fn old_files_open() {
    let (w, h) = (100u32, 70u32);
    let mut px = vec![[255u8, 255, 255, 0]; (w * h) as usize];
    for y in 10..30 { for x in 50..90 { px[(y * w + x) as usize] = [9, 8, 7, 255]; } }
    // 0.19 tiles: count, then column, row, length, LZ4 (size first).
    let mut tiles = Vec::new();
    let mut count = 0u32;
    for ty in 0..h.div_ceil(64) {
        for tx in 0..w.div_ceil(64) {
            let mut raw = Vec::with_capacity(64 * 64 * 4);
            for y in ty * 64..ty * 64 + 64 { for x in tx * 64..tx * 64 + 64 {
                raw.extend(if x < w && y < h { px[(y * w + x) as usize] } else { [0; 4] });
            } }
            let z = lz4_flex::compress_prepend_size(&raw);
            tiles.extend(tx.to_le_bytes());
            tiles.extend(ty.to_le_bytes());
            tiles.extend((z.len() as u32).to_le_bytes());
            tiles.extend(z);
            count += 1;
        }
    }
    let mut blob = count.to_le_bytes().to_vec();
    blob.extend(tiles);
    let doc = |version: u32, layers: &str| format!(r#"{{"format":"aurastudio","version":{version},"width":{w},"height":{h},"dpi":144,"active":2,
        "layers":[{{"type":"layer","id":1,"name":"Paper","visible":true,"opacity":1,"blend":"Normal","locked":true,"background":true}},
                  {{"type":"layer","id":2,"name":"Ink","visible":true,"opacity":1,"blend":"Normal","locked":false{layers}}}],
        "undo":[{{"label":"Brush","layer":2,"rect":[50,10,90,30]}},{{"label":"Bad","layer":2,"rect":[0,0,500,10]}}]}}"#);
    let archive = |version: u32, entries: Vec<(String, Vec<u8>)>| {
        let mut z = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        z.start_file("mimetype", SimpleFileOptions::default()).unwrap();
        z.write_all(b"application/x-aurastudio").unwrap();
        z.start_file("document.json", SimpleFileOptions::default()).unwrap();
        z.write_all(doc(version, "").as_bytes()).unwrap();
        for (n, d) in entries { z.start_file(n, SimpleFileOptions::default()).unwrap(); z.write_all(&d).unwrap(); }
        z.finish().unwrap().into_inner()
    };
    // The Paper: one white tile each.
    let paper = {
        let mut b = count.to_le_bytes().to_vec();
        for ty in 0..h.div_ceil(64) { for tx in 0..w.div_ceil(64) {
            let z = lz4_flex::compress_prepend_size(&[255u8; 64 * 64 * 4]);
            b.extend(tx.to_le_bytes()); b.extend(ty.to_le_bytes()); b.extend((z.len() as u32).to_le_bytes()); b.extend(z);
        } }
        b
    };
    let v5 = archive(5, vec![("layers/1.tiles".into(), paper), ("layers/2.tiles".into(), blob), ("history/undo-0.lz4".into(), vec![0, 0, 0, 0, b'T', b'I', b'L', b'E']), ("history/undo-1.lz4".into(), vec![1])]);
    let raw: Vec<u8> = px.iter().flatten().copied().collect();
    let white_raw = vec![255u8; (w * h * 4) as usize];
    let v1 = archive(1, vec![("layers/1.lz4".into(), lz4_flex::compress_prepend_size(&white_raw)), ("layers/2.lz4".into(), lz4_flex::compress_prepend_size(&raw))]);
    for (file, steps) in [(v5, 1), (v1, 0)] {
        let s = read(io::Cursor::new(file), 16384).unwrap();
        assert_eq!((s.stack.width, s.stack.height, s.stack.tile, s.dpi), (w, h, DocType::Illustration.default_tile_size(), 144.0));
        let ink = s.stack.image(2).unwrap();
        assert_eq!(ink.read(ink.rect()), px);
        assert_eq!(ink.pixel(-5, -5)[3], 0, "nothing past the old canvas");
        assert_eq!(s.stack.image(1).unwrap().pixel(50, 50), [255; 4]);
        // The step past the canvas is dropped, the other kept as it was.
        assert_eq!(s.undo.len(), steps);
        if steps == 1 { assert_eq!((s.undo[0].rect, s.undo[0].label.as_str()), (Some([50, 10, 90, 30]), "Brush")); }
    }
}

#[test]
fn other_files_are_refused() {
    assert!(read(io::Cursor::new(b"hello".to_vec()), 16384).is_err());
    let mut z = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    z.start_file("mimetype", SimpleFileOptions::default()).unwrap();
    z.write_all(b"application/zip").unwrap();
    assert!(read(io::Cursor::new(z.finish().unwrap().into_inner()), 16384).is_err());
    // Too big for the GPU.
    assert!(read(io::Cursor::new(bytes(&sample().0)), 64).is_err());
}

/// Damaged files (bytes changed or cut) give an error or a valid
/// document, never a panic or a huge allocation.
#[test]
fn damaged_files_never_crash() {
    let (mut snap, ..) = sample();
    let mut sel = Mask::new(96, 64, 32, 0);
    sel.fill_rect(Rect::new(1, 1, 9, 9), 255);
    snap.selection = Some(sel);
    let good = bytes(&snap);
    let mut seed = 0x1234_5678_9abc_def1u64;
    let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    for _ in 0..3000 {
        let mut bad = good.clone();
        match next() % 3 {
            0 => bad.truncate((next() % good.len() as u64) as usize),
            _ => for _ in 0..1 + next() % 8 {
                let i = (next() % bad.len() as u64) as usize;
                bad[i] = next() as u8;
            },
        }
        if let Ok(s) = read(io::Cursor::new(bad), 4096) { assert!(s.stack.layer_count() > 0); }
    }
}

#[test]
fn save_writes_through_a_temporary_file_and_reuses_tiles() {
    let dir = std::env::temp_dir().join(format!("aurs-test-{}", std::process::id()));
    let path = dir.join("drawing.aurs");
    let (mut snap, ink, _) = sample();
    let mut caches = SaveCaches::default();
    save(&snap, &path, &mut caches).unwrap();
    paint(&mut snap.stack, ink, Rect::new(0, 0, 4, 4), [5, 5, 5, 255]);
    snap.preview = Some(preview(&merged(&snap.stack), 96, 64));
    save(&snap, &path, &mut caches).unwrap();
    let back = open(&path, 16384).unwrap();
    assert_eq!(doc_name(&path), "drawing");
    assert_eq!(tree(&back.stack), tree(&snap.stack));
    assert!(!path.with_extension("aurs.saving").exists());
    let (tw, th, _) = thumbnail(&path, 512).unwrap();
    assert_eq!((tw, th), (96, 64));
    std::fs::remove_dir_all(dir).unwrap();
}

/// A picture opened band by band (RGB, grey) gives the same pixels as
/// the whole RGBA one.
#[test]
fn pictures_import_and_export() {
    let dir = std::env::temp_dir().join(format!("aurs-img-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (300u32, 530u32);
    let rgb = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8]));
    let grey = image::GrayImage::from_fn(w, h, |x, y| image::Luma([((x * 3 + y) % 256) as u8]));
    let rgba = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 200) as u8, 9, (y % 100) as u8, if x < 100 { 0 } else { 255 }]));
    for (name, img) in [("rgb", image::DynamicImage::ImageRgb8(rgb)), ("grey", image::DynamicImage::ImageLuma8(grey)), ("rgba", image::DynamicImage::ImageRgba8(rgba))] {
        let path = dir.join(format!("{name}.png"));
        img.save(&path).unwrap();
        let (n, s) = import_image(&path, 16384, 64).unwrap();
        assert_eq!(n, name);
        let want = img.to_rgba8();
        let ids = s.layer_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(s.active_id(), ids[1]);
        let l = s.image(ids[0]).unwrap();
        assert_eq!(l.read(l.rect()), want.as_raw().as_chunks::<4>().0, "{name}");
    }
    // Decoded whole (JPEG, BMP, 16-bit PNG): the same pixels as the image
    // crate gives.
    let rgb = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8]));
    let deep = image::ImageBuffer::<image::Rgb<u16>, _>::from_fn(w, h, |x, y| image::Rgb([(x * 200) as u16, (y * 100) as u16, 7]));
    for (file, img) in [("a.jpg", image::DynamicImage::ImageRgb8(rgb.clone())), ("a.bmp", image::DynamicImage::ImageRgb8(rgb)), ("deep.png", image::DynamicImage::ImageRgb16(deep))] {
        let path = dir.join(file);
        img.save(&path).unwrap();
        let want = image::open(&path).unwrap().to_rgba8();
        let (_, s) = import_image(&path, 16384, 64).unwrap();
        let l = s.image(s.layer_ids()[0]).unwrap();
        let got = l.read(l.rect());
        let worst = got.iter().flatten().zip(want.as_raw()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(worst <= 1, "{file}: off by {worst}");
    }
    let px: Vec<Rgba8> = (0..30 * 20).map(|i| [i as u8, 50, 200, 255]).collect();
    let png = dir.join("photo.png");
    export_image(&px, 30, 20, &png).unwrap();
    let (_, s) = import_image(&png, 16384, 16).unwrap();
    assert_eq!(s.image(s.layer_ids()[0]).unwrap().read(Rect::of_size(30, 20)), px);
    export_image(&px, 30, 20, &dir.join("photo.jpg")).unwrap();
    assert_eq!(import_image(&dir.join("photo.jpg"), 16384, 16).unwrap().1.width, 30);
    assert!(import_image(&png, 16, 16).is_err());
    assert!(export_image(&px, 30, 20, &dir.join("x.gif")).is_err());
    assert!(!dir.join("x.gif").exists(), "a refused export leaves no file");
    assert!(export_image(&px[1..], 30, 20, &dir.join("short.png")).is_err());
    assert!(can_open(&png) && can_open(std::path::Path::new("a.AURS")) && !can_open(std::path::Path::new("a.txt")));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn preview_fits_and_averages() {
    let (w, h, px) = downscale(&vec![[255u8; 4]; 2000 * 1000], 2000, 1000, 512);
    assert_eq!((w, h), (512, 256));
    assert!(px.iter().all(|&v| v == [255; 4]));
    assert_eq!(downscale(&[[9u8; 4]; 16], 4, 4, 512).0, 4);
    assert_eq!(downscale(&[], 0, 0, 512).0, 0);
}

#[test]
fn migrations_fill_what_old_files_lack() {
    let j: DocJson = serde_json::from_str(r#"{"format":"aurastudio","version":5,"width":4,"height":4,"dpi":72,"kind":"Animation","active":1,"layers":[]}"#).unwrap();
    let j = migrate(j);
    assert_eq!(j.tile, Some(DocType::Animation.default_tile_size()));
    assert_eq!((depth_from(&j.depth), fps_from(j.fps)), (PixelDepth::U8, Fps::default()));
    assert_eq!(fps_from(Some([0, 1])), Fps::default());
}

/// Save and open time of an A4 page at 300 dpi, four painted layers.
#[test]
#[ignore]
fn a4_save_and_open_time() {
    let (w, h) = (2480, 3508);
    let mut s = LayerStack::new(w, h, 128, Start::Paper);
    let mut noise = 1u32;
    for _ in 0..4 {
        let id = s.add_layer("Paint");
        let px: Vec<Rgba8> = (0..w * h).map(|_| { noise = noise.wrapping_mul(1664525).wrapping_add(1013904223); [(noise >> 24) as u8, (noise >> 16) as u8, 9, 255] }).collect();
        s.image_mut(id).unwrap().write(Rect::of_size(w, h), &px);
    }
    let snap = Snapshot::of(s);
    let mut caches = SaveCaches::default();
    for _ in 0..2 {
        let t = std::time::Instant::now();
        let mut buf = io::Cursor::new(Vec::new());
        write_with(&snap, &mut buf, &mut caches).unwrap();
        let wt = t.elapsed();
        let len = buf.get_ref().len();
        let t = std::time::Instant::now();
        let _ = read(io::Cursor::new(buf.into_inner()), 16384).unwrap();
        println!("write {:.0} ms, read {:.0} ms, {} MB", wt.as_secs_f64() * 1e3, t.elapsed().as_secs_f64() * 1e3, len >> 20);
    }
}

/// A thumbnail is kept on disk and read back from there; kept ones of
/// files no longer listed go.
#[test]
fn thumbnails_are_kept_on_disk() {
    let dir = std::env::temp_dir().join(format!("aurs-thumbs-{}", std::process::id()));
    let cache = dir.join("cache");
    std::fs::create_dir_all(&dir).unwrap();
    let pic = dir.join("pic.png");
    let px: Vec<u8> = (0..600 * 400).flat_map(|i| [(i % 251) as u8, 40, 200, 255]).collect();
    image::save_buffer(&pic, &px, 600, 400, image::ExtendedColorType::Rgba8).unwrap();
    let first = crate::thumbnail_cached(&pic, 64, &cache).unwrap();
    assert_eq!((first.0, first.1), (64, 43));
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    let again = crate::thumbnail_cached(&pic, 64, &cache).unwrap();
    assert_eq!(again, first, "the same picture, from the kept file");
    std::fs::write(cache.join("0000000000000000.png"), b"old").unwrap();
    crate::trim_thumbnails(&cache, std::slice::from_ref(&pic), 64);
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

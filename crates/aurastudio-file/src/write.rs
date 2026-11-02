//! Writing an archive: layers encoded on all cores (unchanged tiles taken
//! from the last save's encodings), then stored in a zip; saved next to
//! the file first, then moved over it.

use std::collections::HashMap;
use std::io::{self, Write, Seek};
use std::path::Path;

use aurastudio_layers::{FolderBlend, FolderKind, Node};
use aurastudio_raster::{Image, Mask, SaveCache};
use rayon::prelude::*;
use zip::write::SimpleFileOptions;

use crate::limits::{EXTENSION, MIMETYPE};
use crate::schema::*;
use crate::snapshot::Snapshot;

/// The last save's encodings, per layer: an incremental save encodes only
/// what changed.
#[derive(Default)]
pub struct SaveCaches(HashMap<u64, SaveCache>);

fn tree(nodes: &[Node]) -> Vec<NodeJson> {
    nodes.iter().map(|n| match n {
        Node::Layer(l) => NodeJson::Layer {
            id: l.id, name: l.name.clone(), visible: l.visible, opacity: l.opacity, blend: l.blend.name().into(), locked: l.locked, background: l.background, props: PropsJson::of(l.props),
        },
        Node::Folder(f) => NodeJson::Folder {
            id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity,
            blend: match f.blend { FolderBlend::PassThrough => PASS_THROUGH.into(), FolderBlend::Isolated(m) => m.name().into() },
            locked: f.locked, collapsed: f.collapsed, children: tree(&f.children), props: PropsJson::of(f.props),
            animation: f.anim().map(AnimJson::of), sheet: f.sheet_start(), group: f.kind == FolderKind::Group,
        },
        Node::Filter(f) => NodeJson::Filter {
            id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity, locked: f.locked,
            filter: FilterJson { kind: f.filter.kind.name().into(), values: f.filter.values.clone(), color: f.filter.color }, props: PropsJson::of(f.props),
        },
        Node::Mask(m) => NodeJson::Mask { id: m.id, name: m.name.clone(), visible: m.visible, locked: m.locked, props: PropsJson::of(m.props) },
    }).collect()
}

type Entry<'a> = (String, u64, Option<&'a Image>, Option<&'a Mask>);

fn collect<'a>(nodes: &'a [Node], out: &mut Vec<Entry<'a>>) {
    for n in nodes {
        match n {
            Node::Layer(l) => out.push((format!("layers/{}.surface", l.id), l.id, Some(&l.surface), None)),
            Node::Mask(m) => out.push((format!("masks/{}.surface", m.id), m.id, None, Some(&m.mask))),
            Node::Folder(f) => collect(&f.children, out),
            Node::Filter(_) => {}
        }
    }
}

pub(crate) fn json_of(snap: &Snapshot) -> DocJson {
    let s = &snap.stack;
    let step = |x: &crate::Step| StepJson { label: x.label.clone(), layer: x.layer, rect: x.rect.map(|r| r.map(i64::from)) };
    DocJson {
        format: "aurastudio".into(), version: FORMAT_VERSION, width: s.width, height: s.height, dpi: snap.dpi,
        kind: snap.kind.name().into(), tile: Some(s.tile), depth: snap.depth.name().into(), frame: snap.frame,
        fps: (snap.fps != aurastudio_core::Fps::default()).then_some([snap.fps.num, snap.fps.den]),
        active: s.active_id(), layers: tree(&s.root),
        undo: snap.undo.iter().map(step).collect(), redo: snap.redo.iter().map(step).collect(),
        origin: snap.origin.clone(),
        onion: snap.onion.as_ref().map(|o| OnionJson { enabled: o.enabled, mobile: o.mobile.clone(), fixed: o.fixed.clone() }),
        profile: snap.profile.as_ref().map(|p| p.0.clone()),
        selection: snap.selection.is_some(),
    }
}

/// Write the snapshot as an .aurs archive.
pub fn write<W: Write + Seek>(snap: &Snapshot, out: W) -> io::Result<()> { write_with(snap, out, &mut SaveCaches::default()) }

/// The same, reusing (and then keeping) `caches`.
pub fn write_with<W: Write + Seek>(snap: &Snapshot, out: W, caches: &mut SaveCaches) -> io::Result<()> {
    aurastudio_core::zone!("file write");
    let json = json_of(snap);
    // Every surface, encoded on all cores.
    let mut surfaces = Vec::new();
    collect(&snap.stack.root, &mut surfaces);
    let mut owned: Vec<(u64, SaveCache)> = surfaces.iter().map(|s| (s.1, caches.0.remove(&s.1).unwrap_or_default())).collect();
    let blobs: Vec<Vec<u8>> = surfaces.par_iter().zip(owned.par_iter_mut()).map(|(s, (_, cache))| match (s.2, s.3) {
        (Some(img), _) => img.save_with(cache).0,
        (_, Some(m)) => m.save(),
        _ => Vec::new(),
    }).collect();
    caches.0 = owned.into_iter().collect();
    let mut zip = zip::ZipWriter::new(out);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let err = |e: zip::result::ZipError| io::Error::other(e);
    zip.start_file("mimetype", stored).map_err(err)?;
    zip.write_all(MIMETYPE.as_bytes())?;
    zip.start_file("document.json", stored).map_err(err)?;
    zip.write_all(&serde_json::to_vec_pretty(&json).map_err(io::Error::other)?)?;
    if let Some((w, h, px)) = &snap.preview {
        use image::ImageEncoder;
        if px.len() != *w as usize * *h as usize { return Err(io::Error::other("preview size")); }
        let raw: Vec<u8> = px.iter().flatten().copied().collect();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&raw, *w, *h, image::ExtendedColorType::Rgba8).map_err(io::Error::other)?;
        zip.start_file("preview.png", stored).map_err(err)?;
        zip.write_all(&png)?;
    }
    for (s, b) in surfaces.iter().zip(&blobs) {
        zip.start_file(s.0.as_str(), stored).map_err(err)?;
        zip.write_all(b)?;
    }
    if let Some(sel) = &snap.selection {
        zip.start_file("selection.surface", stored).map_err(err)?;
        zip.write_all(&sel.save())?;
    }
    if let Some((_, icc)) = &snap.profile {
        zip.start_file("profile.icc", stored).map_err(err)?;
        zip.write_all(icc)?;
    }
    for (kind, steps) in [("undo", &snap.undo), ("redo", &snap.redo)] {
        for (i, st) in steps.iter().enumerate() {
            zip.start_file(step_entry(FORMAT_VERSION, kind, i), stored).map_err(err)?;
            zip.write_all(&st.data)?;
        }
    }
    zip.finish().map_err(err)?;
    Ok(())
}

/// Save to `path` safely: written next to it first, then moved over it, so
/// a crash or a full disk mid-save never leaves a broken file in its place.
pub fn save(snap: &Snapshot, path: &Path, caches: &mut SaveCaches) -> io::Result<()> {
    if let Some(dir) = path.parent() { if !dir.as_os_str().is_empty() { std::fs::create_dir_all(dir)?; } }
    let tmp = path.with_extension(format!("{EXTENSION}.saving"));
    let result = (|| {
        let mut file = io::BufWriter::new(std::fs::File::create(&tmp)?);
        write_with(snap, &mut file, caches)?;
        let file = file.into_inner().map_err(std::io::IntoInnerError::into_error)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

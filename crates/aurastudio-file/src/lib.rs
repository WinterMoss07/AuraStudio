//! `.aurs` documents (a zip like .kra / .ora, entries stored: mimetype,
//! document.json, layers/<id>.tiles LZ4, history/, preview.png) and image
//! import / export. Reading checks everything: a damaged file gives an
//! error, never a crash.

use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use aurastudio_canvas::{BlendMode, TILE};
use aurastudio_document::{DocId, Document, History, Kind, SavedStep, DEFAULT_BUDGET};
use aurastudio_filters::Filter;
use aurastudio_layers::{Anim, FilterLayer, Folder, FolderBlend, FolderKind, Layer, LayerStack, Node, Props, Cel, Tiles, LABELS, TILE_BYTES};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

/// File extension of AuraStudio documents.
pub const EXTENSION: &str = "aurs";
/// Version written in new files; files from a newer version are refused.
/// 2: layers saved as tiles. 3: filter layers, labels, clipping,
/// reference and draft marks. 4: animation folders (written only when
/// there is one: other files stay readable by older versions).
pub const FORMAT_VERSION: u32 = 4;
const PLAIN_VERSION: u32 = 3;
/// Undo steps kept in a saved file (the newest ones).
pub const SAVED_UNDO_STEPS: usize = 50;
/// Image formats Open can read, by extension.
pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];
/// Formats Export can write.
pub const EXPORT_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

const MIMETYPE: &str = "application/x-aurastudio";
const PREVIEW_MAX: u32 = 512;
/// Largest document.json accepted (the layer tree and step list are small).
const MAX_JSON: u64 = 64 << 20;

// --- document.json --------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct DocJson {
    format:  String,
    version: u32,
    width:   u32,
    height:  u32,
    dpi:     f32,
    /// Illustration, Animation or Comics (files before it: Illustration).
    #[serde(default = "illustration")]
    kind:    String,
    /// The current frame (animation).
    #[serde(default = "first_frame", skip_serializing_if = "is_first_frame")]
    frame:   u32,
    active:  u64,
    /// Bottom to top.
    layers:  Vec<NodeJson>,
    #[serde(default)]
    undo:    Vec<StepJson>,
    #[serde(default)]
    redo:    Vec<StepJson>,
    /// Autosaves only: the file the work belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin:  Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum NodeJson {
    Layer {
        id: u64, name: String, visible: bool, opacity: f32, blend: String, locked: bool,
        #[serde(default)]
        background: bool,
        #[serde(default, skip_serializing_if = "PropsJson::is_default")]
        props: PropsJson,
    },
    Folder {
        id: u64, name: String, visible: bool, opacity: f32, blend: String, locked: bool,
        #[serde(default)]
        collapsed: bool,
        children: Vec<NodeJson>,
        #[serde(default, skip_serializing_if = "PropsJson::is_default")]
        props: PropsJson,
        /// An animation folder: its template.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        animation: Option<AnimJson>,
        /// A frame of one: its number.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<u32>,
        /// Animation folders put together.
        #[serde(default, skip_serializing_if = "is_false")]
        group: bool,
    },
    Filter {
        id: u64, name: String, visible: bool, opacity: f32, locked: bool,
        filter: FilterJson,
        #[serde(default, skip_serializing_if = "PropsJson::is_default")]
        props: PropsJson,
    },
}

/// A node's Props: the label by name ("Blue"...), the marks.
#[derive(Serialize, Deserialize, Default, PartialEq)]
struct PropsJson {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label:     String,
    #[serde(default, skip_serializing_if = "is_false")]
    clip:      bool,
    #[serde(default, skip_serializing_if = "is_false")]
    reference: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    draft:     bool,
}

fn is_false(b: &bool) -> bool { !*b }

fn illustration() -> String { Kind::Illustration.name().into() }

fn first_frame() -> u32 { 1 }

fn is_first_frame(n: &u32) -> bool { *n == 1 }

/// An animation folder's template and chosen cel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct AnimJson {
    template: Vec<CelJson>,
    #[serde(default, rename = "cel", alias = "sub")]
    cel:      usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct CelJson {
    name:    String,
    #[serde(default, skip_serializing_if = "is_false")]
    colo:    bool,
    #[serde(default, skip_serializing_if = "is_false")]
    reference: bool,
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    locked:  bool,
    #[serde(default = "yes")]
    onion:   bool,
}

fn yes() -> bool { true }

impl AnimJson {
    fn of(a: &Anim) -> Self {
        let template = a.template.iter().map(|t| CelJson { name: t.name.clone(), colo: t.colo, reference: t.reference, visible: t.visible, locked: t.locked, onion: t.onion }).collect();
        Self { template, cel: a.cel }
    }

    fn anim(self) -> Anim {
        let template: Vec<Cel> = self.template.into_iter().map(|t| Cel { name: t.name, colo: t.colo, reference: t.reference, visible: t.visible, locked: t.locked, onion: t.onion }).collect();
        let cel = self.cel.min(template.len().saturating_sub(1));
        Anim { template, cel }
    }
}

impl PropsJson {
    fn is_default(&self) -> bool { *self == PropsJson::default() }

    fn of(p: Props) -> Self {
        let label = if p.label == 0 { String::new() } else { LABELS.get(p.label as usize).map_or(String::new(), |l| l.0.to_owned()) };
        PropsJson { label, clip: p.clip, reference: p.reference, draft: p.draft }
    }

    /// (An unknown label name: none.)
    fn props(&self) -> Props {
        let label = LABELS.iter().position(|l| l.0 == self.label && !self.label.is_empty()).unwrap_or(0) as u8;
        Props { label, clip: self.clip, reference: self.reference, draft: self.draft }
    }
}

/// A filter layer's filter: its kind by name ("Gaussian blur"...), its
/// values and color.
#[derive(Serialize, Deserialize)]
struct FilterJson {
    kind:   String,
    values: Vec<f32>,
    #[serde(default)]
    color:  [u8; 3],
}

#[derive(Serialize, Deserialize)]
struct StepJson {
    label: String,
    layer: u64,
    rect:  [u32; 4],
}

const PASS_THROUGH: &str = "Pass Through";

fn blend_from(name: &str) -> BlendMode {
    BlendMode::ALL.into_iter().find(|m| m.name() == name).unwrap_or_default()
}

// --- Saving ---------------------------------------------------------------

/// Everything a save writes, copied from the document in one go: the slow
/// part (compressing, writing) runs on another thread meanwhile.
pub struct Snapshot {
    json:     DocJson,
    /// (layer id, a copy of its tiles). Compressed (`pack_tiles`) when
    /// written, not while the document is locked: the interface waits for
    /// that lock.
    layers:   Vec<(u64, Tiles)>,
    undo:     Vec<Vec<u8>>,
    redo:     Vec<Vec<u8>>,
    /// Merged image, small: (width, height, RGBA).
    preview:  Option<(u32, u32, Vec<u8>)>,
    /// Document revision this snapshot is of.
    pub revision: u64,
}

fn has_animation(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n { Node::Folder(f) => f.kind != FolderKind::Plain || has_animation(&f.children), _ => false })
}

/// Take a snapshot of `doc`. `origin`: for autosaves, the file the work
/// belongs to.
pub fn snapshot(doc: &Document, origin: Option<&Path>) -> Snapshot {
    fn walk(nodes: &[Node], blobs: &mut Vec<(u64, Tiles)>) -> Vec<NodeJson> {
        nodes.iter().map(|n| match n {
            Node::Layer(l) => {
                blobs.push((l.id, l.tiles.clone()));
                NodeJson::Layer {
                    id: l.id, name: l.name.clone(), visible: l.visible, opacity: l.opacity,
                    blend: l.blend.name().into(), locked: l.locked, background: l.background,
                    props: PropsJson::of(l.props),
                }
            }
            Node::Folder(f) => NodeJson::Folder {
                id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity,
                blend: match f.blend {
                    FolderBlend::PassThrough => PASS_THROUGH.into(),
                    FolderBlend::Isolated(m) => m.name().into(),
                },
                locked: f.locked, collapsed: f.collapsed,
                children: walk(&f.children, blobs),
                props: PropsJson::of(f.props),
                animation: f.anim().map(AnimJson::of),
                frame: f.frame(),
                group: f.kind == FolderKind::Group,
            },
            Node::Filter(f) => NodeJson::Filter {
                id: f.id, name: f.name.clone(), visible: f.visible, opacity: f.opacity, locked: f.locked,
                filter: FilterJson { kind: f.filter.kind.name().into(), values: f.filter.values.clone(), color: f.filter.color },
                props: PropsJson::of(f.props),
            },
        }).collect()
    }
    let mut layers = Vec::new();
    let tree = walk(&doc.layers.root, &mut layers);
    let (undo, redo) = doc.history().export(SAVED_UNDO_STEPS);
    let step = |s: &SavedStep| StepJson { label: s.label.clone(), layer: s.layer_id, rect: [s.rect.0, s.rect.1, s.rect.2, s.rect.3] };
    let (w, h) = (doc.layers.width, doc.layers.height);
    Snapshot {
        json: DocJson {
            format: "aurastudio".into(),
            version: if has_animation(&doc.layers.root) { FORMAT_VERSION } else { PLAIN_VERSION },
            width: w,
            height: h,
            dpi: doc.dpi,
            kind: doc.kind.name().into(),
            frame: doc.layers.time(),
            active: doc.layers.active_id(),
            layers: tree,
            undo: undo.iter().map(step).collect(),
            redo: redo.iter().map(step).collect(),
            origin: origin.map(Path::to_path_buf),
        },
        layers,
        undo: undo.into_iter().map(|s| s.data).collect(),
        redo: redo.into_iter().map(|s| s.data).collect(),
        preview: Some(doc.layers.composite_preview(PREVIEW_MAX)),
        revision: doc.revision(),
    }
}

/// Write the snapshot as an .aurs archive.
pub fn write<W: Write + Seek>(snap: &Snapshot, out: W) -> io::Result<()> {
    let mut zip = zip::ZipWriter::new(out);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let err = |e: zip::result::ZipError| io::Error::other(e);
    zip.start_file("mimetype", stored).map_err(err)?;
    zip.write_all(MIMETYPE.as_bytes())?;
    zip.start_file("document.json", stored).map_err(err)?;
    zip.write_all(&serde_json::to_vec_pretty(&snap.json).map_err(io::Error::other)?)?;
    if let Some((w, h, rgba)) = &snap.preview {
        let mut png = Vec::new();
        image::RgbaImage::from_raw(*w, *h, rgba.clone())
            .ok_or_else(|| io::Error::other("preview size"))?
            .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        zip.start_file("preview.png", stored).map_err(err)?;
        zip.write_all(&png)?;
    }
    for (id, tiles) in &snap.layers {
        zip.start_file(format!("layers/{id}.tiles"), stored).map_err(err)?;
        zip.write_all(&pack_tiles(tiles))?;
    }
    for (kind, steps) in [("undo", &snap.undo), ("redo", &snap.redo)] {
        for (i, data) in steps.iter().enumerate() {
            zip.start_file(format!("history/{kind}-{i}.lz4"), stored).map_err(err)?;
            zip.write_all(data)?;
        }
    }
    zip.finish().map_err(err)?;
    Ok(())
}

/// Save to `path` safely: written next to it first, then moved over it, so
/// a crash or a full disk mid-save never leaves a broken file in its place.
pub fn save(snap: &Snapshot, path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() { std::fs::create_dir_all(dir)?; }
    }
    let tmp = path.with_extension(format!("{EXTENSION}.saving"));
    let result = (|| {
        let mut file = io::BufWriter::new(std::fs::File::create(&tmp)?);
        write(snap, &mut file)?;
        let file = file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

// --- Opening --------------------------------------------------------------

/// A document read from a file.
pub struct Opened {
    pub doc:    Document,
    /// Autosaves only: the file the work belongs to.
    pub origin: Option<PathBuf>,
}

/// Read an .aurs archive. `max_side`: biggest canvas side accepted (the
/// GPU limit).
pub fn read<R: Read + Seek>(input: R, id: DocId, name: &str, max_side: u32) -> Result<Opened, String> {
    let mut zip = zip::ZipArchive::new(input).map_err(|_| "not an AuraStudio file (not an archive)".to_string())?;
    let mimetype = read_entry(&mut zip, "mimetype", 256)?;
    if mimetype != MIMETYPE.as_bytes() { return Err("not an AuraStudio file".into()); }
    let json: DocJson = serde_json::from_slice(&read_entry(&mut zip, "document.json", MAX_JSON)?)
        .map_err(|e| format!("damaged file (document.json: {e})"))?;
    if json.format != "aurastudio" { return Err("not an AuraStudio file".into()); }
    if json.version > FORMAT_VERSION {
        return Err("made by a newer version of AuraStudio".into());
    }
    // (A kind this version doesn't know may hold what it can't show.)
    let kind = Kind::from_name(&json.kind).ok_or("made by a newer version of AuraStudio (unknown document type)")?;
    let (w, h) = (json.width, json.height);
    if w == 0 || h == 0 || w > max_side || h > max_side {
        return Err(format!("canvas {w} × {h} px is bigger than this GPU allows ({max_side} px)"));
    }
    let len = w as usize * h as usize * 4;
    let version = json.version;

    fn build<R: Read + Seek>(nodes: Vec<NodeJson>, zip: &mut zip::ZipArchive<R>, size: (u32, u32), version: u32)
        -> Result<Vec<Node>, String>
    {
        let len = size.0 as usize * size.1 as usize * 4;
        nodes.into_iter().map(|n| Ok(match n {
            NodeJson::Layer { id, name, visible, opacity, blend, locked, background, props } => {
                let mut layer = Layer::new(size.0, size.1, background);
                let damaged = || format!("damaged file (layer {id})");
                if version >= 2 {
                    let tiles_max = (size.0.div_ceil(TILE) * size.1.div_ceil(TILE)) as u64 * (TILE_BYTES as u64 + 64) + 4;
                    let blob = read_entry(zip, &format!("layers/{id}.tiles"), tiles_max)?;
                    unpack_tiles(&blob, &mut layer.tiles).ok_or_else(damaged)?;
                } else {
                    let blob = read_entry(zip, &format!("layers/{id}.lz4"), len as u64 + 1024)?;
                    let rgba = unpack(&blob, len).ok_or_else(damaged)?;
                    layer.tiles = Tiles::from_rgba(size.0, size.1, &rgba, layer.tiles.fill());
                }
                Node::Layer(Layer { id, name, visible, opacity: opacity.clamp(0.0, 1.0), blend: blend_from(&blend), locked, props: props.props(), ..layer })
            }
            NodeJson::Folder { id, name, visible, opacity, blend, locked, collapsed, children, props, animation, frame, group } => Node::Folder(Folder {
                kind: match (animation, frame, group) {
                    (Some(a), None, false) => FolderKind::Animation(a.anim()),
                    (None, Some(n), false) if (1..=aurastudio_layers::anim::MAX_FRAME).contains(&n) => FolderKind::Frame(n),
                    (None, None, true) => FolderKind::Group,
                    (None, None, false) => FolderKind::Plain,
                    _ => return Err("damaged file (a folder's animation)".into()),
                },
                id, name, visible, opacity: opacity.clamp(0.0, 1.0),
                blend: if blend == PASS_THROUGH { FolderBlend::PassThrough } else { FolderBlend::Isolated(blend_from(&blend)) },
                locked, collapsed,
                children: build(children, zip, size, version)?,
                props: props.props(),
            }),
            NodeJson::Filter { id, name, visible, opacity, locked, filter, props } => {
                let kind = aurastudio_filters::GROUPS.iter().flat_map(|g| g.1.iter()).find(|k| k.name() == filter.kind)
                    .ok_or_else(|| format!("unknown filter \"{}\" (made by a newer version of AuraStudio?)", filter.kind))?;
                // Values past what this version knows are dropped, missing
                // ones get their default (Filter::v checks the ranges).
                let mut values = kind.defaults();
                for (v, f) in values.iter_mut().zip(&filter.values) { if f.is_finite() { *v = *f; } }
                Node::Filter(FilterLayer {
                    id, name, visible, opacity: opacity.clamp(0.0, 1.0), locked, props: props.props(),
                    filter: Filter { kind: *kind, values, color: filter.color },
                })
            }
        })).collect()
    }
    let root = build(json.layers, &mut zip, (w, h), version)?;
    let layers = LayerStack::from_parts(w, h, root, json.active).map_err(|e| format!("damaged file ({e})"))?;

    // History: a step that can't be read is dropped, the document still
    // opens (it is only undo data).
    let mut steps = |kind: &str, list: Vec<StepJson>| -> Vec<SavedStep> {
        list.into_iter().enumerate().filter_map(|(i, s)| {
            let [x0, y0, x1, y1] = s.rect;
            if x0 >= x1 || y0 >= y1 || x1 > w || y1 > h { return None; }
            let data = read_entry(&mut zip, &format!("history/{kind}-{i}.lz4"), len as u64 + 1024).ok()?;
            Some(SavedStep { label: s.label, layer_id: s.layer, rect: (x0, y0, x1, y1), data })
        }).collect()
    };
    let undo = steps("undo", json.undo);
    let redo = steps("redo", json.redo);
    let mut doc = Document::from_parts(id, name, layers, History::import(DEFAULT_BUDGET, undo, redo));
    if json.dpi.is_finite() && (1.0..=10_000.0).contains(&json.dpi) { doc.dpi = json.dpi; }
    doc.kind = kind;
    let active = doc.layers.active_id();
    doc.layers.set_time(json.frame);
    doc.layers.set_active(active);
    Ok(Opened { doc, origin: json.origin })
}

/// Open an .aurs file.
pub fn open(path: &Path, id: DocId, max_side: u32) -> Result<Opened, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut opened = read(io::BufReader::new(file), id, &doc_name(path), max_side)?;
    opened.doc.path = Some(path.to_path_buf());
    Ok(opened)
}

/// The file an autosave belongs to, read without loading the layers.
pub fn peek_origin(path: &Path) -> Option<PathBuf> {
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).ok()?;
    let json: DocJson = serde_json::from_slice(&read_entry(&mut zip, "document.json", MAX_JSON).ok()?).ok()?;
    json.origin
}

/// Can Open read this file (by its extension)?
pub fn can_open(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ext == EXTENSION || IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Small picture of a file for the start screen, at most `max` px: the
/// preview inside an .aurs file, or the image itself shrunk.
pub fn thumbnail(path: &Path, max: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION)) {
        let file = std::fs::File::open(path).ok()?;
        let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).ok()?;
        let png = read_entry(&mut zip, "preview.png", 16 << 20).ok()?;
        image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?
    } else {
        let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(1 << 30);
        reader.limits(limits);
        reader.decode().ok()?
    };
    let img = img.into_rgba8();
    let (w, h) = img.dimensions();
    Some(downscale(img.as_raw(), w, h, max))
}

/// Bytes of one archive entry, at most `limit` (a damaged size can't make
/// it allocate more).
fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, limit: u64) -> Result<Vec<u8>, String> {
    let entry = zip.by_name(name).map_err(|_| format!("damaged file ({name} missing)"))?;
    if entry.size() > limit { return Err(format!("damaged file ({name} too big)")); }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry.take(limit).read_to_end(&mut out).map_err(|_| format!("damaged file ({name} unreadable)"))?;
    Ok(out)
}

/// A layer's stored tiles: their count (u32), then for each its column
/// and row (u32 each) and its pixels, LZ4 with their size first.
fn pack_tiles(tiles: &Tiles) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(tiles.stored() as u32).to_le_bytes());
    for (tx, ty, t) in tiles.iter() {
        out.extend_from_slice(&tx.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        let packed = lz4_flex::compress_prepend_size(&t[..]);
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&packed);
    }
    out
}

/// Read `pack_tiles` data into `tiles` (None if damaged).
fn unpack_tiles(blob: &[u8], tiles: &mut Tiles) -> Option<()> {
    let mut at = 0usize;
    let u32_at = |at: &mut usize| -> Option<u32> {
        let v = u32::from_le_bytes(blob.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(v)
    };
    let count = u32_at(&mut at)?;
    let max = tiles.width().div_ceil(TILE) * tiles.height().div_ceil(TILE);
    if count > max { return None; }
    for _ in 0..count {
        let (tx, ty, n) = (u32_at(&mut at)?, u32_at(&mut at)?, u32_at(&mut at)? as usize);
        if tx >= tiles.width().div_ceil(TILE) || ty >= tiles.height().div_ceil(TILE) { return None; }
        let data = unpack(blob.get(at..at.checked_add(n)?)?, TILE_BYTES)?;
        at += n;
        tiles.set_tile(tx, ty, Some(data.into_boxed_slice().try_into().ok()?));
    }
    (at == blob.len()).then_some(())
}

/// LZ4 data with its size first, expected to unpack to exactly `len`
/// bytes.
fn unpack(blob: &[u8], len: usize) -> Option<Vec<u8>> {
    let size = u32::from_le_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    if size != len { return None; }
    let mut out = vec![0u8; len];
    let n = lz4_flex::block::decompress_into(&blob[4..], &mut out).ok()?;
    (n == len).then_some(out)
}

/// Document name shown for a file: its name without extension.
pub fn doc_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
}

// --- Images ---------------------------------------------------------------

/// Open an image (PNG, JPG...) as a new document: the picture on a layer
/// named after the file, over the locked white Paper.
pub fn import_image(path: &Path, id: DocId, max_side: u32) -> Result<Document, String> {
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?
        .with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits.max_alloc = Some(max_side as u64 * max_side as u64 * 4 * 2);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => format!("image bigger than this GPU allows ({max_side} px)"),
        e => e.to_string(),
    })?.into_rgba8();
    let (w, h) = img.dimensions();
    let name = doc_name(path);
    let layers = aurastudio_layers::LayerStack::from_image(w, h, &name, img.as_raw());
    let doc = Document::from_parts(id, &name, layers, aurastudio_document::History::new(aurastudio_document::DEFAULT_BUDGET));
    Ok(doc)
}

/// Export the merged image (`merged`: canvas-sized RGBA) to PNG or JPG,
/// chosen by the file extension.
pub fn export_image(merged: &[u8], width: u32, height: u32, path: &Path) -> Result<(), String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let img = image::RgbaImage::from_raw(width, height, merged.to_vec()).ok_or("wrong image size")?;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut out = io::BufWriter::new(file);
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
                .encode_image(&rgb).map_err(|e| e.to_string())?;
        }
        "png" => img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?,
        _ => return Err(format!("can't export .{ext} (PNG or JPG)")),
    }
    out.flush().map_err(|e| e.to_string())
}

/// `rgba` (w × h) shrunk to fit in `max` × `max`, averaging the pixels.
fn downscale(rgba: &[u8], w: u32, h: u32, max: u32) -> (u32, u32, Vec<u8>) {
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let mut out = Vec::with_capacity(tw as usize * th as usize * 4);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    for c in 0..4 { sum[c] += rgba[i + c] as u32; }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    (tw, th, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_canvas::PaintSurface;

    fn paint(doc: &mut Document, id: u64, rect: (u32, u32, u32, u32), rgba: [u8; 4]) {
        assert!(doc.layers.begin_stroke(id));
        doc.layers.layer_surface(id).unwrap().prepare(rect);
        let n = ((rect.2 - rect.0) * (rect.3 - rect.1)) as usize;
        assert!(doc.layers.set_layer_rect(id, rect, &rgba.repeat(n)));
        doc.commit_stroke(id, rect);
        doc.layers.end_stroke();
    }

    /// How long a snapshot holds the document: an A4 page at 300 dpi
    /// with two fully painted layers. The interface waits that long.
    #[test]
    #[ignore]
    fn snapshot_time_on_a4() {
        let (w, h) = (2480, 3508);
        let mut doc = Document::new(DocId(1), "a4", w, h);
        let mut noise = 1u32;
        for _ in 0..2 {
            let id = doc.layers.add_layer("Paint");
            assert!(doc.layers.begin_stroke(id));
            doc.layers.layer_surface(id).unwrap().prepare((0, 0, w, h));
            let px: Vec<u8> = (0..w * h * 4).map(|_| { noise = noise.wrapping_mul(1664525).wrapping_add(1013904223); (noise >> 24) as u8 | 1 }).collect();
            assert!(doc.layers.set_layer_rect(id, (0, 0, w, h), &px));
            doc.commit_stroke(id, (0, 0, w, h));
            doc.layers.end_stroke();
        }
        let t = std::time::Instant::now();
        let snap = snapshot(&doc, None);
        println!("snapshot: {:.0} ms ({} layers)", t.elapsed().as_secs_f64() * 1000.0, snap.layers.len());
    }

    fn sample_doc() -> (Document, u64, u64) {
        let mut doc = Document::new(DocId(1), "t", 96, 64);
        doc.dpi = 600.0;
        let ink = doc.layers.active_id();
        paint(&mut doc, ink, (10, 10, 40, 30), [200, 10, 10, 255]);
        let folder = doc.layers.add_folder("Shading");
        let shade = doc.layers.add_layer("Shade");
        if let Some(Node::Layer(l)) = doc.layers.node_mut(shade) {
            l.blend = BlendMode::Multiply;
            l.opacity = 0.5;
        }
        if let Some(Node::Folder(f)) = doc.layers.node_mut(folder) {
            f.blend = FolderBlend::Isolated(BlendMode::Screen);
            f.visible = false;
        }
        paint(&mut doc, shade, (0, 0, 20, 20), [0, 0, 255, 128]);
        paint(&mut doc, shade, (30, 30, 90, 60), [0, 255, 0, 255]);
        doc.undo();
        (doc, ink, shade)
    }

    fn roundtrip(doc: &Document) -> Opened {
        let mut buf = io::Cursor::new(Vec::new());
        write(&snapshot(doc, None), &mut buf).unwrap();
        buf.set_position(0);
        read(buf, DocId(2), "t", 16384).unwrap()
    }

    fn pixels(doc: &Document, id: u64) -> Vec<u8> {
        let Some(Node::Layer(l)) = doc.layers.node(id) else { panic!() };
        l.tiles.to_rgba()
    }

    #[test]
    fn save_and_open_give_the_same_document() {
        let (doc, ink, shade) = sample_doc();
        let mut back = roundtrip(&doc).doc;
        assert_eq!((back.layers.width, back.layers.height, back.dpi), (96, 64, 600.0));
        assert_eq!(back.layers.layer_count(), doc.layers.layer_count());
        assert_eq!(back.layers.active_id(), doc.layers.active_id());
        for id in doc.layers.layer_ids() { assert_eq!(pixels(&back, id), pixels(&doc, id)); }
        let Some(Node::Layer(l)) = back.layers.node(shade) else { panic!() };
        assert_eq!((l.blend, l.opacity), (BlendMode::Multiply, 0.5));
        let Some(Node::Folder(f)) = back.layers.node(shade - 1) else { panic!() };
        assert_eq!((f.blend, f.visible, f.name.as_str()), (FolderBlend::Isolated(BlendMode::Screen), false, "Shading"));
        let Some(Node::Layer(paper)) = back.layers.root.first() else { panic!() };
        assert!(paper.locked && paper.background);
        assert!(!back.is_modified());

        // The history came along: redo, then undo back to before the
        // strokes.
        assert_eq!(back.history().redo_len(), 1);
        back.redo().unwrap();
        assert_eq!(pixels(&back, shade)[((40 * 96 + 50) * 4) as usize..][..4], [0, 255, 0, 255]);
        back.undo().unwrap();
        back.undo().unwrap();
        back.undo().unwrap();
        assert_eq!(pixels(&back, ink)[((20 * 96 + 20) * 4) as usize + 3], 0);
        // New layers get ids the file didn't use.
        let new = back.layers.add_layer("new");
        assert!(!doc.layers.layer_ids().contains(&new));
    }

    #[test]
    fn filter_layers_labels_and_marks_come_back() {
        use aurastudio_filters::{Filter, Kind};
        let (mut doc, ink, shade) = sample_doc();
        {
            let p = doc.layers.node_mut(ink).unwrap().props_mut();
            p.label = 6;
            p.reference = true;
        }
        doc.layers.node_mut(shade).unwrap().props_mut().clip = true;
        doc.layers.node_mut(shade - 1).unwrap().props_mut().draft = true;
        doc.layers.set_active(ink);
        let blur = Filter { values: vec![3.5], ..Filter::new(Kind::GaussianBlur) };
        let f = doc.layers.add_filter_layer(blur.clone());
        *doc.layers.node_mut(f).unwrap().opacity_mut() = 0.25;
        let back = roundtrip(&doc).doc;
        assert_eq!(back.layers.node(ink).unwrap().props(), Props { label: 6, reference: true, ..Props::default() });
        assert!(back.layers.node(shade).unwrap().props().clip);
        assert!(back.layers.node(shade - 1).unwrap().props().draft);
        let Some(Node::Filter(fl)) = back.layers.node(f) else { panic!("filter layer lost") };
        assert_eq!((&fl.filter, fl.opacity, fl.name.as_str()), (&blur, 0.25, "Gaussian blur"));
        // The merged image is the same.
        assert_eq!(back.layers.composite_rect((0, 0, 96, 64)), doc.layers.composite_rect((0, 0, 96, 64)));
    }

    /// Everything a node keeps, its pixels as a hash.
    fn describe(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            out.push(match n {
                Node::Layer(l) => {
                    let h = l.tiles.to_rgba().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
                    format!("layer {} {:?} vis={} op={} {:?} lock={} bg={} {:?} px={h:x}", l.id, l.name, l.visible, l.opacity, l.blend, l.locked, l.background, l.props)
                }
                Node::Filter(f) => format!("filter {} {:?} vis={} op={} lock={} {:?} {:?}", f.id, f.name, f.visible, f.opacity, f.locked, f.props, f.filter),
                Node::Folder(f) => format!("folder {} {:?} vis={} op={} {:?} lock={} fold={} {:?} {:?} [{}]", f.id, f.name, f.visible, f.opacity, f.blend, f.locked, f.collapsed, f.props, f.kind, f.children.len()),
            });
            if let Node::Folder(f) = n { describe(&f.children, out); }
        }
    }

    fn rows(l: &LayerStack) -> Vec<String> {
        l.rows().iter().map(|r| format!("{} {} {} {} {} {} {} {} {:?}", r.id, r.depth, r.is_folder, r.collapsed, r.locked_by_parent, r.clipped, r.reference_by_parent, r.draft_by_parent, r.below)).collect()
    }

    /// Every setting of every kind of node, folders three deep, every
    /// blend mode, label and filter: all of it comes back from the file.
    #[test]
    fn every_layer_setting_comes_back() {
        use aurastudio_filters::GROUPS;
        let (w, h) = (130, 70);
        let mut seed = 7u32;
        let mut rnd = move || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); seed >> 8 };
        let mut id = 0;
        let mut next = || { id += 1; id };
        let layer = |id: u64, name: &str, background: bool, rnd: &mut dyn FnMut() -> u32| {
            let mut l = Layer::new(w, h, background);
            l.id = id;
            l.name = name.into();
            // Some tiles painted, the others left to the fill.
            let fill = l.tiles.fill();
            let px: Vec<u8> = (0..w * h * 4).map(|i| if (i / 4 % w) < 70 { (rnd() & 255) as u8 } else { fill[(i % 4) as usize] }).collect();
            l.tiles = Tiles::from_rgba(w, h, &px, fill);
            l
        };
        let props = |label: u8, clip, reference, draft| Props { label, clip, reference, draft };

        let mut paper = layer(next(), "Paper", true, &mut rnd);
        paper.locked = true;
        let mut lines = layer(next(), "Lines", false, &mut rnd);
        lines.props = props(1, false, true, false);
        let base = layer(next(), "Base", false, &mut rnd);
        let mut shadow = layer(next(), "Shadow", false, &mut rnd);
        (shadow.blend, shadow.opacity, shadow.props) = (BlendMode::Multiply, 0.33, props(3, true, false, false));
        let clip_blur = FilterLayer {
            id: next(), name: "Soft".into(), visible: false, opacity: 0.6, locked: true, props: props(4, true, false, false),
            filter: Filter { values: vec![2.75], ..Filter::new(aurastudio_filters::Kind::GaussianBlur) },
        };
        let mut sketch = layer(next(), "Sketch", false, &mut rnd);
        (sketch.visible, sketch.locked, sketch.props) = (false, true, props(5, false, true, true));
        let odd = layer(next(), "Ink é 漢字 🎨 \"quoted\"", false, &mut rnd);
        let deep = Folder { id: next(), name: "Deep".into(), visible: true, opacity: 0.9, blend: FolderBlend::Isolated(BlendMode::Screen),
            locked: false, collapsed: true, children: vec![Node::Layer(odd)], props: props(6, false, false, false), kind: FolderKind::Plain };
        let sketch_id = sketch.id;
        let nested = Folder { id: next(), name: "Nested".into(), visible: false, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: true, collapsed: false, children: vec![Node::Layer(sketch), Node::Folder(deep)], props: props(0, false, false, true), kind: FolderKind::Plain };
        let colors = Folder { id: next(), name: "Colors".into(), visible: true, opacity: 0.7, blend: FolderBlend::Isolated(BlendMode::Multiply),
            locked: false, collapsed: true, children: vec![Node::Layer(base), Node::Layer(shadow), Node::Filter(clip_blur), Node::Folder(nested)],
            props: props(2, false, false, false), kind: FolderKind::Plain };
        let mut root = vec![Node::Layer(paper), Node::Layer(lines), Node::Folder(colors)];
        for (i, mode) in BlendMode::ALL.iter().enumerate() {
            let mut l = layer(next(), mode.name(), false, &mut rnd);
            (l.blend, l.opacity) = (*mode, (i as f32 + 1.0) / 14.0);
            l.props.label = (i % LABELS.len()) as u8;
            l.props.clip = i % 3 == 1;
            root.push(Node::Layer(l));
        }
        for kind in GROUPS.iter().flat_map(|g| g.1.iter()) {
            let mut f = Filter::new(*kind);
            for v in f.values.iter_mut() { *v *= 1.25; }
            f.color = [(rnd() & 255) as u8, 40, 200];
            root.push(Node::Filter(FilterLayer { id: next(), name: kind.name().into(), visible: true, opacity: 0.5, locked: false, props: Props::default(), filter: f }));
        }
        let mut doc = Document::new(DocId(1), "t", w, h);
        doc.layers = LayerStack::from_parts(w, h, root, sketch_id).unwrap();
        doc.dpi = 350.0;
        doc.kind = Kind::Animation;

        let before = { let mut v = Vec::new(); describe(&doc.layers.root, &mut v); v };
        let back = roundtrip(&doc).doc;
        let after = { let mut v = Vec::new(); describe(&back.layers.root, &mut v); v };
        for (a, b) in before.iter().zip(&after) { assert_eq!(a, b); }
        assert_eq!(before.len(), after.len());
        assert_eq!(rows(&back.layers), rows(&doc.layers));
        assert_eq!((back.layers.active_id(), back.dpi, back.layers.width, back.layers.height), (sketch_id, 350.0, w, h));
        assert_eq!(back.kind, Kind::Animation);
        assert_eq!(back.layers.composite_rect((0, 0, w, h)), doc.layers.composite_rect((0, 0, w, h)));
    }

    /// Where the time goes opening a 2000×3000 JPG (OPEN_JPG=path).
    #[test]
    #[ignore]
    fn open_speed() {
        let path = std::path::PathBuf::from(std::env::var("OPEN_JPG").unwrap());
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let img = image::ImageReader::open(&path).unwrap().with_guessed_format().unwrap().decode().unwrap().into_rgba8();
            let decode = t.elapsed();
            let (w, h) = img.dimensions();
            let t = std::time::Instant::now();
            let layers = LayerStack::from_image(w, h, "x", img.as_raw());
            let tiles = t.elapsed();
            let t = std::time::Instant::now();
            let _ = import_image(&path, DocId(1), 16384).unwrap();
            println!("decode {:.0} ms, tiles {:.0} ms, import_image {:.0} ms ({} layers)", decode.as_secs_f64() * 1e3, tiles.as_secs_f64() * 1e3, t.elapsed().as_secs_f64() * 1e3, layers.layer_count());
        }
    }

    #[test]
    fn only_the_newest_undo_steps_are_kept() {
        let mut doc = Document::new(DocId(1), "t", 16, 16);
        let id = doc.layers.active_id();
        for i in 0..SAVED_UNDO_STEPS as u32 + 10 {
            paint(&mut doc, id, (i % 16, 0, i % 16 + 1, 1), [i as u8, 0, 0, 255]);
        }
        let back = roundtrip(&doc).doc;
        assert_eq!(back.history().undo_len(), SAVED_UNDO_STEPS);
    }

    #[test]
    fn other_files_are_refused() {
        assert!(read(io::Cursor::new(b"hello".to_vec()), DocId(1), "x", 16384).is_err());
        // A zip that isn't ours.
        let mut buf = io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file("mimetype", SimpleFileOptions::default()).unwrap();
        z.write_all(b"application/zip").unwrap();
        z.finish().unwrap();
        buf.set_position(0);
        assert!(read(buf, DocId(1), "x", 16384).is_err());
        // Too big for the GPU.
        let (doc, ..) = sample_doc();
        let mut buf = io::Cursor::new(Vec::new());
        write(&snapshot(&doc, None), &mut buf).unwrap();
        buf.set_position(0);
        assert!(read(buf, DocId(1), "x", 64).is_err());
    }

    /// Animation folders come back: template, chosen cel, frames with
    /// their cels and pixels, the current frame; such a file says
    /// version 4, others still 3.
    #[test]
    fn animation_folders_come_back() {
        let mut doc = Document::new(DocId(1), "a", 64, 48);
        doc.kind = Kind::Animation;
        assert_eq!(snapshot(&doc, None).json.version, PLAIN_VERSION);
        let a = doc.layers.add_animation_folder("Perso A");
        doc.layers.add_cel(a, 1, Cel { locked: true, ..Cel::new("Layout", false) });
        doc.layers.set_cel(a, 2);
        for n in [1, 4] {
            doc.layers.set_time(n);
            let cel = doc.begin_paint().unwrap();
            paint(&mut doc, cel, (n * 5, 5, n * 5 + 10, 20), [n as u8 * 40, 0, 0, 255]);
        }
        doc.layers.set_time(3);
        doc.layers.check_animation().unwrap();
        let snap = snapshot(&doc, None);
        assert_eq!(snap.json.version, FORMAT_VERSION);
        let back = roundtrip(&doc).doc;
        back.layers.check_animation().unwrap();
        assert_eq!(back.layers.time(), 3);
        assert_eq!(back.layers.anim(a), doc.layers.anim(a));
        assert_eq!(back.layers.frames(a), doc.layers.frames(a));
        assert_eq!(back.layers.active_id(), doc.layers.active_id());
        let mut x = Vec::new();
        describe(&back.layers.root, &mut x);
        let mut y = Vec::new();
        describe(&doc.layers.root, &mut y);
        assert_eq!(x, y);
        assert_eq!(back.layers.composite_rect((0, 0, 64, 48)), doc.layers.composite_rect((0, 0, 64, 48)));
    }

    /// Groups (with the folders in them) and the cel marked as the
    /// reference come back; a file from before, where the chosen cel was
    /// called "sub", still opens on the same cel.
    #[test]
    fn groups_and_cel_references_come_back() {
        let mut doc = Document::new(DocId(1), "g", 64, 48);
        doc.kind = Kind::Animation;
        let a = doc.layers.add_animation_folder("A");
        let b = doc.layers.add_animation_folder("B");
        let c = doc.layers.add_animation_folder("C");
        let g = doc.layers.group_animation_folders(&[a, b], "Chars").unwrap();
        // The reference moves to the Douga; the chosen cel is the Genga.
        doc.layers.set_cel_reference(c, 2, true);
        doc.layers.set_cel(c, 3);
        doc.layers.check_animation().unwrap();
        assert_eq!(doc.layers.group_of(a), Some(g));
        let back = roundtrip(&doc).doc;
        back.layers.check_animation().unwrap();
        assert_eq!(back.layers.columns(None), doc.layers.columns(None));
        assert_eq!(back.layers.group_members(g), doc.layers.group_members(g));
        assert_eq!(back.layers.group_of(b), Some(g));
        assert_eq!(back.layers.anim(c), doc.layers.anim(c));
        assert!(back.layers.anim(c).unwrap().template[2].reference);
        assert_eq!(back.layers.anim(c).unwrap().cel, 3);
        // An older file: "sub" where "cel" is.
        fn rename(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(o) => {
                    if o.contains_key("template") {
                        if let Some(x) = o.remove("cel") { o.insert("sub".into(), x); }
                    }
                    o.values_mut().for_each(rename);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(rename),
                _ => {}
            }
        }
        let mut snap = snapshot(&doc, None);
        let mut json = serde_json::to_value(&snap.json).unwrap();
        rename(&mut json);
        assert!(json.to_string().contains("\"sub\""));
        snap.json = serde_json::from_value(json).unwrap();
        let mut buf = io::Cursor::new(Vec::new());
        write(&snap, &mut buf).unwrap();
        buf.set_position(0);
        let old = read(buf, DocId(3), "t", 16384).unwrap().doc;
        assert_eq!(old.layers.anim(c).unwrap().cel, 3);
    }

    /// The document type comes back; files from before it open as
    /// Illustration; a type this version doesn't know is refused.
    #[test]
    fn the_document_type_comes_back() {
        let (mut doc, ..) = sample_doc();
        for kind in Kind::ALL {
            doc.kind = kind;
            assert_eq!(roundtrip(&doc).doc.kind, kind);
        }
        let mut snap = snapshot(&doc, None);
        let mut json = serde_json::to_value(&snap.json).unwrap();
        json.as_object_mut().unwrap().remove("kind");
        snap.json = serde_json::from_value(json).unwrap();
        assert_eq!(snap.json.kind, "Illustration");
        snap.json.kind = "Storyboard".into();
        let mut buf = io::Cursor::new(Vec::new());
        write(&snap, &mut buf).unwrap();
        buf.set_position(0);
        assert!(read(buf, DocId(1), "x", 16384).err().is_some_and(|e| e.contains("newer version")));
    }

    /// Damaged files (bytes changed or cut) give an error or a valid
    /// document, never a panic or a huge allocation.
    #[test]
    fn damaged_files_never_crash() {
        let (doc, ..) = sample_doc();
        let mut buf = io::Cursor::new(Vec::new());
        write(&snapshot(&doc, None), &mut buf).unwrap();
        let good = buf.into_inner();
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
            if let Ok(o) = read(io::Cursor::new(bad), DocId(1), "x", 4096) {
                assert!(o.doc.layers.layer_count() > 0);
            }
        }
    }

    #[test]
    fn save_writes_through_a_temporary_file() {
        let dir = std::env::temp_dir().join(format!("aurs-test-{}", std::process::id()));
        let path = dir.join("drawing.aurs");
        let (doc, ..) = sample_doc();
        save(&snapshot(&doc, Some(Path::new("/x/orig.aurs"))), &path).unwrap();
        save(&snapshot(&doc, None), &path).unwrap();
        let o = open(&path, DocId(3), 16384).unwrap();
        assert_eq!(o.doc.name, "drawing");
        assert_eq!(o.doc.path.as_deref(), Some(path.as_path()));
        assert!(o.origin.is_none());
        assert!(!path.with_extension("aurs.saving").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn images_import_and_export() {
        let dir = std::env::temp_dir().join(format!("aurs-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rgba = vec![0u8; 30 * 20 * 4];
        for (i, p) in rgba.chunks_mut(4).enumerate() { p.copy_from_slice(&[i as u8, 50, 200, 255]); }
        let png = dir.join("photo.png");
        export_image(&rgba, 30, 20, &png).unwrap();
        let doc = import_image(&png, DocId(1), 16384).unwrap();
        assert_eq!((doc.layers.width, doc.layers.height, doc.name.as_str()), (30, 20, "photo"));
        // The picture is the bottom layer (no Paper), an empty layer above
        // it is active.
        let ids = doc.layers.layer_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(pixels(&doc, ids[0]), rgba);
        assert_eq!(doc.layers.active_id(), ids[1]);
        assert!(!doc.layers.node(ids[0]).unwrap().locked());
        export_image(&rgba, 30, 20, &dir.join("photo.jpg")).unwrap();
        assert_eq!(import_image(&dir.join("photo.jpg"), DocId(1), 16384).unwrap().layers.width, 30);
        assert!(import_image(&png, DocId(1), 16).is_err());
        assert!(export_image(&rgba, 30, 20, &dir.join("x.gif")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preview_fits_and_averages() {
        let (w, h, px) = downscale(&[255u8; 2000 * 1000 * 4], 2000, 1000, 512);
        assert_eq!((w, h), (512, 256));
        assert!(px.iter().all(|&v| v == 255));
        assert_eq!(downscale(&[9u8; 4 * 4 * 4], 4, 4, 512).0, 4);
    }
}

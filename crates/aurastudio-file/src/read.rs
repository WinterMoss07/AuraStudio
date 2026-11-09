//! Reading an archive of any version, everything checked.

use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use aurastudio_core::DocType;
use aurastudio_filters::{Filter, Kind};
use aurastudio_layers::{FilterLayer, Folder, FolderBlend, FolderKind, Layer, LayerStack, MaskLayer, Node, MAX_FRAME};
use aurastudio_raster::{legacy, Image, Mask, MAX_TILE, MIN_TILE};

use crate::limits::*;
use crate::schema::*;
use crate::snapshot::{OnionMarks, Snapshot, Step};

fn damaged(what: impl std::fmt::Display) -> String { format!("damaged file ({what})") }

struct Ctx<'z, R: Read + Seek> {
    zip:     &'z mut zip::ZipArchive<R>,
    version: u32,
    w:       u32,
    h:       u32,
    tile:    u32,
    max:     u32,
}

impl<R: Read + Seek> Ctx<'_, R> {
    /// Largest surface entry accepted: sixteen frames of tiles (content
    /// past the frame included), plus some.
    fn surface_max(&self) -> u64 {
        let tiles = self.w.div_ceil(self.tile) as u64 * self.h.div_ceil(self.tile) as u64;
        Image::saved_max(tiles * 16 + 4096, self.tile)
    }

    fn layer_surface(&mut self, id: u64, background: bool) -> Result<Image, String> {
        let fill = LayerStack::clear_pixel(background);
        let (w, h) = (self.w, self.h);
        let entry = layer_entry(self.version, id);
        let bad = || damaged(format!("layer {id}"));
        match self.version {
            1 => {
                let len = w as usize * h as usize * 4;
                let blob = read_entry(self.zip, &entry, len as u64 + 1024)?;
                let raw = unpack(&blob, len).ok_or_else(bad)?;
                let mut s = Image::new(w, h, self.tile, fill);
                s.write(s.rect(), raw.as_chunks::<4>().0);
                s.compact(s.rect().snapped_out(self.tile));
                Ok(s)
            }
            2..=5 => {
                let t = legacy::TILE as u64;
                let max = w.div_ceil(legacy::TILE) as u64 * h.div_ceil(legacy::TILE) as u64 * (t * t * 4 + t * t * 4 / 255 + 32) + 4;
                let blob = read_entry(self.zip, &entry, max)?;
                legacy::load_layer(&blob, w, h, fill, self.tile).ok_or_else(bad)
            }
            _ => {
                let blob = read_entry(self.zip, &entry, self.surface_max())?;
                let s = Image::load(&blob, self.max).ok_or_else(bad)?;
                if (s.width(), s.height()) != (w, h) { return Err(bad()); }
                Ok(s)
            }
        }
    }

    fn mask(&mut self, entry: &str) -> Result<Mask, String> {
        let blob = read_entry(self.zip, entry, self.surface_max())?;
        let m = Mask::load(&blob, self.max).ok_or_else(|| damaged(entry))?;
        if (m.width(), m.height()) != (self.w, self.h) { return Err(damaged(entry)); }
        Ok(m)
    }

    fn build(&mut self, nodes: Vec<NodeJson>, depth: usize) -> Result<Vec<Node>, String> {
        if depth > 64 { return Err(damaged("folders too deep")); }
        nodes.into_iter().map(|n| Ok(match n {
            NodeJson::Layer { id, name, visible, opacity, blend, locked, background, props } => {
                let surface = self.layer_surface(id, background)?;
                Node::Layer(Layer { id, name, visible, opacity: unit(opacity), blend: blend_from(&blend), locked, surface, background, props: props.props() })
            }
            NodeJson::Folder { id, name, visible, opacity, blend, locked, collapsed, children, props, animation, sheet, group } => Node::Folder(Folder {
                kind: match (animation, sheet, group) {
                    (Some(a), None, false) => FolderKind::Animation(a.anim()),
                    (None, Some(n), false) if (1..=MAX_FRAME).contains(&n) => FolderKind::Sheet(n),
                    (None, None, true) => FolderKind::Group,
                    (None, None, false) => FolderKind::Plain,
                    _ => return Err(damaged("a folder's animation")),
                },
                id, name, visible, opacity: unit(opacity),
                blend: if blend == PASS_THROUGH { FolderBlend::PassThrough } else { FolderBlend::Isolated(blend_from(&blend)) },
                locked, collapsed,
                children: self.build(children, depth + 1)?,
                props: props.props(),
            }),
            NodeJson::Filter { id, name, visible, opacity, locked, filter, props } => {
                let kind = Kind::all().find(|k| k.name() == filter.kind)
                    .ok_or_else(|| format!("unknown filter \"{}\" (made by a newer version of AuraStudio?)", filter.kind))?;
                // Values past what this version knows are dropped, missing
                // ones get their default (Filter::v checks the ranges).
                let mut values = kind.defaults();
                for (v, f) in values.iter_mut().zip(&filter.values) { if f.is_finite() { *v = *f; } }
                Node::Filter(FilterLayer { id, name, visible, opacity: unit(opacity), locked, props: props.props(), filter: Filter { kind, values, color: filter.color } })
            }
            NodeJson::Mask { id, name, visible, locked, props } => {
                let mask = self.mask(&format!("masks/{id}.surface"))?;
                Node::Mask(MaskLayer { id, name, visible, locked, props: props.props(), mask })
            }
        })).collect()
    }
}

fn unit(v: f32) -> f32 { if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 } }

/// Read an .aurs archive of any version. `max_side`: biggest frame side
/// accepted (the GPU limit).
pub fn read<R: Read + Seek>(input: R, max_side: u32) -> Result<Snapshot, String> {
    aurastudio_core::zone!("file read");
    let mut zip = zip::ZipArchive::new(input).map_err(|_| "not an AuraStudio file (not an archive)".to_string())?;
    if read_entry(&mut zip, "mimetype", 256)? != MIMETYPE.as_bytes() { return Err("not an AuraStudio file".into()); }
    let json: DocJson = serde_json::from_slice(&read_entry(&mut zip, "document.json", MAX_JSON)?).map_err(|e| damaged(format!("document.json: {e}")))?;
    if json.format != "aurastudio" { return Err("not an AuraStudio file".into()); }
    if json.version == 0 { return Err(damaged("version")); }
    if json.version > FORMAT_VERSION { return Err("made by a newer version of AuraStudio".into()); }
    let json = migrate(json);
    // (A type this version doesn't know may hold what it can't show.)
    let kind = DocType::from_name(&json.kind).ok_or("made by a newer version of AuraStudio (unknown document type)")?;
    let (w, h) = (json.width, json.height);
    if w == 0 || h == 0 || w > max_side || h > max_side {
        return Err(format!("canvas {w} × {h} px is bigger than this GPU allows ({max_side} px)"));
    }
    let tile = json.tile.unwrap_or(kind.default_tile_size());
    if !tile.is_power_of_two() || !(MIN_TILE..=MAX_TILE).contains(&tile) { return Err(damaged("tile size")); }
    let version = json.version;
    let mut ctx = Ctx { zip: &mut zip, version, w, h, tile, max: max_side };
    let root = ctx.build(json.layers, 0)?;
    let mut stack = LayerStack::from_parts(w, h, tile, root, json.active).map_err(damaged)?;
    stack.margin = json.margin.unwrap_or(aurastudio_layers::DEFAULT_MARGIN).min(aurastudio_layers::MAX_MARGIN);
    stack.crop_to_area();
    let selection = if json.selection { ctx.mask("selection.surface").ok() } else { None };

    // History: a step that can't be read is dropped, the document still
    // opens (it is only undo data).
    let step_max = if version < 6 { (w as u64 * h as u64 * 4 + 1024).min(MAX_STEP) } else { MAX_STEP };
    let mut steps = |kind: &str, list: Vec<StepJson>| -> Vec<Step> {
        list.into_iter().enumerate().filter_map(|(i, s)| {
            let rect = match s.rect {
                Some([x0, y0, x1, y1]) => {
                    // (Old steps: within the canvas.)
                    if x0 < 0 || y0 < 0 || x0 >= x1 || y0 >= y1 || x1 > w as i64 || y1 > h as i64 { return None; }
                    Some([x0 as i32, y0 as i32, x1 as i32, y1 as i32])
                }
                None if version < 6 => return None,
                None => None,
            };
            let data = read_entry(ctx.zip, &step_entry(version, kind, i), step_max).ok()?;
            Some(Step { label: s.label, layer: s.layer, rect, data, joined: s.joined, sheet: s.sheet })
        }).collect()
    };
    let undo = steps("undo", json.undo);
    let redo = steps("redo", json.redo);
    let profile = match json.profile {
        Some(name) => Some((name, read_entry(&mut zip, "profile.icc", MAX_ICC)?)),
        None => None,
    };
    let ok = |v: &Option<f32>| v.is_none_or(|v| v.is_finite());
    let onion = json.onion.map(|o| OnionMarks {
        enabled: o.enabled,
        mobile: o.mobile.into_iter().filter(|m| m.0 != 0 && m.0.abs() <= 1000 && ok(&m.1)).collect(),
        fixed: o.fixed.into_iter().filter(|m| (1..=MAX_FRAME).contains(&m.0) && ok(&m.1)).collect(),
    });
    let dpi = if json.dpi.is_finite() && (1.0..=10_000.0).contains(&json.dpi) { json.dpi } else { 300.0 };
    Ok(Snapshot {
        stack, dpi, kind, depth: depth_from(&json.depth), frame: json.frame.clamp(1, MAX_FRAME), fps: fps_from(json.fps),
        onion, profile, selection, undo, redo, preview: None, origin: json.origin, revision: 0,
    })
}

/// Open an .aurs file.
pub fn open(path: &Path, max_side: u32) -> Result<Snapshot, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    read(io::BufReader::new(file), max_side)
}

/// The file an autosave belongs to, read without loading the layers.
pub fn peek_origin(path: &Path) -> Option<PathBuf> {
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).ok()?;
    let json: DocJson = serde_json::from_slice(&read_entry(&mut zip, "document.json", MAX_JSON).ok()?).ok()?;
    json.origin
}

/// The small merged picture inside an archive.
pub(crate) fn preview_of(path: &Path) -> Option<image::DynamicImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).ok()?;
    let png = read_entry(&mut zip, "preview.png", MAX_PREVIEW).ok()?;
    image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()
}


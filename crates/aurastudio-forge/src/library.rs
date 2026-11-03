//! The brush library: the built-in brushes (AuraStudio's drawing set,
//! PhotoCraft's General, Dry Media, Wet Media and Special Effects, the
//! erasers, the Mixer Brushes) and the brushes imported from other apps,
//! kept on disk between sessions.
//!
//! On disk (`<root>`, e.g. `%APPDATA%\AuraStudio\brushes`):
//! - `groups/<name>-<hash>.json`: one imported file's presets, their
//!   bitmaps (sampled tip, Dual Brush tip, texture) replaced by a key and a
//!   small preview;
//! - `tips/<key>.tip`: the bitmaps, deflated, 8 bits a sample when that
//!   loses nothing, else 16 (the key is the content's hash: an imported
//!   tip used by several presets is kept once).
//!
//! Opening the library reads previews only; a full tip is read when its
//! brush is picked ([`Library::load_tips`]) and kept in memory while in use,
//! so a library of thousands of big imported tips costs little. Built-ins
//! are never written: they are made again at each start. Every write goes
//! through a temporary file and a rename; a damaged file is skipped with a
//! warning, never a panic. (The layout follows PhotoCraft's preset store,
//! see NOTICE.md.)

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::brush::{BrushPreset, BrushSettings, Control, Dynamic, Pattern, PatternStyle, ShapeDynamics, Smoothing, Texture, TipShape, Transfer};
use crate::import::BrushFile;
use crate::mixer::MixerSettings;
use crate::tile::{GrayTile, StoredTile, PREVIEW_SIDE};

/// The toolbox group a brush belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind { Draw, Paint, Eraser }

/// A named brush.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name:     String,
    pub kind:     Kind,
    /// The library group: a built-in set, or the file it was imported from.
    pub group:    String,
    /// Folders inside the group, outermost first (an .abr file's own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder:   Vec<String>,
    pub settings: BrushSettings,
    #[serde(default)]
    pub builtin:  bool,
}

impl Preset {
    /// The built-in brushes, in library order.
    pub fn builtin() -> Vec<Preset> { builtin() }

    fn from_photocraft(p: BrushPreset) -> Preset {
        let kind = kind_of(&p.brush, &p.group);
        Preset { name: p.name, kind, group: p.group, folder: p.folder, settings: p.brush, builtin: false }
    }
}

/// The group a brush goes in when nothing says otherwise.
fn kind_of(b: &BrushSettings, group: &str) -> Kind {
    if b.erase { Kind::Eraser } else if group == DRAWING || group == "Dry Media" { Kind::Draw } else { Kind::Paint }
}

const DRAWING: &str = "Drawing";
const ERASERS: &str = "Erasers";
const MIXER: &str = "Mixer";

/// Built-in groups, in library order.
pub const BUILTIN_GROUPS: [&str; 7] = [DRAWING, "General", "Dry Media", "Wet Media", MIXER, "Special Effects", ERASERS];

fn pressure(minimum: f32) -> Dynamic { Dynamic { minimum, ..Dynamic::controlled(Control::PenPressure) } }

fn size_dyn(minimum: f32) -> ShapeDynamics { ShapeDynamics { enabled: true, size: pressure(minimum), ..Default::default() } }

fn transfer(opacity: Option<f32>, flow: Option<f32>) -> Transfer {
    Transfer { enabled: true, opacity: opacity.map_or_else(Dynamic::default, pressure), flow: flow.map_or_else(Dynamic::default, pressure), ..Default::default() }
}

fn ours(group: &str, kind: Kind, name: &str, settings: BrushSettings) -> Preset {
    Preset { name: name.into(), kind, group: group.into(), folder: Vec::new(), settings, builtin: true }
}

/// AuraStudio's own sets (drawing and inking, the Mixer Brushes, the
/// erasers) around PhotoCraft's.
fn builtin() -> Vec<Preset> {
    let base = BrushSettings { pressure_size: false, spacing: 0.1, ..Default::default() };
    let paper = |depth: f32| Texture { enabled: true, pattern: Pattern::Procedural { style: PatternStyle::Paper, size: 128, seed: 3 }, depth, ..Default::default() };
    let d = |n: &str, b: BrushSettings| ours(DRAWING, Kind::Draw, n, b);
    let mut v = vec![
        d("Pencil", BrushSettings { size: 5.0, hardness: 0.8, spacing: 0.08, shape_dynamics: size_dyn(0.35), transfer: transfer(Some(0.0), None), texture: paper(0.45), ..base.clone() }),
        d("Pen", BrushSettings { size: 6.0, hardness: 1.0, spacing: 0.04, shape_dynamics: size_dyn(0.05), ..base.clone() }),
        d("G-Pen", BrushSettings { size: 9.0, hardness: 1.0, spacing: 0.04, shape_dynamics: size_dyn(0.0), smoothing: Smoothing { amount: 0.15, ..Default::default() }, ..base.clone() }),
        d("Ink Brush", BrushSettings { size: 14.0, hardness: 0.95, spacing: 0.04, roundness: 0.8, angle: 30.0, shape_dynamics: size_dyn(0.0), transfer: transfer(None, Some(0.5)), ..base.clone() }),
        d("Marker", BrushSettings { size: 18.0, hardness: 0.9, opacity: 0.6, spacing: 0.04, ..base.clone() }),
        d("Pixel Pencil", BrushSettings { size: 1.0, hardness: 1.0, aliased: true, spacing: 0.1, ..base.clone() }),
    ];
    v.extend(crate::presets::builtin().into_iter().map(|p| Preset { builtin: true, ..Preset::from_photocraft(p) }));
    let mixer = |wet: f32, load: f32, mix: f32, flow: f32| MixerSettings { enabled: true, wet, load, mix, flow, ..Default::default() };
    let m = |n: &str, b: BrushSettings| ours(MIXER, Kind::Paint, n, b);
    v.extend([
        m("Wet Blender", BrushSettings { size: 40.0, hardness: 0.3, spacing: 0.08, mixer: mixer(0.9, 0.0, 0.9, 0.8), shape_dynamics: size_dyn(0.3), ..base.clone() }),
        m("Oil Paint", BrushSettings { size: 30.0, hardness: 0.7, spacing: 0.06, mixer: mixer(0.5, 0.7, 0.4, 1.0), transfer: transfer(None, Some(0.2)), ..base.clone() }),
        m("Soft Smudge", BrushSettings { size: 50.0, hardness: 0.0, spacing: 0.06, mixer: mixer(1.0, 0.0, 1.0, 0.6), ..base.clone() }),
    ]);
    let e = |n: &str, b: BrushSettings| ours(ERASERS, Kind::Eraser, n, BrushSettings { erase: true, ..b });
    v.extend([
        e("Hard Eraser", BrushSettings { size: 20.0, hardness: 1.0, spacing: 0.05, ..base.clone() }),
        e("Soft Eraser", BrushSettings { size: 60.0, hardness: 0.0, flow: 0.5, ..base.clone() }),
        e("Pencil Eraser", BrushSettings { size: 8.0, hardness: 0.8, shape_dynamics: size_dyn(0.4), texture: paper(0.4), ..base.clone() }),
        e("Pixel Eraser", BrushSettings { size: 3.0, hardness: 1.0, aliased: true, ..base }),
    ]);
    // Library order: the groups as listed, each kept together.
    v.sort_by_key(|p| BUILTIN_GROUPS.iter().position(|g| *g == p.group).unwrap_or(usize::MAX));
    v
}

/// What one import added.
#[derive(Clone, Debug, Default)]
pub struct Imported {
    pub group:    String,
    pub count:    usize,
    pub warnings: Vec<String>,
}

/// The brushes: built-ins, then every imported group.
pub struct Library {
    pub presets: Vec<Preset>,
    root:        Option<PathBuf>,
    /// Full tips read so far, by key.
    tips:        Mutex<HashMap<String, Arc<GrayTile>>>,
}

impl Default for Library {
    fn default() -> Self { Self::builtin() }
}

/// Largest group file read.
const MAX_GROUP_BYTES: u64 = 64 << 20;
/// Largest tip file read.
const MAX_TIP_BYTES: u64 = 128 << 20;
const TIP_MAGIC: &[u8; 4] = b"AUTP";

#[derive(Serialize, Deserialize)]
struct GroupFile {
    group:   String,
    presets: Vec<Preset>,
}

impl Library {
    /// The built-ins only, nothing kept on disk.
    pub fn builtin() -> Self { Self { presets: builtin(), root: None, tips: Mutex::new(HashMap::new()) } }

    /// The built-ins and the groups kept in `root` (made if missing).
    /// Returns what couldn't be read.
    pub fn open(root: PathBuf) -> (Self, Vec<String>) {
        let mut lib = Self { root: Some(root.clone()), ..Self::builtin() };
        let mut warnings = Vec::new();
        let dir = root.join("groups");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect()).unwrap_or_default();
        files.sort();
        for f in files {
            match read_capped(&f, MAX_GROUP_BYTES).and_then(|b| serde_json::from_slice::<GroupFile>(&b).map_err(|e| e.to_string())) {
                Ok(g) => lib.presets.extend(g.presets.into_iter().map(|p| Preset { builtin: false, group: g.group.clone(), ..p })),
                Err(e) => warnings.push(format!("{}: {e}", f.display())),
            }
        }
        (lib, warnings)
    }

    /// The groups, in library order.
    pub fn groups(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in &self.presets { if !out.contains(&p.group.as_str()) { out.push(&p.group); } }
        out
    }

    /// Read a brush file (.abr, .brush, .brushset) into a new group named
    /// after it, kept on disk when the library has a place there.
    pub fn import(&mut self, path: &Path) -> Result<Imported, String> {
        let kind = BrushFile::of(path).ok_or("not a brush file (.abr, .brush, .brushset)")?;
        let bytes = read_capped(path, 2 << 30)?;
        let stem = path.file_stem().map_or_else(|| "Imported".into(), |s| s.to_string_lossy().into_owned());
        let group = self.free_group_name(&stem);
        let (presets, warnings) = match kind {
            BrushFile::Abr => { let i = crate::import::read_abr(&bytes, &group)?; (i.presets, i.warnings) }
            BrushFile::Procreate => {
                let i = crate::import::read_procreate(&bytes, &group)?;
                (i.presets, i.warnings)
            }
        };
        if presets.is_empty() { return Err("the file holds no brushes".into()); }
        let mut presets: Vec<Preset> = presets.into_iter().map(|p| Preset { group: group.clone(), ..Preset::from_photocraft(p) }).collect();
        if let Some(root) = self.root.clone() { self.save_group(&root, &group, &mut presets)?; }
        let count = presets.len();
        self.presets.extend(presets);
        Ok(Imported { group, count, warnings })
    }

    /// `name`, or `name (2)`... if a group has it already.
    fn free_group_name(&self, name: &str) -> String {
        let groups = self.groups();
        if !groups.contains(&name) && !BUILTIN_GROUPS.contains(&name) { return name.into(); }
        (2..).map(|n| format!("{name} ({n})")).find(|g| !groups.contains(&g.as_str())).unwrap_or_else(|| name.into())
    }

    /// Take an imported group out of the library (and off the disk).
    pub fn remove_group(&mut self, group: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|p| p.builtin || p.group != group);
        if self.presets.len() == before { return false; }
        if let Some(root) = &self.root {
            let _ = std::fs::remove_file(root.join("groups").join(group_file(group)));
            self.forget_unused_tips(root);
        }
        true
    }

    /// The full bitmaps of `b`'s stored tip, Dual Brush tip and texture,
    /// read when not in memory yet. A tip that can't be read leaves its
    /// preview in use (the stroke is coarser, nothing fails).
    pub fn load_tips(&self, b: &mut BrushSettings) {
        let Some(root) = &self.root else { return };
        let load = |t: &mut StoredTile| {
            if t.full.is_some() { return; }
            let mut cache = self.tips.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(g) = cache.get(&t.key) { t.full = Some(Arc::clone(g)); return; }
            match read_tip(&root.join("tips").join(format!("{}.tip", t.key))) {
                Ok(g) if g.width == t.width && g.height == t.height => {
                    let g = Arc::new(g);
                    cache.insert(t.key.clone(), Arc::clone(&g));
                    t.full = Some(g);
                }
                _ => {}
            }
        };
        if let TipShape::Stored(t) = &mut b.tip { load(t); }
        if let TipShape::Stored(t) = &mut b.dual_brush.tip { load(t); }
        if let Pattern::Stored(t) = &mut b.texture.pattern { load(t); }
    }

    /// Forget the full tips no brush uses now (the brush in use is `keep`).
    pub fn trim_tips(&self, keep: &BrushSettings) {
        let mut used = Vec::new();
        for t in [stored_tip(&keep.tip), stored_tip(&keep.dual_brush.tip), stored_pattern(&keep.texture.pattern)].into_iter().flatten() { used.push(t.key.clone()); }
        self.tips.lock().unwrap_or_else(PoisonError::into_inner).retain(|k, _| used.contains(k));
    }

    /// Write `presets` as the group `group`: bitmaps to tip files, the
    /// settings to the group file; the presets then hold stored tips.
    fn save_group(&self, root: &Path, group: &str, presets: &mut [Preset]) -> Result<(), String> {
        let tips = root.join("tips");
        std::fs::create_dir_all(&tips).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(root.join("groups")).map_err(|e| e.to_string())?;
        let mut cache = self.tips.lock().unwrap_or_else(PoisonError::into_inner);
        let mut store = |g: &GrayTile| -> Result<StoredTile, String> {
            let key = tip_key(g);
            let file = tips.join(format!("{key}.tip"));
            if !file.exists() { write_atomic(&file, &encode_tip(g))?; }
            let full = Arc::new(g.clone());
            cache.insert(key.clone(), Arc::clone(&full));
            Ok(StoredTile { key, width: g.width, height: g.height, preview: Arc::new(g.preview(PREVIEW_SIDE)), full: Some(full) })
        };
        for p in presets.iter_mut() {
            let b = &mut p.settings;
            if let TipShape::Sampled(g) = &b.tip { b.tip = TipShape::Stored(store(g)?); }
            if let TipShape::Sampled(g) = &b.dual_brush.tip { b.dual_brush.tip = TipShape::Stored(store(g)?); }
            if let Pattern::Tile(g) = &b.texture.pattern { b.texture.pattern = Pattern::Stored(store(g)?); }
        }
        drop(cache);
        // The file holds keys and previews (a loaded tip would write its bitmap).
        let mut kept = presets.to_vec();
        for p in &mut kept {
            let b = &mut p.settings;
            if let TipShape::Stored(t) = &mut b.tip { t.full = None; }
            if let TipShape::Stored(t) = &mut b.dual_brush.tip { t.full = None; }
            if let Pattern::Stored(t) = &mut b.texture.pattern { t.full = None; }
        }
        let file = GroupFile { group: group.into(), presets: kept };
        let json = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
        write_atomic(&root.join("groups").join(group_file(group)), &json)
    }

    /// Delete the tip files no group refers to.
    fn forget_unused_tips(&self, root: &Path) {
        let mut used: Vec<&str> = Vec::new();
        for p in &self.presets {
            let b = &p.settings;
            for t in [stored_tip(&b.tip), stored_tip(&b.dual_brush.tip), stored_pattern(&b.texture.pattern)].into_iter().flatten() { used.push(&t.key); }
        }
        let Ok(dir) = std::fs::read_dir(root.join("tips")) else { return };
        for e in dir.flatten() {
            let p = e.path();
            let Some(key) = p.file_stem().and_then(|s| s.to_str()) else { continue };
            if !used.contains(&key) { let _ = std::fs::remove_file(&p); }
        }
    }
}

fn stored_tip(t: &TipShape) -> Option<&StoredTile> { if let TipShape::Stored(s) = t { Some(s) } else { None } }

fn stored_pattern(p: &Pattern) -> Option<&StoredTile> { if let Pattern::Stored(s) = p { Some(s) } else { None } }

/// A group's file name: its name (made safe) and a hash (two names that
/// make the same safe name stay apart).
fn group_file(group: &str) -> String {
    let safe: String = group.chars().take(48).map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}-{:08x}.json", aurastudio_core::Fnv::default().str(group).finish() as u32)
}

/// A bitmap's key: the hash of its size and samples.
fn tip_key(g: &GrayTile) -> String {
    let mut h = aurastudio_core::Fnv::default();
    h.bytes(&g.width.to_le_bytes()).bytes(&g.height.to_le_bytes());
    for v in g.data.iter() { h.bytes(&v.to_le_bytes()); }
    format!("{:016x}", h.finish())
}

/// A tip file: magic, width, height, bits (8 or 16), the samples deflated.
fn encode_tip(g: &GrayTile) -> Vec<u8> {
    // 8 bits when every sample is an 8-bit level (most imported tips).
    let eight = g.data.iter().all(|v| v % 257 == 0);
    let mut raw = Vec::with_capacity(g.data.len() * if eight { 1 } else { 2 });
    for v in g.data.iter() { if eight { raw.push((v / 257) as u8) } else { raw.extend_from_slice(&v.to_le_bytes()) } }
    let mut out = Vec::new();
    out.extend_from_slice(TIP_MAGIC);
    out.extend_from_slice(&g.width.to_le_bytes());
    out.extend_from_slice(&g.height.to_le_bytes());
    out.push(if eight { 8 } else { 16 });
    let mut z = flate2::write::DeflateEncoder::new(out, flate2::Compression::default());
    let _ = z.write_all(&raw);
    z.finish().unwrap_or_default()
}

fn read_tip(path: &Path) -> Result<GrayTile, String> {
    let b = read_capped(path, MAX_TIP_BYTES)?;
    if b.len() < 13 || &b[..4] != TIP_MAGIC { return Err("not a tip file".into()); }
    let w = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let h = u32::from_le_bytes([b[8], b[9], b[10], b[11]]);
    let bits = b[12];
    let n = (w as usize).checked_mul(h as usize).filter(|n| *n > 0 && *n <= 1 << 28).ok_or("bad tip size")?;
    let len = n * if bits == 8 { 1 } else { 2 };
    let mut raw = Vec::with_capacity(len);
    flate2::read::DeflateDecoder::new(&b[13..]).take(len as u64).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() != len { return Err("tip file cut short".into()); }
    let data: Vec<u16> = if bits == 8 { raw.iter().map(|v| u16::from(*v) * 257).collect() } else { raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
    Ok(GrayTile { width: w, height: h, data: data.into() })
}

fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    if len > max { return Err(format!("too large ({} MB)", len >> 20)); }
    let mut v = Vec::with_capacity(len as usize);
    f.take(max).read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(v)
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_grouped_and_unique() {
        let v = builtin();
        let mut names: Vec<&str> = v.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), v.len(), "duplicate preset names");
        let order: Vec<usize> = v.iter().map(|p| BUILTIN_GROUPS.iter().position(|g| *g == p.group).unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
        assert!(v.iter().any(|p| p.kind == Kind::Eraser) && v.iter().any(|p| p.settings.mixer.enabled));
        assert!(v.iter().all(|p| p.settings.erase == (p.kind == Kind::Eraser)));
    }

    #[test]
    fn a_tip_file_round_trips() {
        let g8 = GrayTile::from_fn(17, 9, |x, y| ((x * 13 + y * 7) % 256) as f32 / 255.0);
        let g16 = GrayTile::from_fn(5, 3, |x, y| (x as f32 + 0.1) / (y as f32 + 7.3));
        let dir = std::env::temp_dir().join(format!("forge-tips-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for g in [g8, g16] {
            let p = dir.join("t.tip");
            write_atomic(&p, &encode_tip(&g)).unwrap();
            assert_eq!(read_tip(&p).unwrap(), g);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imported_groups_come_back_with_their_tips() {
        let dir = std::env::temp_dir().join(format!("forge-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tip = GrayTile::from_fn(300, 200, |x, y| if (x + y) % 3 == 0 { 1.0 } else { 0.25 });
        let mut presets = vec![Preset { name: "My Grass".into(), kind: Kind::Paint, group: "Mine".into(), folder: vec!["A".into()], settings: BrushSettings { tip: TipShape::Sampled(tip.clone()), ..Default::default() }, builtin: false }];
        let (lib, _) = Library::open(dir.clone());
        lib.save_group(&dir, "Mine", &mut presets).unwrap();
        let (lib, warnings) = Library::open(dir.clone());
        assert!(warnings.is_empty(), "{warnings:?}");
        let p = lib.presets.iter().find(|p| p.name == "My Grass").unwrap();
        assert_eq!((p.group.as_str(), p.folder.as_slice()), ("Mine", &["A".to_string()][..]));
        let mut b = p.settings.clone();
        assert!(matches!(&b.tip, TipShape::Stored(t) if t.full.is_none() && t.preview.width <= PREVIEW_SIDE));
        lib.load_tips(&mut b);
        assert_eq!(b.tip.bitmap(), Some(&tip));
        let mut lib = lib;
        assert!(lib.remove_group("Mine"));
        assert_eq!(std::fs::read_dir(dir.join("tips")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

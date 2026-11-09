//! document.json: the tree without pixels, the settings, the step list.
//! Every format change gets a version and a migration here, run in order
//! when an older file is read.
//!
//! 1: layers as whole LZ4 images. 2: layers as 64 px tiles. 3: filter
//! layers, labels, clipping, reference and draft marks. 4: animation
//! folders. 5: their drawings as sheets ("sheet", was "frame"). 6
//! (0.20): unbounded surfaces of any tile size, the tile size, pixel
//! depth, fps, the document's ICC profile, the active selection,
//! selection masks, patch-based history.

use aurastudio_core::{BlendMode, DocType, Fps, PixelDepth};
use aurastudio_layers::{Anim, Cel, Props, LABELS};
use serde::{Deserialize, Serialize};

/// Version written in new files; files from a newer version are refused.
pub const FORMAT_VERSION: u32 = 6;

#[derive(Serialize, Deserialize)]
pub(crate) struct DocJson {
    pub format:  String,
    pub version: u32,
    pub width:   u32,
    pub height:  u32,
    /// Room around the frame, percent per side (files before it: the
    /// default; what lies further is cut when opened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin:  Option<u32>,
    pub dpi:     f32,
    /// Illustration, Animation or Comics (files before it: Illustration).
    #[serde(default = "illustration")]
    pub kind:    String,
    /// Tile size of new layers (files before 6: the type's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile:    Option<u32>,
    #[serde(default, skip_serializing_if = "is_u8_depth")]
    pub depth:   String,
    /// The current frame (animation).
    #[serde(default = "first_frame", skip_serializing_if = "is_first_frame")]
    pub frame:   u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps:     Option<[u32; 2]>,
    pub active:  u64,
    /// Bottom to top.
    pub layers:  Vec<NodeJson>,
    #[serde(default)]
    pub undo:    Vec<StepJson>,
    #[serde(default)]
    pub redo:    Vec<StepJson>,
    /// Autosaves only: the file the work belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin:  Option<std::path::PathBuf>,
    /// The onion skin's marks (animation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onion:   Option<OnionJson>,
    /// The document's profile name (its bytes: profile.icc).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// An active selection (selection.surface).
    #[serde(default, skip_serializing_if = "is_false")]
    pub selection: bool,
}

fn illustration() -> String { DocType::Illustration.name().into() }
fn first_frame() -> u32 { 1 }
fn is_first_frame(n: &u32) -> bool { *n == 1 }
pub(crate) fn is_false(b: &bool) -> bool { !*b }
fn is_u8_depth(s: &str) -> bool { s.is_empty() || s == PixelDepth::U8.name() }
fn yes() -> bool { true }

pub(crate) fn depth_from(s: &str) -> PixelDepth { PixelDepth::ALL.into_iter().find(|d| d.name() == s).unwrap_or_default() }
pub(crate) fn fps_from(f: Option<[u32; 2]>) -> Fps { f.filter(|f| f[0] > 0 && f[1] > 0 && f[0] <= 1000 * f[1]).map_or_else(Fps::default, |f| Fps::new(f[0], f[1])) }

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct OnionJson {
    pub enabled: bool,
    #[serde(default)]
    pub mobile:  Vec<(i32, Option<f32>)>,
    #[serde(default)]
    pub fixed:   Vec<(u32, Option<f32>)>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum NodeJson {
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
        /// A sheet of one: the frame it is exposed from ("frame" before 5).
        #[serde(default, alias = "frame", skip_serializing_if = "Option::is_none")]
        sheet: Option<u32>,
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
    /// A selection mask (masks/<id>.surface).
    Mask {
        id: u64, name: String, visible: bool, locked: bool,
        #[serde(default, skip_serializing_if = "PropsJson::is_default")]
        props: PropsJson,
    },
}

/// A node's Props: the label by name ("Blue"...), the marks.
#[derive(Serialize, Deserialize, Default, PartialEq)]
pub(crate) struct PropsJson {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label:     String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clip:      bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reference: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub draft:     bool,
}

impl PropsJson {
    fn is_default(&self) -> bool { *self == PropsJson::default() }

    pub fn of(p: Props) -> Self {
        let label = if p.label == 0 { String::new() } else { LABELS.get(p.label as usize).map_or(String::new(), |l| l.0.to_owned()) };
        PropsJson { label, clip: p.clip, reference: p.reference, draft: p.draft }
    }

    /// (An unknown label name: none.)
    pub fn props(&self) -> Props {
        let label = LABELS.iter().position(|l| l.0 == self.label && !self.label.is_empty()).unwrap_or(0) as u8;
        Props { label, clip: self.clip, reference: self.reference, draft: self.draft }
    }
}

/// An animation folder's template and chosen cel ("sub" before).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct AnimJson {
    pub template: Vec<CelJson>,
    #[serde(default, rename = "cel", alias = "sub")]
    pub cel:      usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct CelJson {
    pub name:      String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub colo:      bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reference: bool,
    #[serde(default = "yes")]
    pub visible:   bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked:    bool,
    #[serde(default = "yes")]
    pub onion:     bool,
}

impl AnimJson {
    pub fn of(a: &Anim) -> Self {
        let template = a.template.iter().map(|t| CelJson { name: t.name.clone(), colo: t.colo, reference: t.reference, visible: t.visible, locked: t.locked, onion: t.onion }).collect();
        Self { template, cel: a.cel }
    }

    pub fn anim(self) -> Anim {
        let template: Vec<Cel> = self.template.into_iter().map(|t| Cel { name: t.name, colo: t.colo, reference: t.reference, visible: t.visible, locked: t.locked, onion: t.onion }).collect();
        let cel = self.cel.min(template.len().saturating_sub(1));
        Anim { template, cel }
    }
}

/// A filter layer's filter: its kind by name ("Gaussian blur"...), its
/// values and color.
#[derive(Serialize, Deserialize)]
pub(crate) struct FilterJson {
    pub kind:   String,
    pub values: Vec<f32>,
    #[serde(default)]
    pub color:  [u8; 3],
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StepJson {
    pub label: String,
    pub layer: u64,
    /// Rect-based steps (before 6): x0, y0, x1, y1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect:  Option<[i64; 4]>,
    /// Undone with the step before it (older versions: never).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub joined: bool,
    /// The sheet it made first: id, folder, index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<[u64; 3]>,
}

pub(crate) const PASS_THROUGH: &str = "Pass Through";

pub(crate) fn blend_from(name: &str) -> BlendMode { BlendMode::ALL.into_iter().find(|m| m.name() == name).unwrap_or_default() }

/// Where a layer's pixels are in an archive of `version`.
pub(crate) fn layer_entry(version: u32, id: u64) -> String {
    match version {
        1 => format!("layers/{id}.lz4"),
        2..=5 => format!("layers/{id}.tiles"),
        _ => format!("layers/{id}.surface"),
    }
}

/// The history entries of `kind` (undo / redo) in an archive of `version`.
pub(crate) fn step_entry(version: u32, kind: &str, i: usize) -> String {
    if version < 6 { format!("history/{kind}-{i}.lz4") } else { format!("history/{kind}-{i}.bin") }
}

/// The migrations, oldest first: a file of `version` brought to what
/// this version reads. (Renamed fields are serde aliases: "frame" for
/// "sheet" (5), "sub" for "cel"; what changed in the archive is
/// `layer_entry` / `step_entry`.)
pub(crate) fn migrate(mut json: DocJson) -> DocJson {
    if json.version < 6 {
        // Before 6: the document type's default tile size, 8 bit, 24 fps.
        let kind = DocType::from_name(&json.kind).unwrap_or_default();
        json.tile.get_or_insert(kind.default_tile_size());
    }
    json
}

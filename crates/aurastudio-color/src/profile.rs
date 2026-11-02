//! A color profile: what a document's (or a monitor's, a printer's)
//! numbers mean. Cheap to clone (shared), compared by its ICC bytes.

use std::sync::Arc;

use aurastudio_core::{ByteReader, ByteWriter, Fnv};
use moxcms::{ColorProfile, DataColorSpace, Layout, ProfileText};

use crate::{err, ColorError};

/// The kind of numbers a profile describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    Rgb,
    Gray,
    Cmyk,
    Lab,
    /// Something else (Luv, n-colorant…): read, not used.
    Other,
}

impl Model {
    /// Channels per color.
    pub fn channels(self) -> usize { match self { Model::Gray => 1, Model::Cmyk => 4, _ => 3 } }

    pub(crate) fn layout(self) -> Layout { match self { Model::Gray => Layout::Gray, Model::Cmyk => Layout::Rgba, _ => Layout::Rgb } }
}

struct Inner {
    name:  String,
    desc:  String,
    model: Model,
    icc:   Vec<u8>,
    key:   u64,
    cms:   ColorProfile,
}

#[derive(Clone)]
pub struct Profile(Arc<Inner>);

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Profile({:?}, {:?})", self.0.name, self.0.model) }
}

impl PartialEq for Profile {
    fn eq(&self, o: &Self) -> bool { self.0.key == o.0.key && self.0.icc == o.0.icc }
}

impl Eq for Profile {}

fn text(t: &Option<ProfileText>) -> Option<String> {
    match t.as_ref()? {
        ProfileText::PlainString(s) => Some(s.clone()),
        ProfileText::Localizable(v) => v.iter().find(|l| l.language == "en").or(v.first()).map(|l| l.value.clone()),
        ProfileText::Description(d) => Some(if d.unicode_string.is_empty() { d.ascii_string.clone() } else { d.unicode_string.clone() }),
    }.map(|s| s.trim_end_matches('\0').trim().to_owned()).filter(|s| !s.is_empty())
}

impl Profile {
    fn of(cms: ColorProfile, icc: Vec<u8>, name: &str) -> Profile {
        let model = match cms.color_space {
            DataColorSpace::Rgb => Model::Rgb,
            DataColorSpace::Gray => Model::Gray,
            DataColorSpace::Cmyk => Model::Cmyk,
            DataColorSpace::Lab => Model::Lab,
            _ => Model::Other,
        };
        let desc = text(&cms.description).unwrap_or_else(|| name.to_owned());
        let mut h = Fnv::default();
        h.bytes(&icc);
        Profile(Arc::new(Inner { name: name.to_owned(), desc, model, key: h.finish(), icc, cms }))
    }

    fn built_in(cms: ColorProfile, name: &str) -> Profile {
        // (Encoding a built-in profile doesn't fail; empty bytes would still
        // compare and convert through the parsed profile.)
        let icc = cms.encode().unwrap_or_default();
        Self::of(cms, icc, name)
    }

    /// sRGB IEC 61966-2.1: documents by default, untagged images.
    pub fn srgb() -> Profile {
        static P: std::sync::OnceLock<Profile> = std::sync::OnceLock::new();
        P.get_or_init(|| Self::built_in(ColorProfile::new_srgb(), "sRGB")).clone()
    }

    /// Display P3 (wide gamut screens).
    pub fn display_p3() -> Profile {
        static P: std::sync::OnceLock<Profile> = std::sync::OnceLock::new();
        P.get_or_init(|| Self::built_in(ColorProfile::new_display_p3(), "Display P3")).clone()
    }

    /// The built-ins (no commercial profile is bundled: others come from
    /// files).
    pub fn built_ins() -> [Profile; 2] { [Self::srgb(), Self::display_p3()] }

    /// From ICC bytes (a file, an image's embedded profile); `name` is
    /// what to call it if it has no description (a file name).
    pub fn from_icc(icc: &[u8], name: &str) -> Result<Profile, ColorError> {
        let cms = ColorProfile::new_from_slice(icc).map_err(|e| err("bad ICC profile", e))?;
        Ok(Self::of(cms, icc.to_vec(), name))
    }

    pub fn from_file(path: &std::path::Path) -> Result<Profile, ColorError> {
        let icc = std::fs::read(path).map_err(|e| err(&path.display().to_string(), e))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self::from_icc(&icc, &name)
    }

    /// Short name (menus).
    pub fn name(&self) -> &str { &self.0.name }
    /// The profile's own description.
    pub fn description(&self) -> &str { &self.0.desc }
    pub fn model(&self) -> Model { self.0.model }
    pub fn icc(&self) -> &[u8] { &self.0.icc }
    /// A key of its content (caches).
    pub fn key(&self) -> u64 { self.0.key }
    pub fn is_srgb(&self) -> bool { *self == Self::srgb() }
    pub fn is_matrix_shaper(&self) -> bool { self.0.cms.is_matrix_shaper() }
    pub(crate) fn cms(&self) -> &ColorProfile { &self.0.cms }

    /// For a file (.aurs header): name, then the ICC bytes.
    pub fn write(&self, w: &mut ByteWriter) { w.str(&self.0.name).blob(&self.0.icc); }

    pub fn read(r: &mut ByteReader<'_>) -> Result<Profile, ColorError> {
        let name = r.str(1024).ok_or_else(|| ColorError("profile name missing".into()))?.to_owned();
        let icc = r.blob(64 << 20).ok_or_else(|| ColorError("profile bytes missing".into()))?;
        // The built-ins come back as themselves (same bytes, same key).
        Self::built_ins().into_iter().find(|p| p.icc() == icc).map_or_else(|| Self::from_icc(icc, &name), Ok)
    }
}

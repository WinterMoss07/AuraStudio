//! Brushes from other apps: Photoshop's `.abr` (v1, v2 and v6+, with their
//! tips, patterns and dynamics) and Procreate's `.brush` / `.brushset`.
//! The readers and the mappings are PhotoCraft's (see NOTICE.md). Every
//! reader bounds-checks its input and never panics; what has no equivalent
//! here is listed in the import's warnings, never dropped silently.

mod abr_map;
mod procreate;
pub(crate) mod psd;
mod zip;

pub(crate) use abr_map::read_abr;
pub(crate) use procreate::read_procreate;

/// What a brush file is, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushFile { Abr, Procreate }

impl BrushFile {
    /// The extensions read: `.abr`, `.brush`, `.brushset`.
    pub const EXTENSIONS: [&'static str; 3] = ["abr", "brush", "brushset"];

    pub fn of(path: &std::path::Path) -> Option<BrushFile> {
        let e = path.extension()?.to_str()?.to_ascii_lowercase();
        match e.as_str() {
            "abr" => Some(BrushFile::Abr),
            "brush" | "brushset" => Some(BrushFile::Procreate),
            _ => None,
        }
    }
}

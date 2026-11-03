//! Brushes from other apps: Photoshop's `.abr` (v1, v2 and v6+, with their
//! tips, patterns and dynamics) and Procreate's `.brush` / `.brushset`.
//! The readers and the mappings are PhotoCraft's (see NOTICE.md). Every
//! reader bounds-checks its input and never panics; what has no equivalent
//! here is listed in the import's warnings, never dropped silently.

pub mod abr_map;
pub mod procreate;
pub(crate) mod psd;
pub mod zip;

pub use abr_map::{read_abr, AbrImport};
pub use procreate::{read_procreate, ProcreateImport};

/// A long import was cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("cancelled") }
}

impl std::error::Error for Cancelled {}

/// What a long import checks between brushes: cancel, and its progress.
#[derive(Clone, Copy)]
pub struct Interrupt<'a> {
    cancel:   &'a (dyn Fn() -> bool + Sync),
    progress: &'a (dyn Fn(f32) + Sync),
}

fn never() -> bool { false }

fn ignore(_: f32) {}

impl<'a> Interrupt<'a> {
    /// Never cancelled, no progress.
    pub const NONE: Interrupt<'static> = Interrupt { cancel: &never, progress: &ignore };

    pub fn new(cancel: &'a (dyn Fn() -> bool + Sync), progress: &'a (dyn Fn(f32) + Sync)) -> Self { Self { cancel, progress } }

    pub fn cancelled(&self) -> bool { (self.cancel)() }

    pub fn check(&self) -> Result<(), Cancelled> { if self.cancelled() { Err(Cancelled) } else { Ok(()) } }

    pub fn progress(&self, p: f32) { (self.progress)(p.clamp(0.0, 1.0)) }
}

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

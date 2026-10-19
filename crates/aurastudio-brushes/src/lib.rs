//! Brush library: the presets of the Tools docker, by medium.
//!
//! - Bundled: assets/brushes/catalog.txt (our round brushes and a choice of
//!   the MyPaint collection, CC0), embedded at build time.
//! - Imported .myb files, copied to the user's data folder; their medium
//!   is guessed from their name.
//!
//! Each preset keeps its own size, opacity, hardness and blend mode.

pub mod stroke_preview;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurastudio_brush::{parse_myb, Brush, MyPaintBrush, MyPaintEngine};

/// A brush of the catalog.
pub enum BundledBrush { Round, Soft, MyPaint(&'static str) }

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// The mediums, in the Tools docker's order.
pub const MEDIUMS: &[&str] = &["Pencil", "Ink", "Charcoal & pastel", "Paint", "Watercolor", "Airbrush", "Blend & smudge", "Eraser", "Texture & FX", "Misc"];

/// Medium of an imported brush, from words in its name (Misc if none).
pub fn guess_medium(name: &str) -> &'static str {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["eraser", "erase", "gomme"]) { "Eraser" }
    else if has(&["blend", "smudge", "smear", "blur", "knife"]) { "Blend & smudge" }
    else if has(&["pencil", "sketch", "graphite", "crayon", "hb", "2b", "4b", "6b", "8b"]) { "Pencil" }
    else if has(&["charcoal", "chalk", "pastel", "conte", "fusain"]) { "Charcoal & pastel" }
    else if has(&["water", "wet", "aquarel", "gouache", "wash"]) { "Watercolor" }
    else if has(&["air", "spray", "soft"]) { "Airbrush" }
    else if has(&["ink", "pen", "liner", "marker", "callig", "nib", "feutre"]) { "Ink" }
    else if has(&["oil", "acryl", "paint", "brush", "bristle", "flat", "round", "fan"]) { "Paint" }
    else if has(&["texture", "grass", "leaf", "leaves", "fur", "hair", "splat", "cloud", "particle", "noise", "grain", "pattern"]) { "Texture & FX" }
    else { "Misc" }
}

/// Which engine paints a preset.
#[derive(Clone)]
pub enum Engine {
    /// AuraStudio's dab engine: presets only differ by their settings.
    Native,
    /// A MyPaint brush, painted by libmypaint's rules.
    MyPaint(Arc<MyPaintBrush>),
}

impl Engine {
    pub fn is_mypaint(&self) -> bool {
        matches!(self, Engine::MyPaint(_))
    }
}

#[derive(Clone)]
pub struct Preset {
    pub name:   String,
    /// One of MEDIUMS.
    pub medium: String,
    /// Who made it (or the imported pack).
    pub author: String,
    pub engine: Engine,
    /// Size, opacity, hardness and blend mode (the color is global).
    pub settings: Brush,
    /// The imported file, None for bundled brushes.
    pub file:   Option<PathBuf>,
}

impl Preset {
    fn builtin(name: &str, medium: &str, radius: f32, hardness: f32) -> Self {
        let settings = Brush { radius, hardness, ..Brush::new() };
        Self { name: name.into(), medium: medium.into(), author: "AuraStudio".into(), engine: Engine::Native, settings, file: None }
    }

    /// A preset from a .myb brush, at the size and hardness it was made
    /// with; opacity 100 % (the slider multiplies the brush's own).
    fn mypaint(name: String, medium: String, author: String, brush: MyPaintBrush, file: Option<PathBuf>) -> Self {
        let settings = Brush {
            radius: (MyPaintEngine::native_size(&brush) as f32 / 2.0).max(0.5),
            hardness: MyPaintEngine::native_hardness(&brush),
            ..Brush::new()
        };
        Self { name, medium, author, engine: Engine::MyPaint(Arc::new(brush)), settings, file }
    }
}

pub struct Library {
    pub presets: Vec<Preset>,
}

impl Library {
    /// The bundled brushes, then those imported before.
    pub fn load() -> Self {
        let mut presets = Vec::new();
        for (medium, name, author, brush) in BUNDLED {
            match brush {
                BundledBrush::Round => presets.push(Preset::builtin(name, medium, 25.0, 1.0)),
                BundledBrush::Soft => presets.push(Preset::builtin(name, medium, 60.0, 0.0)),
                BundledBrush::MyPaint(json) => match parse_myb(json) {
                    Ok(b) => presets.push(Preset::mypaint(name.to_string(), medium.to_string(), author.to_string(), b, None)),
                    Err(e) => log::warn!("bundled brush {medium}/{name}: {e:?}"),
                },
            }
        }
        let mut lib = Self { presets };
        if let Some(dir) = user_dir() {
            let n = lib.add_from(&dir, None);
            if n > 0 { log::info!("brushes: {n} imported brush(es) loaded from {}", dir.display()); }
        }
        lib
    }

    /// Import a .myb file or a folder of them (recursively): copied into
    /// the user's brush folder and added. Returns how many were added.
    pub fn import(&mut self, path: &Path) -> Result<usize, String> {
        let dest_root = user_dir().ok_or("no user data folder on this system")?;
        let pack = match path.is_dir() {
            true => path.file_name().map_or_else(|| "Imported".into(), |n| n.to_string_lossy().into_owned()),
            false => "Imported".into(),
        };
        let dest = dest_root.join(&pack);
        std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut copied = 0;
        for f in myb_files(path) {
            let target = dest.join(f.file_name().unwrap());
            // Already there (imported from the library folder itself):
            // copying a file onto itself would empty it.
            let same = f.canonicalize().ok().zip(target.canonicalize().ok()).is_some_and(|(a, b)| a == b);
            if same || std::fs::copy(&f, &target).is_ok() { copied += 1; }
        }
        if copied == 0 { return Err("no .myb brush found there".into()); }
        Ok(self.add_from(&dest, Some(&pack)))
    }

    /// Add the .myb files under `dir` not added yet; author: `pack`, else
    /// their folder's name.
    fn add_from(&mut self, dir: &Path, pack: Option<&str>) -> usize {
        let mut added = 0;
        for f in myb_files(dir) {
            if self.presets.iter().any(|p| p.file.as_ref() == Some(&f)) { continue; }
            let Ok(json) = std::fs::read_to_string(&f) else { continue };
            let brush = match parse_myb(&json) {
                Ok(b) => b,
                Err(e) => { log::warn!("brush {}: {e:?}", f.display()); continue; }
            };
            let name = f.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
            let author = pack.map(str::to_owned).unwrap_or_else(|| {
                f.parent().and_then(|p| p.file_name()).map_or_else(|| "Imported".into(), |n| n.to_string_lossy().into_owned())
            });
            let medium = guess_medium(&name).to_string();
            self.presets.push(Preset::mypaint(name, medium, author, brush, Some(f)));
            added += 1;
        }
        added
    }
}

/// Where imported brushes are kept (e.g. %APPDATA%\AuraStudio\brushes).
pub fn user_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AURASTUDIO_BRUSH_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::data_dir().map(|d| d.join("AuraStudio").join("brushes"))
}

/// .myb files at `path` (a file, or a folder searched recursively), sorted.
fn myb_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("myb")) { out.push(path.to_path_buf()); }
        return out;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("myb")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_by_medium() {
        let lib = Library::load();
        let n = lib.presets.len();
        assert!((50..=70).contains(&n), "{n} bundled brushes");
        assert!(lib.presets.iter().all(|p| MEDIUMS.contains(&p.medium.as_str())), "unknown medium");
        // Every medium has brushes, in MEDIUMS order.
        let mut used: Vec<&str> = lib.presets.iter().map(|p| p.medium.as_str()).collect();
        used.dedup();
        assert_eq!(used, MEDIUMS);
        assert_eq!(lib.presets[0].medium, "Pencil");
        let round = lib.presets.iter().find(|p| p.name == "Round pen").unwrap();
        assert!(matches!(round.engine, Engine::Native) && round.medium == "Ink");
        assert_eq!(lib.presets.iter().find(|p| p.name == "Liner").unwrap().author, "David Revoy");
    }

    #[test]
    fn imported_brushes_get_a_medium_from_their_name() {
        assert_eq!(guess_medium("Big Soft Eraser"), "Eraser");
        assert_eq!(guess_medium("2B pencil HB"), "Pencil");
        assert_eq!(guess_medium("wet acrylic"), "Watercolor");
        assert_eq!(guess_medium("Bristle oil"), "Paint");
        assert_eq!(guess_medium("G-pen"), "Ink");
        assert_eq!(guess_medium("zorglub"), "Misc");
    }

    #[test]
    fn import_copies_brushes_into_the_user_folder() {
        let tmp = std::env::temp_dir().join(format!("aura_brush_test_{}", std::process::id()));
        let src = tmp.join("My pack");
        let user = tmp.join("user");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/inky.myb"), r#"{"version": 3, "settings": {"radius_logarithmic": {"base_value": 1.5, "inputs": {}}}}"#).unwrap();
        std::fs::write(src.join("broken.myb"), "not json").unwrap();
        std::env::set_var("AURASTUDIO_BRUSH_DIR", &user);

        let mut lib = Library { presets: Vec::new() };
        let added = lib.import(&src).unwrap();
        assert_eq!(added, 1);
        assert!(user.join("My pack/inky.myb").is_file());
        assert_eq!(lib.presets[0].author, "My pack");
        assert_eq!(lib.presets[0].medium, "Ink");
        assert_eq!(lib.presets[0].name, "inky");
        // Importing again doesn't duplicate.
        assert_eq!(lib.import(&src).unwrap(), 0);
        std::env::remove_var("AURASTUDIO_BRUSH_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

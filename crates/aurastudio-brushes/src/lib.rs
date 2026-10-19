//! Brush library: the presets listed in the Tools docker, by medium
//! (pencil, ink, paint...).
//!
//! - The bundled brushes, listed in assets/brushes/catalog.txt: our own
//!   engine's round brushes and a selection of the MyPaint collection
//!   (CC0, assets/brushes/mypaint), embedded in the binary at build time.
//! - Brushes the user imports (.myb files or folders of them), copied into
//!   the user's data folder so they are there at the next launch; their
//!   medium is guessed from their name.
//!
//! Each preset keeps its own settings (size, opacity, hardness, blend
//! mode): changing them while a brush is selected only changes
//! that brush, like CSP.

use std::borrow::Cow;
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

pub mod stroke_preview;

/// Which engine paints a preset.
#[derive(Clone)]
pub enum Engine {
    /// AuraStudio's brush engine (aurastudio_brush::DabEngine); presets
    /// only differ by their settings.
    Native,
    /// A MyPaint brush, painted natively by libmypaint's rules.
    MyPaint(Arc<MyPaintBrush>),
}

impl Engine {
    pub fn is_mypaint(&self) -> bool {
        matches!(self, Engine::MyPaint(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    Bundled,
    Imported(PathBuf),
}

#[derive(Clone)]
pub struct Preset {
    pub name:   String,
    /// Medium (one of MEDIUMS).
    pub group:  String,
    /// Who made it (shown on hover).
    pub author: String,
    pub engine: Engine,
    /// Size, opacity, hardness and blend mode of this brush (the color is
    /// global and ignored here).
    pub settings:  Brush,
    /// PNG preview (MyPaint brushes come with one).
    pub preview:   Option<Cow<'static, [u8]>>,
    pub source:    Source,
}

impl Preset {
    fn builtin(name: &str, medium: &str, radius: f32, hardness: f32) -> Self {
        let mut settings = Brush::new();
        settings.radius = radius;
        settings.hardness = hardness;
        Self {
            name: name.into(),
            group: medium.into(),
            author: "AuraStudio".into(),
            engine: Engine::Native,
            settings,
            preview: None,
            source: Source::BuiltIn,
        }
    }

    /// A preset from a parsed .myb brush, with the size / opacity /
    /// hardness it was made with.
    fn mypaint(name: String, group: String, author: String, brush: MyPaintBrush, preview: Option<Cow<'static, [u8]>>, source: Source) -> Self {
        let size = MyPaintEngine::native_size(&brush);
        let mut settings = Brush::new();
        settings.radius = (size as f32 / 2.0).max(0.5);
        // The Opacity slider multiplies the brush's own opacity: 100 %,
        // the brush as made.
        settings.opacity = 1.0;
        settings.hardness = MyPaintEngine::native_hardness(&brush);
        Self { name, group, author, engine: Engine::MyPaint(Arc::new(brush)), settings, preview, source }
    }

    /// Decode the preview, downscaled to at most `max` px: (w, h, RGBA).
    pub fn decode_preview(&self, max: u32) -> Option<(u32, u32, Vec<u8>)> {
        let bytes = self.preview.as_ref()?;
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes.as_ref()));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width, info.height);
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => buf[..(w * h * 4) as usize].to_vec(),
            png::ColorType::Rgb => buf[..(w * h * 3) as usize].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => buf[..(w * h * 2) as usize].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
            png::ColorType::Grayscale => buf[..(w * h) as usize].iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        // Nearest downscale: previews are small already (128 px).
        let k = (max as f32 / w.max(h) as f32).min(1.0);
        let (tw, th) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let mut out = Vec::with_capacity((tw * th * 4) as usize);
        for y in 0..th {
            for x in 0..tw {
                let i = (((y * h / th) * w + x * w / tw) * 4) as usize;
                out.extend_from_slice(&rgba[i..i + 4]);
            }
        }
        Some((tw, th, out))
    }
}

pub struct Library {
    pub presets: Vec<Preset>,
}

impl Library {
    /// The bundled brushes (catalog.txt), then what the user imported
    /// before.
    pub fn load() -> Self {
        let mut presets = Vec::new();
        for (medium, name, author, brush) in BUNDLED {
            match brush {
                BundledBrush::Round => presets.push(Preset::builtin(name, medium, 25.0, 1.0)),
                BundledBrush::Soft => presets.push(Preset::builtin(name, medium, 60.0, 0.0)),
                BundledBrush::MyPaint(json) => match parse_myb(json) {
                    Ok(b) => presets.push(Preset::mypaint(name.to_string(), medium.to_string(), author.to_string(), b, None, Source::Bundled)),
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

    /// Mediums in use, in display order.
    pub fn groups(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in &self.presets {
            if !out.contains(&p.group.as_str()) { out.push(&p.group); }
        }
        out
    }

    /// Import a .myb file or a folder of them (searched recursively): the
    /// files are copied into the user's brush folder, so they stay
    /// available, and added to the library. Returns how many were added.
    pub fn import(&mut self, path: &Path) -> Result<usize, String> {
        let dest_root = user_dir().ok_or("no user data folder on this system")?;
        let group = if path.is_dir() {
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into())
        } else {
            "Imported".into()
        };
        let dest = dest_root.join(&group);
        std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut copied = 0;
        for f in myb_files(path) {
            let Some(stem) = f.file_stem() else { continue };
            let target = dest.join(f.file_name().unwrap());
            // Already in the library folder (imported from there): copying a
            // file onto itself would empty it.
            let same = f.canonicalize().ok().zip(target.canonicalize().ok()).is_some_and(|(a, b)| a == b);
            if same || std::fs::copy(&f, &target).is_ok() {
                copied += 1;
                let prev = f.with_file_name(format!("{}_prev.png", stem.to_string_lossy()));
                if prev.is_file() {
                    let _ = std::fs::copy(&prev, dest.join(prev.file_name().unwrap()));
                }
            }
        }
        if copied == 0 { return Err("no .myb brush found there".into()); }
        Ok(self.add_from(&dest, Some(&group)))
    }

    /// Add the .myb files under `dir`; group = their folder name (or
    /// `group`). Already present brushes (same file) are skipped.
    fn add_from(&mut self, dir: &Path, group: Option<&str>) -> usize {
        let mut added = 0;
        for f in myb_files(dir) {
            if self.presets.iter().any(|p| p.source == Source::Imported(f.clone())) { continue; }
            let Ok(json) = std::fs::read_to_string(&f) else { continue };
            let brush = match parse_myb(&json) {
                Ok(b) => b,
                Err(e) => { log::warn!("brush {}: {e:?}", f.display()); continue; }
            };
            let name = f.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
            // The pack (its folder) as the author; the medium from the name.
            let pack = group.map(str::to_owned).unwrap_or_else(|| {
                f.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into())
            });
            let prev = f.with_file_name(format!("{}_prev.png", f.file_stem().unwrap().to_string_lossy()));
            let preview = std::fs::read(prev).ok().map(Cow::Owned);
            let medium = guess_medium(&name).to_string();
            self.presets.push(Preset::mypaint(name, medium, pack, brush, preview, Source::Imported(f)));
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
        assert!(lib.presets.iter().all(|p| MEDIUMS.contains(&p.group.as_str())), "unknown medium");
        // Every medium has brushes, in MEDIUMS order.
        assert_eq!(lib.groups(), MEDIUMS);
        assert_eq!(lib.presets[0].group, "Pencil");
        let round = lib.presets.iter().find(|p| p.name == "Round pen").unwrap();
        assert!(matches!(round.engine, Engine::Native) && round.group == "Ink");
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
        assert_eq!(lib.presets[0].group, "Ink");
        assert_eq!(lib.presets[0].name, "inky");
        // Importing again doesn't duplicate.
        assert_eq!(lib.import(&src).unwrap(), 0);
        std::env::remove_var("AURASTUDIO_BRUSH_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

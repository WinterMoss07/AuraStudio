//! Brush library: the presets listed in the Brushes docker.
//!
//! - Built-in presets for our own engines (round pen, soft round, dab
//!   round).
//! - The MyPaint brush collection (196 brushes, CC0, see
//!   assets/brushes/mypaint), embedded in the binary at build time.
//! - Brushes the user imports (.myb files or folders of them), copied into
//!   the user's data folder so they are there at the next launch.
//!
//! Each preset keeps its own settings (size, opacity, hardness, blend
//! mode, smoothing): changing them while a brush is selected only changes
//! that brush, like CSP.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurastudio_brush::{parse_myb, Brush, MyPaintBrush, MyPaintEngine};

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// Which engine paints a preset.
#[derive(Clone)]
pub enum Engine {
    /// Swept round pen (continuous, no dabs).
    RoundPen,
    /// Round dabs through hokusai with no build-up within a stroke.
    DabRound,
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
    pub group:  String,
    pub engine: Engine,
    /// Size, opacity, hardness and blend mode of this brush (the color is
    /// global and ignored here).
    pub settings:  Brush,
    /// Pressure smoothing (round pen and dab round).
    pub smoothing: f32,
    /// PNG preview (MyPaint brushes come with one).
    pub preview:   Option<Cow<'static, [u8]>>,
    pub source:    Source,
}

impl Preset {
    fn builtin(name: &str, engine: Engine, radius: u32, hardness: f32) -> Self {
        let mut settings = Brush::new();
        settings.radius = radius;
        settings.hardness = hardness;
        Self {
            name: name.into(),
            group: "AuraStudio".into(),
            engine,
            settings,
            smoothing: 0.5,
            preview: None,
            source: Source::BuiltIn,
        }
    }

    /// A preset from a parsed .myb brush, with the size / opacity /
    /// hardness it was made with.
    fn mypaint(name: String, group: String, brush: MyPaintBrush, preview: Option<Cow<'static, [u8]>>, source: Source) -> Self {
        let (size, opacity) = MyPaintEngine::native_size_and_opacity(&brush);
        let mut settings = Brush::new();
        settings.radius = (size / 2).max(1);
        settings.opacity = opacity;
        settings.hardness = MyPaintEngine::native_hardness(&brush);
        Self { name, group, engine: Engine::MyPaint(Arc::new(brush)), settings, smoothing: 0.0, preview, source }
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
    /// Built-in presets, then the bundled MyPaint collection, then what
    /// the user imported before.
    pub fn load() -> Self {
        let mut presets = vec![
            Preset::builtin("Round pen", Engine::RoundPen, 25, 1.0),
            Preset::builtin("Soft round", Engine::RoundPen, 60, 0.0),
            Preset::builtin("Dab round", Engine::DabRound, 25, 0.9),
        ];
        for &(group, name, json, preview) in BUNDLED {
            match parse_myb(json) {
                Ok(b) => presets.push(Preset::mypaint(name.into(), group.into(), b, preview.map(Cow::Borrowed), Source::Bundled)),
                Err(e) => log::warn!("bundled brush {group}/{name}: {e:?}"),
            }
        }
        let mut lib = Self { presets };
        if let Some(dir) = user_dir() {
            let n = lib.add_from(&dir, None);
            if n > 0 { log::info!("brushes: {n} imported brush(es) loaded from {}", dir.display()); }
        }
        lib
    }

    /// Group names in display order.
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
            if std::fs::copy(&f, &target).is_ok() {
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
            let g = group.map(str::to_owned).unwrap_or_else(|| {
                f.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into())
            });
            let prev = f.with_file_name(format!("{}_prev.png", f.file_stem().unwrap().to_string_lossy()));
            let preview = std::fs::read(prev).ok().map(Cow::Owned);
            self.presets.push(Preset::mypaint(name, g, brush, preview, Source::Imported(f)));
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
    fn bundled_collection_loads_with_previews() {
        let lib = Library::load();
        let mypaint = lib.presets.iter().filter(|p| p.source == Source::Bundled).count();
        assert!(mypaint >= 190, "only {mypaint} bundled brushes parsed");
        assert_eq!(lib.presets[0].name, "Round pen");
        let groups = lib.groups();
        assert_eq!(groups[0], "AuraStudio");
        assert!(groups.contains(&"Deevad") && groups.contains(&"Classic"), "{groups:?}");
        let charcoal = lib.presets.iter().find(|p| p.name == "charcoal").expect("charcoal");
        let (w, h, rgba) = charcoal.decode_preview(48).expect("preview");
        assert!(w <= 48 && h <= 48 && rgba.len() == (w * h * 4) as usize);
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
        assert_eq!(lib.presets[0].group, "My pack");
        assert_eq!(lib.presets[0].name, "inky");
        // Importing again doesn't duplicate.
        assert_eq!(lib.import(&src).unwrap(), 0);
        std::env::remove_var("AURASTUDIO_BRUSH_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

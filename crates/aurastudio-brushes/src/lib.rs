//! Brush library: the bundled presets (catalog.txt) and imported .myb
//! files, by medium, each with its own size, opacity, hardness, blend.

pub mod hash;
pub mod stroke_preview;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurastudio_brush::forge::Painting;
use aurastudio_brush::{parse_myb, Brush, Forge, ForgeBrush};

/// A brush of the catalog.
pub enum BundledBrush { Round, Soft, MyPaint(&'static str) }

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// The bundled mediums, in the catalog's order.
pub const MEDIUMS: &[&str] = &["Pencil", "Ink", "Charcoal & pastel", "Paint", "Watercolor", "Airbrush", "Blend & smudge", "Eraser", "Texture & FX", "Misc"];

/// Which toolbox group a medium's brushes belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind { Draw, Paint, Eraser }

impl Kind {
    fn key(self) -> &'static str {
        match self { Kind::Draw => "draw", Kind::Paint => "paint", Kind::Eraser => "eraser" }
    }

    fn from_key(k: &str) -> Option<Kind> {
        [Kind::Draw, Kind::Paint, Kind::Eraser].into_iter().find(|x| x.key() == k)
    }

    /// Group of a bundled medium: dry techniques to Draw, wet ones to Paint.
    fn of_bundled(medium: &str) -> Kind {
        match medium {
            "Paint" | "Watercolor" | "Airbrush" | "Blend & smudge" | "Texture & FX" => Kind::Paint,
            "Eraser" => Kind::Eraser,
            _ => Kind::Draw,
        }
    }
}

/// A medium: a folder of brushes in the Tools docker, shown as an icon.
#[derive(Clone, Debug, PartialEq)]
pub struct Medium {
    pub name:   String,
    pub kind:   Kind,
    /// Made by the user (can be renamed, and removed once empty).
    pub custom: bool,
}

/// The mediums of `kind`, in their order.
pub fn mediums_of(mediums: &[Medium], kind: Kind) -> Vec<&str> {
    mediums.iter().filter(|m| m.kind == kind).map(|m| m.name.as_str()).collect()
}

fn default_mediums() -> Vec<Medium> {
    // In the toolbox's order: Draw's, then Paint's, then the eraser.
    let order = ["Pencil", "Ink", "Charcoal & pastel", "Misc", "Paint", "Watercolor", "Airbrush", "Blend & smudge", "Texture & FX", "Eraser"];
    order.iter().map(|&n| Medium { name: n.into(), kind: Kind::of_bundled(n), custom: false }).collect()
}

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

/// Where a preset's brush comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Made for Forge (our round brushes).
    Forge,
    /// A MyPaint brush (.myb), read into Forge.
    MyPaint,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Forge => "Forge brush",
            Origin::MyPaint => "MyPaint brush",
        }
    }
}

#[derive(Clone)]
pub struct Preset {
    pub name:   String,
    /// One of MEDIUMS.
    pub medium: String,
    /// Who made it (or the imported pack).
    pub author: String,
    pub origin: Origin,
    /// Its Forge definition (settings, curves, painting mode).
    pub brush:  Arc<ForgeBrush>,
    /// Size, opacity, hardness and blend mode (the color is global).
    pub settings: Brush,
    /// The imported file, None for bundled brushes.
    pub file:   Option<PathBuf>,
    /// Place in its medium's list (the user can drag brushes around).
    pub order:  u32,
    /// Its medium's group (kept by the Library).
    pub kind:   Kind,
    /// Hash of its Forge definition (`hash::of_debug`): what the stroke
    /// previews kept on disk are keyed by. Set again if `brush` changes.
    pub definition_hash: u64,
}

impl Preset {
    /// With `definition_hash` computed from `brush`.
    fn hashed(mut self) -> Self {
        self.rehash();
        self
    }

    /// Call after changing `brush` (a brush editor), so that its stroke
    /// previews are painted again.
    pub fn rehash(&mut self) { self.definition_hash = hash::of_debug(&*self.brush); }

    fn builtin(name: &str, medium: &str, radius: f32, hardness: f32) -> Self {
        let settings = Brush { radius, hardness, ..Brush::new() };
        Self { name: name.into(), medium: medium.into(), author: "AuraStudio".into(), origin: Origin::Forge,
            definition_hash: 0, brush: Arc::new(ForgeBrush::round()), settings, file: None, order: 0, kind: Kind::of_bundled(medium) }.hashed()
    }

    /// A Forge engine for a stroke with this preset.
    pub fn start(&self) -> Forge { Forge::new((*self.brush).clone()) }

    /// Its blend mode applies (wash painting; build-up brushes blend their
    /// dabs themselves).
    pub fn blends(&self) -> bool { self.brush.options.painting == Painting::Wash }

    /// A preset from a .myb brush, at the size and hardness it was made
    /// with; opacity 100 % (the slider multiplies the brush's own).
    fn mypaint(name: String, medium: String, author: String, brush: ForgeBrush, file: Option<PathBuf>) -> Self {
        let settings = Brush {
            radius: (Forge::native_size(&brush) as f32 / 2.0).max(0.5),
            hardness: Forge::native_hardness(&brush),
            ..Brush::new()
        };
        let kind = Kind::of_bundled(&medium);
        Self { name, medium, author, origin: Origin::MyPaint, definition_hash: 0, brush: Arc::new(brush), settings, file, order: 0, kind }.hashed()
    }

    /// Stable key in the saved layout.
    fn key(&self) -> String {
        match &self.file {
            Some(f) => f.display().to_string(),
            None => format!("bundled:{}", self.name),
        }
    }
}

pub struct Library {
    pub presets: Vec<Preset>,
    /// Every medium, in order (the Tools docker shows those of a group).
    pub mediums: Vec<Medium>,
}

impl Library {
    /// The bundled brushes, then those imported before.
    pub fn load() -> Self {
        let mut lib = Self::load_bundled();
        if let Some(dir) = user_dir() {
            let n = lib.add_from(&dir, None);
            if n > 0 { log::info!("brushes: {n} imported brush(es) loaded from {}", dir.display()); }
        }
        for (i, p) in lib.presets.iter_mut().enumerate() { p.order = i as u32; }
        lib.load_mediums();
        lib.load_layout();
        lib
    }

    /// The bundled brushes only, as shipped (no user layout).
    pub fn load_bundled() -> Self {
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
        Self { presets, mediums: default_mediums() }
    }

    /// Move brush `index` to `medium`, before brush `before` (None: at the
    /// end), and save the layout.
    pub fn move_preset(&mut self, index: usize, medium: &str, before: Option<usize>) {
        let kind = self.kind_of(medium);
        let Some(p) = self.presets.get_mut(index) else { return };
        p.medium = medium.to_owned();
        p.kind = kind;
        let mut list: Vec<usize> = (0..self.presets.len()).filter(|&i| i != index && self.presets[i].medium == medium).collect();
        list.sort_by_key(|&i| self.presets[i].order);
        let at = before.and_then(|b| list.iter().position(|&i| i == b)).unwrap_or(list.len());
        list.insert(at, index);
        for (k, &i) in list.iter().enumerate() { self.presets[i].order = k as u32; }
        self.save_layout();
    }

    /// The brushes of `medium`, in their order.
    pub fn in_medium(presets: &[Preset], medium: &str) -> Vec<usize> {
        let mut v: Vec<usize> = (0..presets.len()).filter(|&i| presets[i].medium == medium).collect();
        v.sort_by_key(|&i| presets[i].order);
        v
    }

    /// Mediums and order the user gave the brushes (layout.txt in the
    /// brush folder: key, medium, order per line).
    fn load_layout(&mut self) {
        let Some(text) = layout_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        for line in text.lines() {
            let mut f = line.split('\t');
            let (Some(key), Some(medium), Some(order)) = (f.next(), f.next(), f.next()) else { continue };
            let (Ok(order), true) = (order.parse(), self.mediums.iter().any(|m| m.name == medium)) else { continue };
            let kind = self.kind_of(medium);
            if let Some(p) = self.presets.iter_mut().find(|p| p.key() == key) {
                p.medium = medium.to_owned();
                p.order = order;
                p.kind = kind;
            }
        }
    }

    fn save_layout(&self) {
        let Some(path) = layout_path() else { return };
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        let text: String = self.presets.iter().map(|p| format!("{}\t{}\t{}\n", p.key(), p.medium, p.order)).collect();
        if let Err(e) = std::fs::write(&path, text) { log::warn!("brush layout: {e}"); }
    }

    /// The group of a medium (Draw for an unknown one).
    pub fn kind_of(&self, medium: &str) -> Kind {
        self.mediums.iter().find(|m| m.name == medium).map_or(Kind::Draw, |m| m.kind)
    }

    /// Move medium `name` before `before` (None: last of its group).
    pub fn move_medium(&mut self, name: &str, before: Option<&str>) {
        let Some(from) = self.mediums.iter().position(|m| m.name == name) else { return };
        let m = self.mediums.remove(from);
        let at = before.and_then(|b| self.mediums.iter().position(|x| x.name == b))
            .or_else(|| self.mediums.iter().rposition(|x| x.kind == m.kind).map(|i| i + 1))
            .unwrap_or(self.mediums.len());
        self.mediums.insert(at, m);
        self.save_mediums();
    }

    /// A new empty medium at the end of `kind`'s; returns its name.
    pub fn add_medium(&mut self, kind: Kind) -> String {
        let name = (1..).map(|n| if n == 1 { "New medium".to_string() } else { format!("New medium {n}") })
            .find(|n| !self.mediums.iter().any(|m| &m.name == n)).unwrap();
        let at = self.mediums.iter().rposition(|m| m.kind == kind).map_or(self.mediums.len(), |i| i + 1);
        self.mediums.insert(at, Medium { name: name.clone(), kind, custom: true });
        self.save_mediums();
        name
    }

    /// Rename a medium made by the user (its brushes follow). False if
    /// the name is empty or taken.
    pub fn rename_medium(&mut self, old: &str, new: &str) -> bool {
        let new = new.trim();
        if new.is_empty() || self.mediums.iter().any(|m| m.name == new) { return false; }
        let Some(m) = self.mediums.iter_mut().find(|m| m.name == old && m.custom) else { return false };
        m.name = new.to_owned();
        for p in self.presets.iter_mut().filter(|p| p.medium == old) { p.medium = new.to_owned(); }
        self.save_mediums();
        self.save_layout();
        true
    }

    /// Remove an empty medium made by the user.
    pub fn remove_medium(&mut self, name: &str) -> bool {
        if self.presets.iter().any(|p| p.medium == name) { return false; }
        let before = self.mediums.len();
        self.mediums.retain(|m| !(m.name == name && m.custom));
        if self.mediums.len() == before { return false; }
        self.save_mediums();
        true
    }

    /// The mediums' order and the user's own (mediums.txt in the brush
    /// folder: group, name, and "custom" for the user's, per line). Bundled
    /// mediums missing from it go last of their group.
    fn load_mediums(&mut self) {
        let Some(text) = mediums_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        let defaults = std::mem::take(&mut self.mediums);
        for line in text.lines() {
            let mut f = line.split('\t');
            let (Some(kind), Some(name)) = (f.next().and_then(Kind::from_key), f.next()) else { continue };
            if name.is_empty() || self.mediums.iter().any(|m| m.name == name) { continue; }
            let custom = f.next() == Some("custom");
            if !custom && !defaults.iter().any(|d| d.name == name) { continue; }
            // A bundled medium keeps its own group.
            let kind = if custom { kind } else { Kind::of_bundled(name) };
            self.mediums.push(Medium { name: name.to_owned(), kind, custom });
        }
        for d in defaults {
            if self.mediums.iter().any(|m| m.name == d.name) { continue; }
            let at = self.mediums.iter().rposition(|m| m.kind == d.kind).map_or(self.mediums.len(), |i| i + 1);
            self.mediums.insert(at, d);
        }
    }

    fn save_mediums(&self) {
        let Some(path) = mediums_path() else { return };
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        let text: String = self.mediums.iter()
            .map(|m| format!("{}\t{}{}\n", m.kind.key(), m.name, if m.custom { "\tcustom" } else { "" })).collect();
        if let Err(e) = std::fs::write(&path, text) { log::warn!("brush mediums: {e}"); }
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

fn mediums_path() -> Option<PathBuf> {
    user_dir().map(|d| d.join("mediums.txt"))
}

fn layout_path() -> Option<PathBuf> {
    user_dir().map(|d| d.join("layout.txt"))
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
    #[test]
    fn a_preset_hash_follows_the_brush_definition_only() {
        let a = Preset::builtin("Round", "Pencil", 10.0, 1.0);
        let b = Preset::builtin("Other name", "Ink", 30.0, 0.5);
        assert_ne!(a.definition_hash, 0);
        // Name, medium and size are not the definition.
        assert_eq!(a.definition_hash, b.definition_hash);
        let mut c = a.clone();
        let mut brush = (*c.brush).clone();
        brush.options.dab_on_press = !brush.options.dab_on_press;
        c.brush = Arc::new(brush);
        assert_eq!(c.definition_hash, a.definition_hash, "stale until rehash()");
        c.rehash();
        assert_ne!(c.definition_hash, a.definition_hash);
    }

    use super::*;

    /// Tests reading or setting AURASTUDIO_BRUSH_DIR take turns.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn catalog_loads_by_medium() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(round.origin == Origin::Forge && round.medium == "Ink");
        assert_eq!(lib.presets.iter().find(|p| p.name == "Liner").unwrap().author, "David Revoy");
    }

    #[test]
    fn brushes_move_and_the_layout_is_kept() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("aura_layout_test_{}", std::process::id()));
        std::env::set_var("AURASTUDIO_BRUSH_DIR", &tmp);
        let mut lib = Library::load();
        let find = |lib: &Library, n: &str| lib.presets.iter().position(|p| p.name == n).unwrap();
        let (w, oil) = (find(&lib, "Thin watercolor"), find(&lib, "Oil"));
        lib.move_preset(w, "Paint", Some(oil));
        let paint = Library::in_medium(&lib.presets, "Paint");
        assert_eq!(paint[paint.iter().position(|&i| i == oil).unwrap() - 1], w);
        let lib = Library::load();
        assert_eq!(lib.presets[find(&lib, "Thin watercolor")].medium, "Paint", "saved");
        std::env::remove_var("AURASTUDIO_BRUSH_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mediums_move_get_added_renamed_and_are_kept() {
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("aura_mediums_test_{}", std::process::id()));
        std::env::set_var("AURASTUDIO_BRUSH_DIR", &tmp);
        let mut lib = Library::load();
        assert_eq!(mediums_of(&lib.mediums, Kind::Draw), ["Pencil", "Ink", "Charcoal & pastel", "Misc"]);
        lib.move_medium("Misc", Some("Pencil"));
        let name = lib.add_medium(Kind::Draw);
        assert!(lib.rename_medium(&name, "Mine"));
        assert!(!lib.rename_medium("Mine", "Ink"), "taken");
        assert!(!lib.rename_medium("Ink", "Inks"), "bundled");
        let pen = lib.presets.iter().position(|p| p.name == "Round pen").unwrap();
        lib.move_preset(pen, "Mine", None);
        assert!(!lib.remove_medium("Mine"), "not empty");

        let lib = Library::load();
        assert_eq!(mediums_of(&lib.mediums, Kind::Draw), ["Misc", "Pencil", "Ink", "Charcoal & pastel", "Mine"]);
        let pen = lib.presets.iter().find(|p| p.name == "Round pen").unwrap();
        assert_eq!((pen.medium.as_str(), pen.kind), ("Mine", Kind::Draw));
        std::env::remove_var("AURASTUDIO_BRUSH_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
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
        let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("aura_brush_test_{}", std::process::id()));
        let src = tmp.join("My pack");
        let user = tmp.join("user");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/inky.myb"), r#"{"version": 3, "settings": {"radius_logarithmic": {"base_value": 1.5, "inputs": {}}}}"#).unwrap();
        std::fs::write(src.join("broken.myb"), "not json").unwrap();
        std::env::set_var("AURASTUDIO_BRUSH_DIR", &user);

        let mut lib = Library { presets: Vec::new(), mediums: default_mediums() };
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

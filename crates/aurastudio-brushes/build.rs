//! Embeds the bundled MyPaint brushes (assets/brushes/mypaint, CC0) into
//! the binary, in the order of their order.conf, so the app ships with a
//! full brush collection and no data folder to install.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/brushes/mypaint");
    let root = root.canonicalize().expect("assets/brushes/mypaint");
    println!("cargo:rerun-if-changed={}", root.display());

    // Groups as order.conf lists them ("Group: X", then "folder/name"),
    // skipping Favorites (it only repeats brushes from other groups).
    let order = std::fs::read_to_string(root.join("order.conf")).unwrap_or_default();
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut group = String::new();
    for line in order.lines() {
        let line = line.trim();
        if let Some(g) = line.strip_prefix("Group:") {
            group = g.trim().to_owned();
        } else if !line.is_empty() && !line.starts_with('#') && group != "Favorites"
            && root.join(format!("{line}.myb")).is_file() && seen.insert(line.to_owned())
        {
            entries.push((group.clone(), line.to_owned()));
        }
    }
    // Anything not listed, at the end of its folder's group.
    let mut extra: Vec<(String, String)> = Vec::new();
    for dir in std::fs::read_dir(&root).unwrap().flatten() {
        if !dir.path().is_dir() { continue; }
        let folder = dir.file_name().to_string_lossy().into_owned();
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "myb") {
                let rel = format!("{folder}/{}", p.file_stem().unwrap().to_string_lossy());
                if seen.insert(rel.clone()) { extra.push((folder.clone(), rel)); }
            }
        }
    }
    extra.sort();
    entries.extend(extra);

    let mut out = String::from("/// (group, name, .myb json, preview png)\npub static BUNDLED: &[(&str, &str, &str, Option<&[u8]>)] = &[\n");
    for (group, rel) in &entries {
        let myb = root.join(format!("{rel}.myb"));
        let prev = root.join(format!("{rel}_prev.png"));
        let name = Path::new(rel).file_name().unwrap().to_string_lossy().replace('_', " ");
        let prev = if prev.is_file() { format!("Some(include_bytes!({:?}))", prev.display().to_string()) } else { "None".into() };
        writeln!(out, "    ({group:?}, {name:?}, include_str!({:?}), {prev}),", myb.display().to_string()).unwrap();
    }
    out.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bundled.rs"), out).unwrap();
}

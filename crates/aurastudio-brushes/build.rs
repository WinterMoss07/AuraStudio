//! Embeds the brushes listed in assets/brushes/catalog.txt (medium, file,
//! display name) into the binary, so the app ships with its brushes and
//! no data folder to install. The MyPaint files are CC0
//! (assets/brushes/mypaint).

use std::fmt::Write;
use std::path::PathBuf;

fn main() {
    let assets = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/brushes");
    let assets = assets.canonicalize().expect("assets/brushes");
    let catalog = assets.join("catalog.txt");
    println!("cargo:rerun-if-changed={}", catalog.display());
    let text = std::fs::read_to_string(&catalog).expect("assets/brushes/catalog.txt");

    let mut out = String::from("/// (medium, name, author, brush), from assets/brushes/catalog.txt.\npub static BUNDLED: &[(&str, &str, &str, BundledBrush)] = &[\n");
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        let [medium, brush, name] = parts[..] else { panic!("catalog.txt line {}: medium | brush | name", n + 1) };
        let (author, source) = match brush {
            "@round" => ("AuraStudio".to_string(), "BundledBrush::Round".to_string()),
            "@soft" => ("AuraStudio".to_string(), "BundledBrush::Soft".to_string()),
            file => {
                let path = assets.join("mypaint").join(format!("{file}.myb"));
                assert!(path.is_file(), "catalog.txt line {}: no {}", n + 1, path.display());
                println!("cargo:rerun-if-changed={}", path.display());
                let author = match file.split('/').next().unwrap_or("") {
                    "deevad" => "David Revoy",
                    "ramon" => "Ramón Miranda",
                    "tanda" => "Marcelo \"Tanda\" Cerviño",
                    "kaerhon_v1" => "Guillaume Loussarévian",
                    "Dieterle" => "Brien Dieterle",
                    _ => "MyPaint",
                };
                (author.to_string(), format!("BundledBrush::MyPaint(include_str!({:?}))", path.display().to_string()))
            }
        };
        writeln!(out, "    ({medium:?}, {name:?}, {author:?}, {source}),").unwrap();
    }
    out.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("bundled.rs"), out).unwrap();
}

// Writes a Display P3 monitor profile, to try color management without a
// calibrated screen: AURASTUDIO_DISPLAY_ICC=<path> aurastudio
// Usage: cargo run -p aurastudio-color --example write_test_profile -- out.icc
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "display_p3.icc".into());
    let icc = moxcms::ColorProfile::new_display_p3().encode().expect("encode");
    std::fs::write(&path, icc).expect("write");
    println!("wrote {path}");
}

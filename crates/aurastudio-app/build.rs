//! Windows: put the logo in the .exe, so Explorer, the taskbar and
//! shortcuts show it (the window icon is set at runtime, see app.rs), and
//! the file's details (name, version, author). An unsigned .exe without
//! them looks more like malware to antivirus heuristics.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/logo/aurastudio.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/logo/aurastudio.ico");
        let version = env!("CARGO_PKG_VERSION");
        res.set("ProductName", "AuraStudio");
        res.set("FileDescription", "AuraStudio - painting, illustration and animation");
        res.set("CompanyName", "WinterMoss07");
        res.set("LegalCopyright", "© WinterMoss07");
        res.set("OriginalFilename", &format!("aurastudio-{version}.exe"));
        res.set("InternalName", "aurastudio");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no .exe icon: {e}");
        }
    }
}

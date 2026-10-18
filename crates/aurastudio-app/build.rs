//! Windows: put the logo in the .exe, so Explorer, the taskbar and
//! shortcuts show it (the window icon is set at runtime, see app.rs).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/logo/aurastudio.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/logo/aurastudio.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no .exe icon: {e}");
        }
    }
}

//! Finding the monitor's ICC profile.
//!
//! AURASTUDIO_DISPLAY_ICC=<path to .icc/.icm> overrides it on every
//! platform (testing, or a profile the OS doesn't know about).
//! Windows: the profile assigned to the monitor the window is on (Color
//! Management > Devices), or Windows' default (usually sRGB).
//! Other platforms: none yet (treated as sRGB).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayProfile {
    /// File name, for the log and the View menu.
    pub name: String,
    pub icc:  Vec<u8>,
}

/// Profile of the monitor showing window `hwnd` (Windows HWND as isize).
pub fn monitor_profile(hwnd: Option<isize>) -> Option<DisplayProfile> {
    if let Ok(path) = std::env::var("AURASTUDIO_DISPLAY_ICC") {
        return load(&path);
    }
    platform_profile_path(hwnd).and_then(|p| load(&p))
}

fn load(path: &str) -> Option<DisplayProfile> {
    match std::fs::read(path) {
        Ok(icc) => {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_owned());
            Some(DisplayProfile { name, icc })
        }
        Err(e) => {
            log::warn!("display profile {path}: {e}");
            None
        }
    }
}

#[cfg(windows)]
fn platform_profile_path(hwnd: Option<isize>) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
    use windows::Win32::UI::ColorSystem::GetICMProfileW;

    let hwnd = HWND(hwnd.unwrap_or(0) as *mut core::ffi::c_void);
    // SAFETY: plain Win32 calls; the DC is released before returning and
    // the buffer outlives the call that fills it.
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() { return None; }
        let mut len: u32 = 0;
        // First call: required buffer size (in characters).
        let _ = GetICMProfileW(hdc, &mut len, PWSTR::null());
        let mut buf = vec![0u16; len.max(1) as usize];
        let ok = len > 0 && GetICMProfileW(hdc, &mut len, PWSTR(buf.as_mut_ptr())).as_bool();
        ReleaseDC(hwnd, hdc);
        if !ok { return None; }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

#[cfg(not(windows))]
fn platform_profile_path(_hwnd: Option<isize>) -> Option<String> {
    None
}

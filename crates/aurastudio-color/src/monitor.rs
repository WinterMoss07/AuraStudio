//! The monitor's profile: the system's for the monitor under the window
//! (Windows), sRGB, or a chosen file; `AURASTUDIO_DISPLAY_ICC` overrides
//! all (tests, a calibration check).

use std::path::PathBuf;

use crate::{ColorError, Profile};

/// Preferences > Color: where the monitor's profile comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MonitorChoice {
    /// What Windows has for the monitor showing the window.
    #[default]
    System,
    Srgb,
    File(PathBuf),
}

/// The profile for window `hwnd` (a Windows HWND as isize; asked again
/// when the window moves to another monitor). A system profile that is
/// missing or unreadable falls back to sRGB; the error says why, for the
/// log.
pub fn monitor_profile(choice: &MonitorChoice, hwnd: Option<isize>) -> (Profile, Option<ColorError>) {
    let path = std::env::var_os("AURASTUDIO_DISPLAY_ICC").map(PathBuf::from).or(match choice {
        MonitorChoice::Srgb => return (Profile::srgb(), None),
        MonitorChoice::File(p) => Some(p.clone()),
        MonitorChoice::System => platform_profile_path(hwnd).map(PathBuf::from),
    });
    match path.map(|p| Profile::from_file(&p)) {
        Some(Ok(p)) => (p, None),
        Some(Err(e)) => (Profile::srgb(), Some(e)),
        None => (Profile::srgb(), None),
    }
}

#[cfg(windows)]
fn platform_profile_path(hwnd: Option<isize>) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
    use windows::Win32::UI::ColorSystem::GetICMProfileW;

    let hwnd = Some(HWND(hwnd.unwrap_or(0) as *mut core::ffi::c_void));
    // SAFETY: plain Win32 calls; the DC is released before returning and
    // the buffer outlives the call that fills it.
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_invalid() { return None; }
        let mut len: u32 = 0;
        // First call: required buffer size (in characters).
        let _ = GetICMProfileW(hdc, &mut len, None);
        let mut buf = vec![0u16; len.max(1) as usize];
        let ok = len > 0 && GetICMProfileW(hdc, &mut len, Some(PWSTR(buf.as_mut_ptr()))).as_bool();
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

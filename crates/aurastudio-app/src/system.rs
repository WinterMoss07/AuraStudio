//! The few Windows calls the window needs that winit doesn't make: the
//! window's real DPI, a DPI report for the log, the Alt menu turned off.

/// How Windows scales this window, for the log: the process's DPI
/// awareness (per-monitor is needed to follow each monitor's scaling) and
/// the window's current DPI (96 = 100 %).
#[cfg(windows)]
pub fn dpi_report(hwnd: isize) -> String {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::HiDpi::{GetAwarenessFromDpiAwarenessContext, GetDpiForWindow, GetThreadDpiAwarenessContext};
    // SAFETY: plain queries; `hwnd` is the app's live window.
    let (awareness, dpi) = unsafe {
        let a = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext());
        (a.0, GetDpiForWindow(HWND(hwnd as *mut std::ffi::c_void)))
    };
    let name = match awareness { 0 => "unaware", 1 => "system", 2 => "per-monitor", _ => "unknown" };
    format!("DPI awareness {name}, window DPI {dpi} ({:.0} %)", dpi as f32 / 96.0 * 100.0)
}

#[cfg(not(windows))]
pub fn dpi_report(_hwnd: isize) -> String { String::from("DPI report: Windows only") }

/// The window's current DPI as Windows sees it (96 = 100 %), asked
/// directly: winit only learns it from WM_DPICHANGED, which does not come
/// in every case (a 200 % laptop screen to a 100 % HD screen stayed at
/// 200 %).
#[cfg(windows)]
pub fn window_dpi(hwnd: isize) -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    // SAFETY: a plain query on the app's live window.
    let dpi = unsafe { GetDpiForWindow(HWND(hwnd as *mut std::ffi::c_void)) };
    (dpi > 0).then_some(dpi)
}

#[cfg(not(windows))]
pub fn window_dpi(_hwnd: isize) -> Option<u32> { None }

/// Turn off Windows' Alt menu mode (Alt is the picker: it ate shortcuts);
/// Alt+Space and Alt+F4 still work.
#[cfg(windows)]
pub fn disable_alt_menu(hwnd: isize) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{SC_KEYMENU, WM_SYSCOMMAND};
    const SUBCLASS_ID: usize = 0x414C_544D_454E;

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        // lParam: the character typed with Alt (' ' for Alt+Space), 0 for
        // Alt or F10 alone.
        if msg == WM_SYSCOMMAND && (wparam.0 & 0xFFF0) == SC_KEYMENU as usize && lparam.0 != b' ' as isize {
            return LRESULT(0);
        }
        // SAFETY: the arguments are the message being handled.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
    // SAFETY: `proc_` lives for the whole program; the window is the app's.
    unsafe { SetWindowSubclass(HWND(hwnd as *mut std::ffi::c_void), Some(proc_), SUBCLASS_ID, 0) }.as_bool()
}

#[cfg(not(windows))]
pub fn disable_alt_menu(_hwnd: isize) -> bool { false }

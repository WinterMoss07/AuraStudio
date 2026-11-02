//! Windows capture: WinTab on a thread of its own (the interface can never
//! delay the pen), Windows Ink as a hook on the main window when WinTab is
//! missing, and the helpers that tell pen-made mouse events apart.

pub mod ink;
pub mod wintab;

use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo;

/// Where the window's client area sits on the screen (pixels, as f64
/// bits), set from the main thread when the window moves, read by the
/// capture thread for each packet.
#[derive(Default)]
pub struct Origin { x: AtomicI64, y: AtomicI64 }

impl Origin {
    pub fn set(&self, x: f64, y: f64) {
        self.x.store(x.to_bits() as i64, Ordering::Relaxed);
        self.y.store(y.to_bits() as i64, Ordering::Relaxed);
    }
    pub fn get(&self) -> (f64, f64) {
        (f64::from_bits(self.x.load(Ordering::Relaxed) as u64), f64::from_bits(self.y.load(Ordering::Relaxed) as u64))
    }
}

/// The client area's top left on the screen, asked now (main thread).
pub fn client_origin(hwnd: isize) -> Option<(f64, f64)> {
    let mut o = POINT { x: 0, y: 0 };
    // SAFETY: `o` is a valid POINT; a stale handle only makes it fail.
    unsafe { ClientToScreen(HWND(hwnd as *mut c_void), &mut o) }.as_bool().then_some((o.x as f64, o.y as f64))
}

/// The mouse message being handled was made by Windows out of pen or
/// touch input (the `MI_WP_SIGNATURE` in its extra info): dropped, so a
/// pen gesture never arrives twice.
pub fn is_synthetic_mouse() -> bool {
    // SAFETY: no arguments; reads the current message's extra info.
    let extra = unsafe { GetMessageExtraInfo() }.0 as usize;
    crate::synthetic::is_pen_or_touch(extra)
}

/// Counters shared with the capture threads.
#[derive(Default)]
pub struct Counters {
    pub wintab_packets: AtomicU64,
    /// Gaps in WinTab packet serial numbers: packets the driver dropped.
    pub wintab_lost:    AtomicU64,
    pub ink_pen:        AtomicU64,
    pub reopened:       AtomicU64,
}

/// Capture in place.
pub struct Capture {
    pub wintab:   Option<wintab::Thread>,
    pub ink:      Option<ink::Hook>,
    pub origin:   Arc<Origin>,
    pub counters: Arc<Counters>,
}

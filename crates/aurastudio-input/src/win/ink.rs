//! Windows Ink (pointer messages) as a hook on the main window: pointer
//! messages only go to the window under the pointer (Krita does the
//! same). The fallback when WinTab is missing; with WinTab working it only
//! counts pen messages, so a driver set to send both can be told.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crossbeam_queue::SegQueue;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Input::Pointer::{GetPointerDeviceRects, GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType, POINTER_PEN_INFO};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_ROTATION, PEN_MASK_TILT_X, PEN_MASK_TILT_Y,
    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_INPUT_TYPE, PT_PEN, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERLEAVE,
    WM_POINTERUP, WM_POINTERUPDATE,
};

use super::{Counters, Origin};
use crate::clock::{now, TimeMap};
use crate::convert::{finer_if_agrees, himetric_to_screen};
use crate::event::Device;
use crate::state::RawSample;

const SUBCLASS_ID: usize = 0x4155_5241_494E;

struct HookState {
    out:      Arc<SegQueue<RawSample>>,
    origin:   Arc<Origin>,
    counters: Arc<Counters>,
    /// Push samples (false: WinTab captures, only count).
    capture:  bool,
    /// Device and screen rects per pen device (asked once).
    rects:    RefCell<HashMap<isize, ([i32; 4], [i32; 4])>>,
    time:     RefCell<TimeMap>,
    contact:  std::cell::Cell<bool>,
}

fn device_rects(st: &HookState, dev: HANDLE) -> Option<([i32; 4], [i32; 4])> {
    if let Some(r) = st.rects.borrow().get(&(dev.0 as isize)) { return Some(*r); }
    let (mut d, mut s) = (RECT::default(), RECT::default());
    // SAFETY: two valid RECTs for the device the message names.
    unsafe { GetPointerDeviceRects(dev, &mut d, &mut s) }.ok()?;
    let r = ([d.left, d.top, d.right, d.bottom], [s.left, s.top, s.right, s.bottom]);
    st.rects.borrow_mut().insert(dev.0 as isize, r);
    Some(r)
}

fn push(st: &HookState, info: &POINTER_PEN_INFO) {
    let p = &info.pointerInfo;
    let fine = device_rects(st, p.sourceDevice).and_then(|(d, s)| himetric_to_screen((p.ptHimetricLocationRaw.x, p.ptHimetricLocationRaw.y), d, s));
    let screen = finer_if_agrees(fine, (p.ptPixelLocationRaw.x, p.ptPixelLocationRaw.y));
    let (ox, oy) = st.origin.get();
    let contact = p.pointerFlags.0 & POINTER_FLAG_INCONTACT.0 != 0;
    let arrived = now();
    if contact && !st.contact.get() { st.time.borrow_mut().start(p.dwTime, arrived); }
    st.contact.set(contact);
    let time = st.time.borrow_mut().map(p.dwTime, arrived);
    let eraser = info.penFlags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED) != 0;
    let m = info.penMask;
    st.out.push(RawSample {
        device: if eraser { Device::Eraser } else { Device::Pen },
        x: screen.0 - ox, y: screen.1 - oy,
        pressure: if m & PEN_MASK_PRESSURE != 0 { info.pressure as f32 / 1024.0 } else { 1.0 },
        tilt: (if m & PEN_MASK_TILT_X != 0 { info.tiltX as f32 } else { 0.0 }, if m & PEN_MASK_TILT_Y != 0 { info.tiltY as f32 } else { 0.0 }),
        rotation: if m & PEN_MASK_ROTATION != 0 { info.rotation as f32 } else { 0.0 },
        contact,
        in_range: p.pointerFlags.0 & POINTER_FLAG_INRANGE.0 != 0,
        buttons: (info.penFlags & PEN_FLAG_BARREL != 0) as u8,
        time,
    });
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERENTER | WM_POINTERLEAVE) {
        // SAFETY: `data` is the HookState the hook was installed with,
        // alive until the hook is removed.
        let st = unsafe { &*(data as *const HookState) };
        let id = (wparam.0 & 0xffff) as u32;
        let mut kind = POINTER_INPUT_TYPE::default();
        // SAFETY: the pointer id of the message being handled.
        if unsafe { GetPointerType(id, &mut kind) }.is_ok() && kind == PT_PEN {
            st.counters.ink_pen.fetch_add(1, Ordering::Relaxed);
            if st.capture {
                let mut n = 0u32;
                // Every sample since the last message (high-rate pens).
                let history = msg == WM_POINTERUPDATE && unsafe { GetPointerPenInfoHistory(id, &mut n, None) }.is_ok() && n > 1;
                if history {
                    let mut buf = vec![POINTER_PEN_INFO::default(); n.min(256) as usize];
                    let mut got = buf.len() as u32;
                    if unsafe { GetPointerPenInfoHistory(id, &mut got, Some(buf.as_mut_ptr())) }.is_ok() {
                        buf.truncate(got as usize);
                        // (Newest first in the history.)
                        for info in buf.iter().rev() { push(st, info); }
                    }
                } else {
                    let mut info = POINTER_PEN_INFO::default();
                    if unsafe { GetPointerPenInfo(id, &mut info) }.is_ok() { push(st, &info); }
                }
            }
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

pub struct Hook { hwnd: HWND, state: Box<HookState> }

impl Hook {
    pub fn install(hwnd: isize, out: Arc<SegQueue<RawSample>>, origin: Arc<Origin>, counters: Arc<Counters>, capture: bool) -> Result<Hook, String> {
        let hwnd = HWND(hwnd as *mut c_void);
        let state = Box::new(HookState { out, origin, counters, capture, rects: RefCell::new(HashMap::new()), time: RefCell::new(TimeMap::default()), contact: std::cell::Cell::new(false) });
        let data = (&*state as *const HookState) as usize;
        // SAFETY: the window belongs to this thread; `state` lives in the
        // Hook, which removes the subclass before it goes.
        if !unsafe { SetWindowSubclass(hwnd, Some(proc_), SUBCLASS_ID, data) }.as_bool() { return Err("could not hook the window for Windows Ink".into()); }
        Ok(Hook { hwnd, state })
    }

    /// Forget the cached device rects (a display change moved them).
    pub fn refresh(&self) { self.state.rects.borrow_mut().clear(); }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: removing the subclass installed above.
        unsafe { let _ = RemoveWindowSubclass(self.hwnd, Some(proc_), SUBCLASS_ID); }
    }
}

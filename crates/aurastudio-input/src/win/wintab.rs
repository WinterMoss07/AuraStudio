//! WinTab (wintab32.dll, loaded at run time) on a thread of its own, with
//! a hidden message window and its own message loop: packets are drained
//! in batches with the driver's queue enlarged, stamped on the app's
//! clock, and handed over at once; losses are counted from the serial
//! numbers; the context is reopened on display changes and wake.

use std::ffi::{c_char, c_void};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use windows::core::PCSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExA, DefWindowProcA, DestroyWindow, DispatchMessageA, GetMessageA, PostMessageA, PostQuitMessage,
    RegisterClassA, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_DESTROY, WNDCLASSA,
};

use super::{Counters, Origin};
use crate::clock::{now, TimeMap};
use crate::convert::{wintab_tilt, wintab_to_screen, WINTAB_SCALE};
use crate::event::Device;
use crate::state::RawSample;

const WTI_DEFSYSCTX: u32 = 4;
const WTI_DEVICES: u32 = 100;
const DVC_NPRESSURE: u32 = 15;
const DVC_ORIENTATION: u32 = 17;
const CXO_SYSTEM: u32 = 0x0001;
const CXO_MESSAGES: u32 = 0x0004;
const PK_TIME: u32 = 0x0004;
const PK_SERIAL_NUMBER: u32 = 0x0010;
const PK_CURSOR: u32 = 0x0020;
const PK_BUTTONS: u32 = 0x0040;
const PK_X: u32 = 0x0080;
const PK_Y: u32 = 0x0100;
const PK_NORMAL_PRESSURE: u32 = 0x0400;
const PK_ORIENTATION: u32 = 0x1000;
const WT_PACKET: u32 = 0;
const WT_PROXIMITY: u32 = 5;
/// Packets drained at once.
const BATCH: usize = 128;
/// Messages to the capture thread.
const MSG_OVERLAP: u32 = WM_APP + 1;
const MSG_REOPEN: u32 = WM_APP + 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct LogContextA {
    name: [c_char; 40], options: u32, status: u32, locks: u32, message_base: u32,
    device: u32, packet_rate: u32, packet_data: u32, packet_mode: u32,
    move_mask: u32, button_down_mask: u32, button_up_mask: u32,
    input_origin: [i32; 3], input_extent: [i32; 3],
    output_origin: [i32; 3], output_extent: [i32; 3], sensitivity: [i32; 3],
    system_mode: i32, system_origin: [i32; 2], system_extent: [i32; 2],
    system_sensitivity: [i32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Axis { min: i32, max: i32, units: u32, resolution: i32 }

/// One packet in the order of the mask bits (PK_TIME, PK_SERIAL_NUMBER,
/// PK_CURSOR, PK_BUTTONS, PK_X, PK_Y, PK_NORMAL_PRESSURE, PK_ORIENTATION).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Packet { time: u32, serial: u32, cursor: u32, buttons: u32, x: i32, y: i32, pressure: u32, orient: [i32; 3] }

/// The same without PK_ORIENTATION (a tablet without tilt): the driver
/// writes packets of this size, read with `Packet`'s they would shift.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FlatPacket { time: u32, serial: u32, cursor: u32, buttons: u32, x: i32, y: i32, pressure: u32 }

impl From<FlatPacket> for Packet {
    fn from(p: FlatPacket) -> Self {
        Packet { time: p.time, serial: p.serial, cursor: p.cursor, buttons: p.buttons, x: p.x, y: p.y, pressure: p.pressure, orient: [0; 3] }
    }
}

type WtInfoA = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
type WtOpenA = unsafe extern "system" fn(HWND, *mut LogContextA, i32) -> *mut c_void;
type WtClose = unsafe extern "system" fn(*mut c_void) -> i32;
type WtPacketsGet = unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32;
type WtQueueSizeSet = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type WtOverlap = unsafe extern "system" fn(*mut c_void, i32) -> i32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryA(name: *const c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

/// SAFETY: `T` must be the function type of the export named `name`.
unsafe fn load_proc<T: Copy>(module: *mut c_void, name: &'static [u8]) -> Option<T> {
    let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
    if address.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&address) }) }
}

struct Api { module: *mut c_void, info: WtInfoA, open: WtOpenA, close: WtClose, get: WtPacketsGet, queue: WtQueueSizeSet, overlap: Option<WtOverlap> }

impl Api {
    fn load() -> Result<Api, String> {
        // SAFETY: a plain DLL load by name; every export is checked.
        unsafe {
            let module = LoadLibraryA(c"wintab32.dll".as_ptr());
            if module.is_null() { return Err("WinTab driver (wintab32.dll) is not installed".into()); }
            let api = (|| Some(Api {
                module,
                info: load_proc(module, b"WTInfoA\0")?,
                open: load_proc(module, b"WTOpenA\0")?,
                close: load_proc(module, b"WTClose\0")?,
                get: load_proc(module, b"WTPacketsGet\0")?,
                queue: load_proc(module, b"WTQueueSizeSet\0")?,
                overlap: load_proc(module, b"WTOverlap\0"),
            }))();
            match api {
                Some(a) if (a.info)(0, 0, std::ptr::null_mut()) != 0 => Ok(a),
                _ => { FreeLibrary(module); Err("the WinTab driver does not answer".into()) }
            }
        }
    }
}

/// What the capture thread holds.
struct State {
    api:      Api,
    hwnd:     HWND,
    ctx:      *mut c_void,
    message:  u32,
    pressure: (i32, i32),
    tilt:     bool,
    seen_max: i32,
    serial:   Option<u32>,
    contact:  bool,
    last_device: Device,
    /// The virtual desktop the context maps to (origin, extent).
    desk:     [i32; 4],
    /// Where the pen last was (window pixels): leaving range happens there.
    last_at:  (f64, f64),
    time:     TimeMap,
    out:      Arc<crate::Feed>,
    /// Wakes the app's loop: samples waiting.
    waker:    Arc<crate::Waker>,
    origin:   Arc<Origin>,
    counters: Arc<Counters>,
}

impl State {
    /// Open (or reopen) the tablet context on the hidden window.
    fn open(&mut self) -> Result<(), String> {
        if !self.ctx.is_null() {
            // SAFETY: a context we opened.
            unsafe { (self.api.close)(self.ctx) };
            self.ctx = std::ptr::null_mut();
        }
        // SAFETY: WTInfo fills the structures it is given (sizes per spec).
        unsafe {
            let mut c: LogContextA = std::mem::zeroed();
            if (self.api.info)(WTI_DEFSYSCTX, 0, (&mut c as *mut LogContextA).cast()) == 0 { return Err("no WinTab system context".into()); }
            let dev = WTI_DEVICES + if c.device == u32::MAX { 0 } else { c.device };
            let mut p = Axis::default();
            let has_p = (self.api.info)(dev, DVC_NPRESSURE, (&mut p as *mut Axis).cast()) != 0;
            let mut o = [Axis::default(); 3];
            self.tilt = (self.api.info)(dev, DVC_ORIENTATION, o.as_mut_ptr().cast()) != 0;
            self.pressure = if has_p && p.max > p.min { (p.min, p.max) } else { (0, 1023) };
            let mut mask = PK_TIME | PK_SERIAL_NUMBER | PK_CURSOR | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE;
            if self.tilt { mask |= PK_ORIENTATION; }
            self.desk = [c.system_origin[0], c.system_origin[1], c.system_extent[0], c.system_extent[1]];
            c.options |= CXO_SYSTEM | CXO_MESSAGES;
            // The whole virtual desktop, WINTAB_SCALE times finer than a
            // pixel (the driver's own mapping, with fractions).
            c.output_origin[0] = c.system_origin[0] * WINTAB_SCALE;
            c.output_origin[1] = c.system_origin[1] * WINTAB_SCALE;
            c.output_extent[0] = c.system_extent[0] * WINTAB_SCALE;
            c.output_extent[1] = -c.system_extent[1] * WINTAB_SCALE;
            c.packet_data = mask;
            c.packet_mode = 0;
            c.move_mask = mask;
            for (d, s) in c.name.iter_mut().zip(b"AuraStudio\0") { *d = *s as c_char; }
            let ctx = (self.api.open)(self.hwnd, &mut c, 1);
            if ctx.is_null() { return Err("WinTab could not open a tablet context".into()); }
            // The driver's queue is tiny by default: as large as it takes.
            for n in [1024, 512, 256, 128, 64] { if (self.api.queue)(ctx, n) != 0 { break; } }
            self.ctx = ctx;
            self.message = c.message_base;
        }
        self.serial = None;
        Ok(())
    }

    /// The screens changed since the context was opened (a monitor added,
    /// moved or resized, back from sleep): the pen would map to the old
    /// desktop.
    fn desk_changed(&self) -> bool {
        // SAFETY: WTInfo fills the structure it is given.
        unsafe {
            let mut c: LogContextA = std::mem::zeroed();
            (self.api.info)(WTI_DEFSYSCTX, 0, (&mut c as *mut LogContextA).cast()) != 0
                && [c.system_origin[0], c.system_origin[1], c.system_extent[0], c.system_extent[1]] != self.desk
        }
    }

    /// Every packet waiting, in batches.
    fn drain(&mut self) {
        let mut any = false;
        if self.tilt {
            let mut buf = [Packet::default(); BATCH];
            loop {
                // SAFETY: `buf` holds BATCH packets of the layout the mask asks for.
                let n = unsafe { (self.api.get)(self.ctx, BATCH as i32, buf.as_mut_ptr().cast()) };
                if n <= 0 { break; }
                any = true;
                let arrived = now();
                for p in &buf[..n as usize] { self.packet(p, arrived); }
                if (n as usize) < BATCH { break; }
            }
        } else {
            let mut buf = [FlatPacket::default(); BATCH];
            loop {
                // SAFETY: as above, the layout without orientation.
                let n = unsafe { (self.api.get)(self.ctx, BATCH as i32, buf.as_mut_ptr().cast()) };
                if n <= 0 { break; }
                any = true;
                let arrived = now();
                for p in &buf[..n as usize] { self.packet(&Packet::from(*p), arrived); }
                if (n as usize) < BATCH { break; }
            }
        }
        if any { self.waker.wake(); }
    }

    fn packet(&mut self, p: &Packet, arrived: aurastudio_core::Micros) {
        self.counters.wintab_packets.fetch_add(1, Ordering::Relaxed);
        if let Some(prev) = self.serial {
            let gap = p.serial.wrapping_sub(prev).wrapping_sub(1);
            if gap > 0 && gap < 1 << 20 { self.counters.wintab_lost.fetch_add(gap as u64, Ordering::Relaxed); }
        }
        self.serial = Some(p.serial);
        let raw = p.pressure as i32;
        self.seen_max = self.seen_max.max(raw);
        let (lo, hi) = (self.pressure.0, self.pressure.1.max(self.seen_max));
        let pressure = if hi > lo { ((raw - lo) as f32 / (hi - lo) as f32).clamp(0.0, 1.0) } else { 0.0 };
        let contact = p.buttons & 1 != 0;
        if contact && !self.contact { self.time.start(p.time, arrived); }
        self.contact = contact;
        let time = self.time.map(p.time, arrived);
        let (tilt, eraser) = if self.tilt { (wintab_tilt(p.orient[0], p.orient[1]), p.orient[1] < 0) } else { ((0.0, 0.0), false) };
        let eraser = eraser || p.cursor % 3 == 2;
        let (sx, sy) = wintab_to_screen(p.x, p.y);
        let (ox, oy) = self.origin.get();
        let device = if eraser { Device::Eraser } else { Device::Pen };
        self.last_device = device;
        self.last_at = (sx - ox, sy - oy);
        self.out.push(RawSample {
            device,
            x: sx - ox, y: sy - oy, pressure, tilt, rotation: 0.0,
            contact, in_range: true, buttons: ((p.buttons >> 1) & 0x3) as u8, time,
        });
    }
}

/// The capture thread (closed when dropped).
pub struct Thread {
    hwnd:   isize,
    handle: Option<std::thread::JoinHandle<()>>,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = STATE.with_borrow_mut(|s| {
        let Some(s) = s.as_mut() else { return false };
        if msg == s.message + WT_PACKET && lparam.0 as *mut c_void == s.ctx { s.drain(); return true; }
        if msg == s.message + WT_PROXIMITY && wparam.0 as *mut c_void == s.ctx {
            // Leaving the context: the pen is out of range, where it was
            // last (a stroke still down ends there, not at the corner).
            if lparam.0 & 0xffff == 0 {
                let (x, y) = s.last_at;
                s.out.push(RawSample { device: s.last_device, x, y, pressure: 0.0, tilt: (0.0, 0.0), rotation: 0.0, contact: false, in_range: false, buttons: 0, time: now() });
                s.contact = false;
                s.waker.wake();
            }
            return true;
        }
        match msg {
            MSG_OVERLAP => {
                // Back to the front: open again if the screens changed meanwhile.
                if wparam.0 != 0 && s.desk_changed() && s.open().is_ok() { s.counters.reopened.fetch_add(1, Ordering::Relaxed); }
                if let Some(o) = s.api.overlap { unsafe { o(s.ctx, wparam.0 as i32) }; }
                true
            }
            MSG_REOPEN => { if s.open().is_ok() { s.counters.reopened.fetch_add(1, Ordering::Relaxed); } true }
            _ => false,
        }
    });
    if handled { return LRESULT(0); }
    if msg == WM_DESTROY { unsafe { PostQuitMessage(0) }; return LRESULT(0); }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
}

impl Thread {
    /// Start capturing; `Err` (with a reason in plain words) when WinTab is
    /// missing or does not open.
    pub fn start(out: Arc<crate::Feed>, origin: Arc<Origin>, counters: Arc<Counters>, waker: Arc<crate::Waker>) -> Result<Thread, String> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<isize, String>>();
        let handle = std::thread::Builder::new().name("pen input".into()).spawn(move || {
            let api = match Api::load() { Ok(a) => a, Err(e) => { let _ = tx.send(Err(e)); return; } };
            // SAFETY: a message-only window of a class registered here, used
            // and destroyed on this thread only.
            let hwnd = unsafe {
                let inst = GetModuleHandleA(PCSTR::null()).unwrap_or_default();
                let class = WNDCLASSA { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: PCSTR(c"AuraStudioPen".as_ptr().cast()), ..Default::default() };
                RegisterClassA(&class);
                CreateWindowExA(WINDOW_EX_STYLE(0), PCSTR(c"AuraStudioPen".as_ptr().cast()), PCSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(inst.into()), None)
            };
            let Ok(hwnd) = hwnd else { let _ = tx.send(Err("could not make the pen input window".into())); return };
            let mut st = State { api, hwnd, ctx: std::ptr::null_mut(), message: 0, pressure: (0, 1023), tilt: false, seen_max: 0, serial: None, contact: false, last_device: Device::Pen, desk: [0; 4], last_at: (0.0, 0.0), time: TimeMap::default(), out, waker, origin, counters };
            if let Err(e) = st.open() {
                // SAFETY: our window and our module.
                unsafe { let _ = DestroyWindow(hwnd); FreeLibrary(st.api.module); }
                let _ = tx.send(Err(e));
                return;
            }
            STATE.with_borrow_mut(|s| *s = Some(st));
            let _ = tx.send(Ok(hwnd.0 as isize));
            let mut msg = MSG::default();
            // SAFETY: the loop of this thread's own window.
            while unsafe { GetMessageA(&mut msg, None, 0, 0) }.as_bool() { unsafe { DispatchMessageA(&msg) }; }
            STATE.with_borrow_mut(|s| if let Some(s) = s.take() {
                // SAFETY: our context and module.
                unsafe { (s.api.close)(s.ctx); FreeLibrary(s.api.module); }
            });
        }).map_err(|e| e.to_string())?;
        match rx.recv() {
            Ok(Ok(hwnd)) => Ok(Thread { hwnd, handle: Some(handle) }),
            Ok(Err(e)) => { let _ = handle.join(); Err(e) }
            Err(_) => Err("the pen input thread stopped".into()),
        }
    }

    fn post(&self, msg: u32, w: usize) {
        // SAFETY: posting to our own window (the thread may be gone: it fails).
        unsafe { let _ = PostMessageA(Some(HWND(self.hwnd as *mut c_void)), msg, WPARAM(w), LPARAM(0)); }
    }

    /// The app came to the front (context on top) or went to the back.
    pub fn overlap(&self, front: bool) { self.post(MSG_OVERLAP, front as usize); }

    /// Display changed or the computer woke: open the context again.
    pub fn reopen(&self) { self.post(MSG_REOPEN, 0); }
}

impl Drop for Thread {
    fn drop(&mut self) {
        self.post(windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, 0);
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

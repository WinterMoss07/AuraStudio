// Portions of this file (Windows Ink + WinTab hooks) are adapted from
// efude-input (https://github.com/852wa/Efude), © 2026 Hakoniwa,
// licensed MIT OR Apache-2.0. See /NOTICE.md. Vendored (not a crate
// dependency) so this input layer is ours to modify and optimize.

pub struct StylusSample {
    /// Physical client pixels (same unit as winit's CursorMoved).
    pub x:        f32,
    pub y:        f32,
    /// 0..1.
    pub pressure: f32,
    /// Pen timestamp in milliseconds (the driver's clock, wraps after ~49
    /// days): only differences between samples are meaningful.
    pub time_ms:  u32,
}

// ---------------------------------------------------------------
// Windows: native WinTab first (feels more direct with Wacom drivers),
// automatic fallback to Windows Ink (WM_POINTER with high-frequency
// history) when WinTab isn't available.
// ---------------------------------------------------------------
#[cfg(target_os = "windows")]
pub struct Stylus { inner: imp::Inner }

// ---------------------------------------------------------------
// A/B variant: build with `--features efude-upstream` to use the raw
// efude-input crate's WindowsInkHook / WintabHook instead of our vendored
// copy below. Only meant to tell whether some latency comes from the
// native hook itself or from our gpu / brush pipeline; never used in
// normal builds.
#[cfg(all(target_os = "windows", feature = "efude-upstream"))]
mod imp {
    use super::StylusSample;
    use efude_input::{PenInputQueue, WindowsInkHook, WintabHook};
    use windows::Win32::{Foundation::HWND, UI::HiDpi::GetDpiForWindow};

    pub struct Inner {
        queue: PenInputQueue,
        hwnd:  isize,
        hook:  Hook,
    }

    enum Hook { Wintab(WintabHook), Ink(WindowsInkHook), None }

    pub fn install(hwnd_raw: isize) -> Inner {
        let hwnd = hwnd_raw as *mut std::ffi::c_void;
        let queue = PenInputQueue::default();

        let hook = match WintabHook::install(hwnd, queue.shared()) {
            Ok(h) => Hook::Wintab(h),
            Err(_) => match WindowsInkHook::install(hwnd, queue.shared()) {
                Ok(h) => Hook::Ink(h),
                Err(_) => Hook::None,
            },
        };

        Inner { queue, hwnd: hwnd_raw, hook }
    }

    pub fn backend(inner: &Inner) -> &'static str {
        match inner.hook {
            Hook::Wintab(_) => "WinTab (efude-input upstream)",
            Hook::Ink(_)    => "Windows Ink (efude-input upstream)",
            Hook::None      => "none",
        }
    }

    pub fn drain(inner: &Inner) -> Vec<StylusSample> {
        // efude-input reports logical pixels; scale back to physical
        // pixels so the A/B comparison draws at the right place.
        let hwnd  = HWND(inner.hwnd as *mut std::ffi::c_void);
        let scale = unsafe { GetDpiForWindow(hwnd) }.max(1) as f32 / 96.0;
        let mut buf = Vec::new();
        inner.queue.drain_into(&mut buf, 256);
        // No pen timestamp in this API: stamped at drain time.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32).unwrap_or(0);
        buf.into_iter()
            .map(|p| StylusSample { x: p.x * scale, y: p.y * scale, pressure: p.pressure, time_ms: now })
            .collect()
    }
}

#[cfg(target_os = "windows")]
impl Stylus {
    pub fn install(hwnd: isize) -> Self {
        Self { inner: imp::install(hwnd) }
    }
    pub fn drain(&self) -> Vec<StylusSample> {
        imp::drain(&self.inner)
    }
    /// Which native pen API got hooked: shown in the log at startup.
    pub fn backend(&self) -> &'static str {
        imp::backend(&self.inner)
    }
}

#[cfg(all(target_os = "windows", not(feature = "efude-upstream")))]
mod imp {
    use super::StylusSample;
    use crossbeam_queue::ArrayQueue;
    use std::ffi::c_void;
    use std::sync::Arc;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::ScreenToClient,
        UI::{
            Input::Pointer::{GetPointerPenInfo, GetPointerPenInfoHistory, POINTER_PEN_INFO},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, WM_POINTERDOWN, WM_POINTERUP,
                WM_POINTERUPDATE,
            },
        },
    };

    #[derive(Clone, Copy)]
    struct PenPacket { x: f32, y: f32, pressure: f32, time_ms: u32 }

    /// Queue a packet; when full, drop the oldest one (the newest pen
    /// position matters most).
    fn push_latest(queue: &ArrayQueue<PenPacket>, packet: PenPacket) {
        if queue.push(packet).is_err() {
            let _ = queue.pop();
            let _ = queue.push(packet);
        }
    }

    pub struct Inner {
        queue: Arc<ArrayQueue<PenPacket>>,
        hook:  Hook,
    }

    enum Hook { Wintab(wintab::WintabHook), Ink(WindowsInkHook), None }

    pub fn install(hwnd_raw: isize) -> Inner {
        let queue: Arc<ArrayQueue<PenPacket>> = Arc::new(ArrayQueue::new(16_384));
        let hwnd  = hwnd_raw as *mut c_void;

        let hook = match wintab::WintabHook::install(hwnd, Arc::clone(&queue)) {
            Ok(h) => Hook::Wintab(h),
            Err(_) => match WindowsInkHook::install(hwnd, Arc::clone(&queue)) {
                Ok(h) => Hook::Ink(h),
                Err(_) => Hook::None,
            },
        };

        Inner { queue, hook }
    }

    pub fn backend(inner: &Inner) -> &'static str {
        match inner.hook {
            Hook::Wintab(_) => "WinTab",
            Hook::Ink(_)    => "Windows Ink",
            Hook::None      => "none",
        }
    }

    pub fn drain(inner: &Inner) -> Vec<StylusSample> {
        let mut out = Vec::new();
        while let Some(p) = inner.queue.pop() {
            out.push(StylusSample { x: p.x, y: p.y, pressure: p.pressure, time_ms: p.time_ms });
        }
        out
    }

    // --- Windows Ink (WM_POINTER) -------------------------------

    struct HookState { queue: Arc<ArrayQueue<PenPacket>> }
    const SUBCLASS_ID: usize = 0x4546554445;

    fn push_info(hwnd: HWND, state: &HookState, info: &POINTER_PEN_INFO) {
        let mut point = info.pointerInfo.ptPixelLocation;
        // Physical client pixels, same unit as winit's CursorMoved and the
        // wgpu surface. efude divides by the DPI scale (its UI works in
        // logical pixels); doing that here put pen samples at 1/scale of
        // their real position on high-DPI screens.
        if unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
            let packet = PenPacket {
                x: point.x as f32,
                y: point.y as f32,
                pressure: if info.penMask & PEN_MASK_PRESSURE != 0 {
                    info.pressure as f32 / 1024.
                } else { 1. },
                time_ms: info.pointerInfo.dwTime,
            };
            push_latest(&state.queue, packet);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize,
    ) -> LRESULT {
        if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
            let pointer_id = (wparam.0 & 0xffff) as u32;
            let state = unsafe { &*(data as *const HookState) };
            let mut history_count = 0u32;
            if msg == WM_POINTERUPDATE
                && unsafe { GetPointerPenInfoHistory(pointer_id, &mut history_count, None) }.is_ok()
                && history_count > 0
            {
                let capacity = history_count.min(256) as usize;
                let mut history = vec![POINTER_PEN_INFO::default(); capacity];
                let mut available = capacity as u32;
                if unsafe {
                    GetPointerPenInfoHistory(pointer_id, &mut available, Some(history.as_mut_ptr()))
                }.is_ok() {
                    history.truncate(available.min(capacity as u32) as usize);
                    for info in history.iter().rev() {
                        push_info(hwnd, state, info);
                    }
                    return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
                }
            }
            let mut info = POINTER_PEN_INFO::default();
            if unsafe { GetPointerPenInfo(pointer_id, &mut info) }.is_ok() {
                push_info(hwnd, state, &info);
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub struct WindowsInkHook { hwnd: HWND, state: Box<HookState> }
    impl WindowsInkHook {
        pub fn install(hwnd: *mut c_void, queue: Arc<ArrayQueue<PenPacket>>) -> Result<Self, windows::core::Error> {
            let hwnd  = HWND(hwnd);
            let state = Box::new(HookState { queue });
            let data  = (&*state as *const HookState) as usize;
            if !unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, data) }.as_bool() {
                return Err(windows::core::Error::from_win32());
            }
            Ok(Self { hwnd, state })
        }
    }
    impl Drop for WindowsInkHook {
        fn drop(&mut self) {
            let _keep_alive = &self.state;
            unsafe { let _ = RemoveWindowSubclass(self.hwnd, Some(window_proc), SUBCLASS_ID); }
        }
    }

    // --- WinTab (wintab32.dll, loaded at runtime) -------------------

    mod wintab {
        use super::{c_void, push_latest, Arc, ArrayQueue, DefSubclassProc, HWND, LPARAM,
                     LRESULT, PenPacket, POINT, RemoveWindowSubclass, ScreenToClient,
                     SetWindowSubclass, WPARAM};
        use std::ffi::c_char;

        const WTI_DEFSYSCTX: u32 = 4;
        const WTI_DEVICES: u32 = 100;
        const DVC_NPRESSURE: u32 = 15;
        const DVC_ORIENTATION: u32 = 17;
        const CXO_SYSTEM: u32 = 0x0001;
        const CXO_MESSAGES: u32 = 0x0004;
        const PK_TIME: u32 = 0x0004;
        const PK_BUTTONS: u32 = 0x0040;
        const PK_X: u32 = 0x0080;
        const PK_Y: u32 = 0x0100;
        const PK_NORMAL_PRESSURE: u32 = 0x0400;
        const PK_ORIENTATION: u32 = 0x1000;
        const WT_PACKET_OFFSET: u32 = 0;
        const SUBCLASS_ID: usize = 0x454655445746;
        const PACKET_WORDS: usize = 16;

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
        impl Default for LogContextA {
            fn default() -> Self { unsafe { std::mem::zeroed() } }
        }

        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct Axis { min: i32, max: i32, units: u32, resolution: i32 }

        type WtInfoA = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
        type WtOpenA = unsafe extern "system" fn(HWND, *mut LogContextA, i32) -> *mut c_void;
        type WtPacket = unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32;
        type WtClose = unsafe extern "system" fn(*mut c_void) -> i32;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LoadLibraryA(name: *const c_char) -> *mut c_void;
            fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
            fn FreeLibrary(module: *mut c_void) -> i32;
        }

        unsafe fn load_proc<T: Copy>(module: *mut c_void, name: &'static [u8]) -> Option<T> {
            let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
            if address.is_null() { None } else { Some(unsafe { std::mem::transmute_copy(&address) }) }
        }

        #[derive(Clone, Copy, Default)]
        struct AxisRange { min: i32, max: i32 }

        struct HookState {
            queue: Arc<ArrayQueue<PenPacket>>,
            module: *mut c_void,
            context: *mut c_void,
            packet_fn: WtPacket,
            close_fn: WtClose,
            message: u32,
            packet_mask: u32,
            pressure_range: AxisRange,
            pressure_seen_max: std::cell::Cell<i32>,
            /// Tilt / rotation axes, read at install; kept for tilt support
            /// in brushes (not used yet).
            #[allow(dead_code)]
            orientation: [AxisRange; 3],
        }

        fn field_offset(mask: u32, target: u32) -> Option<usize> {
            let mut offset = 0;
            for bit_index in 0..32 {
                let bit = 1u32 << bit_index;
                if mask & bit == 0 { continue; }
                if bit == target { return Some(offset); }
                let size = match bit {
                    PK_TIME | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE => 4,
                    PK_ORIENTATION => 12,
                    _ => return None,
                };
                offset += size;
            }
            None
        }

        fn packet_u32(words: &[u32; PACKET_WORDS], offset: usize) -> u32 { words[offset / 4] }
        fn packet_i32(words: &[u32; PACKET_WORDS], offset: usize) -> i32 { words[offset / 4] as i32 }
        fn normalized(value: i32, range: AxisRange) -> f32 {
            let span = range.max.saturating_sub(range.min);
            if span <= 0 { 0.0 } else {
                (value.saturating_sub(range.min) as f32 / span as f32).clamp(0.0, 1.0)
            }
        }

        fn push_packet(hwnd: HWND, state: &HookState, serial: u32) {
            let mut words = [0u32; PACKET_WORDS];
            if unsafe { (state.packet_fn)(state.context, serial, words.as_mut_ptr()) } == 0 { return; }
            let (Some(time_offset), Some(x_offset), Some(y_offset)) = (
                field_offset(state.packet_mask, PK_TIME),
                field_offset(state.packet_mask, PK_X),
                field_offset(state.packet_mask, PK_Y),
            ) else { return; };
            let time_ms = packet_u32(&words, time_offset);
            let mut screen = POINT { x: packet_i32(&words, x_offset), y: packet_i32(&words, y_offset) };
            if !unsafe { ScreenToClient(hwnd, &mut screen) }.as_bool() { return; }
            let pressure = field_offset(state.packet_mask, PK_NORMAL_PRESSURE)
                .map(|offset| {
                    let raw = packet_u32(&words, offset) as i32;
                    if raw > state.pressure_seen_max.get() { state.pressure_seen_max.set(raw); }
                    let range = AxisRange {
                        min: state.pressure_range.min,
                        max: state.pressure_range.max.max(state.pressure_seen_max.get()),
                    };
                    normalized(raw, range)
                })
                .unwrap_or(1.0);
            // Physical client pixels (see push_info in the Windows Ink section).
            let packet = PenPacket { x: screen.x as f32, y: screen.y as f32, pressure, time_ms };
            push_latest(&state.queue, packet);
        }

        unsafe extern "system" fn window_proc(
            hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize,
        ) -> LRESULT {
            let state = unsafe { &*(data as *const HookState) };
            if msg == state.message && lparam.0 as *mut c_void == state.context {
                push_packet(hwnd, state, wparam.0 as u32);
            }
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }

        pub struct WintabHook { hwnd: HWND, state: Box<HookState> }

        impl WintabHook {
            pub fn install(hwnd: *mut c_void, queue: Arc<ArrayQueue<PenPacket>>) -> Result<Self, String> {
                let module = unsafe { LoadLibraryA(c"wintab32.dll".as_ptr()) };
                if module.is_null() {
                    return Err("Wintab driver (wintab32.dll) is unavailable".into());
                }
                let result = (|| unsafe {
                    let wt_info: WtInfoA = load_proc(module, b"WTInfoA\0")
                        .ok_or_else(|| "Wintab WTInfoA export is unavailable".to_string())?;
                    let wt_open: WtOpenA = load_proc(module, b"WTOpenA\0")
                        .ok_or_else(|| "Wintab WTOpenA export is unavailable".to_string())?;
                    let packet_fn: WtPacket = load_proc(module, b"WTPacket\0")
                        .ok_or_else(|| "Wintab WTPacket export is unavailable".to_string())?;
                    let close_fn: WtClose = load_proc(module, b"WTClose\0")
                        .ok_or_else(|| "Wintab WTClose export is unavailable".to_string())?;
                    if wt_info(0, 0, std::ptr::null_mut()) == 0 {
                        return Err("Wintab driver did not respond to WTInfo".into());
                    }
                    let mut context = LogContextA::default();
                    if wt_info(WTI_DEFSYSCTX, 0, (&mut context as *mut LogContextA).cast()) == 0 {
                        return Err("Wintab system context is unavailable".into());
                    }
                    let device_category = WTI_DEVICES.saturating_add(
                        if context.device == u32::MAX { 0 } else { context.device }
                    );
                    let mut pressure_axis = Axis::default();
                    let pressure_supported = wt_info(
                        device_category, DVC_NPRESSURE, (&mut pressure_axis as *mut Axis).cast(),
                    ) != 0;
                    let mut orientation_axes = [Axis::default(); 3];
                    let orientation_supported = wt_info(
                        device_category, DVC_ORIENTATION, orientation_axes.as_mut_ptr().cast(),
                    ) != 0;

                    let mut packet_mask = PK_TIME | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE;
                    if orientation_supported { packet_mask |= PK_ORIENTATION; }
                    context.options |= CXO_SYSTEM | CXO_MESSAGES;
                    context.output_origin[0] = context.system_origin[0];
                    context.output_origin[1] = context.system_origin[1];
                    context.output_extent[0] = context.system_extent[0];
                    context.output_extent[1] = -context.system_extent[1];
                    context.packet_data = packet_mask;
                    context.packet_mode = 0;
                    context.move_mask = packet_mask & (PK_X | PK_Y | PK_NORMAL_PRESSURE | PK_ORIENTATION);
                    context.name[..11].copy_from_slice(&b"AuraStudio\0".map(|b| b as c_char));
                    let hwnd = HWND(hwnd);
                    let context_handle = wt_open(hwnd, &mut context, 1);
                    if context_handle.is_null() {
                        return Err("Wintab could not open a tablet context".into());
                    }
                    let state = Box::new(HookState {
                        queue, module, context: context_handle, packet_fn, close_fn,
                        message: context.message_base + WT_PACKET_OFFSET,
                        packet_mask,
                        pressure_range: if pressure_supported && pressure_axis.max > pressure_axis.min {
                            AxisRange { min: pressure_axis.min, max: pressure_axis.max }
                        } else {
                            AxisRange { min: 0, max: 1023 }
                        },
                        pressure_seen_max: std::cell::Cell::new(0),
                        orientation: orientation_axes.map(|a| AxisRange { min: a.min, max: a.max }),
                    });
                    let data = (&*state as *const HookState) as usize;
                    if !SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, data).as_bool() {
                        let _ = (state.close_fn)(state.context);
                        return Err("Could not attach Wintab messages to the application window".into());
                    }
                    Ok(Self { hwnd, state })
                })();
                if result.is_err() { unsafe { FreeLibrary(module) }; }
                result
            }
        }
        impl Drop for WintabHook {
            fn drop(&mut self) {
                unsafe {
                    let _ = RemoveWindowSubclass(self.hwnd, Some(window_proc), SUBCLASS_ID);
                    let _ = (self.state.close_fn)(self.state.context);
                    let _ = FreeLibrary(self.state.module);
                }
            }
        }
    }
}

// ---------------------------------------------------------------
// macOS / Linux: no native hook yet; the app falls back to winit's Touch
// events (see app.rs).
// ---------------------------------------------------------------
#[cfg(not(target_os = "windows"))]
pub struct Stylus;

#[cfg(not(target_os = "windows"))]
impl Stylus {
    pub fn install(_hwnd: isize) -> Self { Self }
    pub fn drain(&self) -> Vec<StylusSample> { Vec::new() }
    pub fn backend(&self) -> &'static str { "none (winit touch events)" }
}

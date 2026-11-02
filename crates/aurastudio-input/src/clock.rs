//! The app's one clock, in microseconds: QPC on Windows, a monotonic
//! clock elsewhere. Every device's samples are stamped on it, so speed,
//! smoothing and latency compare like with like.

use aurastudio_core::Micros;

#[cfg(windows)]
pub fn now() -> Micros {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    // SAFETY: both write one i64 each.
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 { return Micros(0); }
    Micros((c as i128 * 1_000_000 / f as i128) as u64)
}

#[cfg(not(windows))]
pub fn now() -> Micros {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    Micros(START.get_or_init(Instant::now).elapsed().as_micros() as u64)
}

/// A device's own sample times (WinTab: milliseconds on the driver's
/// clock) mapped to the app's clock by an offset fixed at stroke start
/// (Blender's way): samples drained together keep their real spacing.
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeMap {
    offset: Option<i64>,
    last:   Option<u32>,
}

impl TimeMap {
    /// Fix the offset from a sample's device time and its arrival.
    pub fn start(&mut self, device_ms: u32, arrived: Micros) {
        self.offset = Some(arrived.0 as i64 - device_ms as i64 * 1000);
        self.last = Some(device_ms);
    }

    /// The app time of a device time (the last offset; arrival time if
    /// none was fixed yet). Never earlier than `arrived` minus 1 s, never
    /// later than `arrived` (a driver clock that jumps is not trusted).
    pub fn map(&mut self, device_ms: u32, arrived: Micros) -> Micros {
        // (The driver's clock wraps after ~49 days: a backward jump past
        // half its range is a wrap.)
        if let Some(l) = self.last { if device_ms < l && l - device_ms > u32::MAX / 2 { self.offset = None; } }
        self.last = Some(device_ms);
        let Some(o) = self.offset else { self.start(device_ms, arrived); return arrived };
        let t = device_ms as i64 * 1000 + o;
        let a = arrived.0 as i64;
        if t > a || t < a - 1_000_000 { self.start(device_ms, arrived); return arrived; }
        Micros(t as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drained_samples_keep_their_spacing() {
        let mut m = TimeMap::default();
        m.start(10_000, Micros(5_000_000));
        // Three samples 5 ms apart, all drained at once 20 ms later.
        let arrived = Micros(5_030_000);
        let t: Vec<u64> = [10_005, 10_010, 10_015].iter().map(|d| m.map(*d, arrived).0).collect();
        assert_eq!(t, vec![5_005_000, 5_010_000, 5_015_000]);
        // A clock that jumps ahead of the arrival is re-anchored.
        assert_eq!(m.map(90_000, Micros(5_040_000)), Micros(5_040_000));
        assert!(now() <= now());
    }
}

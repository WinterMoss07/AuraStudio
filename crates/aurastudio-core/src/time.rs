//! Real time and film time: two different types, never mixed.

/// Real time in microseconds on one clock for the whole app (QPC on
/// Windows, read by `aurastudio-input`): pen samples, frames, latency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Micros(pub u64);

impl Micros {
    #[inline]
    pub fn since(self, earlier: Micros) -> Micros { Micros(self.0.saturating_sub(earlier.0)) }

    #[inline]
    pub fn as_secs_f64(self) -> f64 { self.0 as f64 * 1e-6 }
}

/// A film frame number (0 = the first frame of the timeline).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameNo(pub u32);

/// Frames per second as a fraction: 24/1, 25/1, 24000/1001 (23.976) —
/// no drift over a long film, stored as is in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fps {
    pub num: u32,
    pub den: u32,
}

impl Default for Fps {
    fn default() -> Self { Fps::new(24, 1) }
}

impl Fps {
    /// `den` 0 is read as 1.
    pub const fn new(num: u32, den: u32) -> Fps { Fps { num, den: if den == 0 { 1 } else { den } } }

    pub fn as_f64(self) -> f64 { self.num as f64 / self.den as f64 }

    /// When `frame` starts, in microseconds from frame 0 (exact up to the
    /// microsecond, computed from the fraction).
    #[cfg(test)]
    pub fn start_of(self, frame: FrameNo) -> Micros {
        if self.num == 0 { return Micros(0); }
        Micros((frame.0 as u128 * self.den as u128 * 1_000_000 / self.num as u128) as u64)
    }

    /// The frame shown at `t` from frame 0.
    pub fn frame_at(self, t: Micros) -> FrameNo {
        if self.den == 0 { return FrameNo(0); }
        FrameNo((t.0 as u128 * self.num as u128 / (self.den as u128 * 1_000_000)).min(u32::MAX as u128) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_film_does_not_drift() {
        let f = Fps::new(24000, 1001);
        // One hour of 23.976: frame 86313 starts at 3599.9 s.
        let t = f.start_of(FrameNo(86_313));
        assert_eq!(t.0, 86_313u64 * 1001 * 1_000_000 / 24_000);
        assert_eq!(f.frame_at(t), FrameNo(86_313));
        assert_eq!(Fps::new(24, 1).frame_at(Micros(1_000_000)), FrameNo(24));
        assert_eq!(Fps::new(24, 0).den, 1);
    }
}

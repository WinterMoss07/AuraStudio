//! A hash that never changes (FNV-1a, 64 bits): keys written to disk must
//! mean the same thing in every build, which `DefaultHasher` does not
//! promise.

const START: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self { Self(START) }
}

impl Fnv {
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for b in bytes { self.0 ^= *b as u64; self.0 = self.0.wrapping_mul(PRIME); }
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn u64(&mut self, v: u64) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn f32(&mut self, v: f32) -> &mut Self { self.u32(v.to_bits()) }
    pub fn bool(&mut self, v: bool) -> &mut Self { self.bytes(&[v as u8]) }
    /// Text, closed by a zero byte so ("ab", "c") differs from ("a", "bc").
    pub fn str(&mut self, s: &str) -> &mut Self { self.bytes(s.as_bytes()).bytes(&[0]) }
    pub fn finish(&self) -> u64 { self.0 }
}

/// The hash of a value's `Debug` text: any field added to the type enters
/// it by itself.
pub fn of_debug<T: std::fmt::Debug>(v: &T) -> u64 {
    let mut h = Fnv::default();
    h.bytes(format!("{v:?}").as_bytes());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_never_change() {
        // FNV-1a 64 reference values.
        assert_eq!(Fnv::default().finish(), 0xcbf2_9ce4_8422_2325);
        assert_eq!(Fnv::default().bytes(b"a").finish(), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(Fnv::default().bytes(b"foobar").finish(), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn text_is_closed_so_neighbours_do_not_merge() {
        let a = Fnv::default().str("ab").str("c").finish();
        let b = Fnv::default().str("a").str("bc").finish();
        assert_ne!(a, b);
    }
}

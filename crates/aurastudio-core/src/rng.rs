//! Deterministic randomness and hashing: the same seed gives the same
//! numbers on every machine and build (replays, fuzz seeds, render noise),
//! and `Fnv` keys written to disk mean the same thing in every build,
//! which `DefaultHasher` does not promise.

/// A small fast generator (xorshift64*): fuzzers, scenarios, jitter.
/// Not for anything secret.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// Any seed; 0 is replaced (xorshift would stay at 0).
    pub fn new(seed: u64) -> Self { Rng(if seed == 0 { 0x9e37_79b9_7f4a_7c15 } else { seed }) }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// 0..n (0 when n is 0).
    pub fn below(&mut self, n: u64) -> u64 { if n == 0 { 0 } else { self.next_u64() % n } }

    /// 0..1, 24 bits.
    pub fn unit(&mut self) -> f32 { (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32 }

    pub fn range(&mut self, a: f32, b: f32) -> f32 { a + (b - a) * self.unit() }

    pub fn chance(&mut self, p: f32) -> bool { self.unit() < p }

    /// One of `items` (`None` when empty).
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() { None } else { items.get(self.below(items.len() as u64) as usize) }
    }
}

/// A well-mixed 64-bit hash of a 64-bit value (splitmix64's finalizer):
/// noise from coordinates without any state, same everywhere.
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Noise 0..1 for (seed, x, y): textures, dither, grain.
#[inline]
pub fn noise(seed: u64, x: i32, y: i32) -> f32 {
    let h = mix64(seed ^ mix64(((x as u32 as u64) << 32) | y as u32 as u64));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

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
    fn generator_is_deterministic() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..100 { assert_eq!(a.next_u64(), b.next_u64()); }
        let mut z = Rng::new(0);
        assert_ne!(z.next_u64(), 0);
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!(r.below(5) < 5);
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn noise_is_stable_and_spread() {
        assert_eq!(noise(1, 10, -3), noise(1, 10, -3));
        assert_ne!(noise(1, 10, -3), noise(2, 10, -3));
        let mean: f32 = (0..64).flat_map(|x| (0..64).map(move |y| noise(9, x, y))).sum::<f32>() / 4096.0;
        assert!((mean - 0.5).abs() < 0.03, "{mean}");
    }

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

//! Little-endian bytes in and out, varints and a magic + version header:
//! the one way every binary format of the app is written (surfaces, undo
//! patches, the strokelog, documents). Reading never panics: a damaged
//! input gives `None`.

pub struct ByteWriter { pub buf: Vec<u8> }

impl Default for ByteWriter {
    fn default() -> Self { Self::new() }
}

impl ByteWriter {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    /// Start with `magic` and `version`.
    pub fn with_header(magic: &[u8; 4], version: u16) -> Self {
        let mut w = Self::new();
        w.bytes(magic);
        w.u16(version);
        w
    }

    pub fn u8(&mut self, v: u8) -> &mut Self { self.buf.push(v); self }
    pub fn u16(&mut self, v: u16) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn u32(&mut self, v: u32) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn u64(&mut self, v: u64) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn i32(&mut self, v: i32) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn f32(&mut self, v: f32) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn f64(&mut self, v: f64) -> &mut Self { self.bytes(&v.to_le_bytes()) }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self { self.buf.extend_from_slice(b); self }

    /// LEB128: 1 byte up to 127.
    pub fn varint(&mut self, mut v: u64) -> &mut Self {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 { self.buf.push(b); return self; }
            self.buf.push(b | 0x80);
        }
    }

    /// Signed varint (zigzag).
    pub fn svarint(&mut self, v: i64) -> &mut Self { self.varint(((v << 1) ^ (v >> 63)) as u64) }

    /// Length (varint) then the bytes.
    pub fn blob(&mut self, b: &[u8]) -> &mut Self { self.varint(b.len() as u64); self.bytes(b) }

    pub fn str(&mut self, s: &str) -> &mut Self { self.blob(s.as_bytes()) }

    pub fn finish(self) -> Vec<u8> { self.buf }
}

pub struct ByteReader<'a> { b: &'a [u8], at: usize }

impl<'a> ByteReader<'a> {
    pub fn new(b: &'a [u8]) -> Self { Self { b, at: 0 } }

    /// Check `magic` and return the version (`None`: another format).
    pub fn header(&mut self, magic: &[u8; 4]) -> Option<u16> {
        if self.bytes(4)? != magic { return None; }
        self.u16()
    }

    pub fn remaining(&self) -> usize { self.b.len() - self.at }
    pub fn at_end(&self) -> bool { self.at == self.b.len() }
    pub fn position(&self) -> usize { self.at }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> { self.bytes(N)?.try_into().ok() }

    pub fn u8(&mut self) -> Option<u8> { Some(self.arr::<1>()?[0]) }
    pub fn u16(&mut self) -> Option<u16> { Some(u16::from_le_bytes(self.arr()?)) }
    pub fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.arr()?)) }
    pub fn u64(&mut self) -> Option<u64> { Some(u64::from_le_bytes(self.arr()?)) }
    pub fn i32(&mut self) -> Option<i32> { Some(i32::from_le_bytes(self.arr()?)) }
    pub fn f32(&mut self) -> Option<f32> { Some(f32::from_le_bytes(self.arr()?)) }
    pub fn f64(&mut self) -> Option<f64> { Some(f64::from_le_bytes(self.arr()?)) }

    pub fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 { return Some(v); }
        }
        None
    }

    pub fn svarint(&mut self) -> Option<i64> { let u = self.varint()?; Some((u >> 1) as i64 ^ -((u & 1) as i64)) }

    /// A `blob`, at most `max` bytes (a damaged length fails here, before
    /// anything is allocated).
    pub fn blob(&mut self, max: usize) -> Option<&'a [u8]> {
        let n = self.varint()? as usize;
        if n > max { return None; }
        self.bytes(n)
    }

    pub fn str(&mut self, max: usize) -> Option<&'a str> { std::str::from_utf8(self.blob(max)?).ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damage() {
        let mut w = ByteWriter::with_header(b"TEST", 3);
        w.u8(7).u16(500).u32(70_000).u64(u64::MAX).i32(-5).f32(1.5).f64(-2.25).varint(300).svarint(-77).str("héllo").blob(&[1, 2, 3]);
        let b = w.finish();
        let mut r = ByteReader::new(&b);
        assert_eq!(r.header(b"TEST"), Some(3));
        assert_eq!((r.u8(), r.u16(), r.u32(), r.u64(), r.i32()), (Some(7), Some(500), Some(70_000), Some(u64::MAX), Some(-5)));
        assert_eq!((r.f32(), r.f64(), r.varint(), r.svarint()), (Some(1.5), Some(-2.25), Some(300), Some(-77)));
        assert_eq!(r.str(100), Some("héllo"));
        assert_eq!(r.blob(2), None, "longer than allowed");
        assert!(ByteReader::new(&b).header(b"NOPE").is_none());
        let mut cut = ByteReader::new(&b[..9]);
        cut.header(b"TEST");
        assert_eq!(cut.u32(), None);
        let mut bad = ByteReader::new(&[0xff; 12]);
        assert_eq!(bad.varint(), None, "never ends");
    }
}

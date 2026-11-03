//! Procreate's brush files are ZIP archives: their entries, read in memory
//! with a size cap each (the `zip` crate, as `file` uses).

use std::cell::RefCell;
use std::io::{Cursor, Read};

/// One entry of the archive.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    index:    usize,
}

/// An archive held in memory.
pub struct ZipReader<'a> {
    archive:     RefCell<zip::ZipArchive<Cursor<&'a [u8]>>>,
    /// Every entry, in the archive's order.
    pub entries: Vec<Entry>,
}

impl<'a> ZipReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut entries = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            // (A name that isn't text names nothing a brush file uses.)
            let Ok(f) = archive.by_index_raw(index) else { continue };
            let Ok(name) = f.name() else { continue };
            entries.push(Entry { name: name.into_owned(), index });
        }
        Ok(Self { archive: RefCell::new(archive), entries })
    }

    pub fn find(&self, name: &str) -> Option<&Entry> { self.entries.iter().find(|e| e.name == name) }

    /// An entry's content, at most `max` bytes (a larger one, or one
    /// that inflates past what it says, is refused), CRC-checked.
    pub fn read(&self, e: &Entry, max: usize) -> Result<Vec<u8>, String> {
        let mut archive = self.archive.borrow_mut();
        let mut f = archive.by_index(e.index).map_err(|err| format!("zip entry `{}`: {err}", e.name))?;
        if f.size() > max as u64 { return Err(format!("zip entry `{}` is {} bytes (max {max})", e.name, f.size())); }
        let mut out = Vec::with_capacity(f.size() as usize);
        // (Reading to the end checks the CRC.)
        f.by_ref().take(max as u64 + 1).read_to_end(&mut out).map_err(|err| format!("zip entry `{}`: {err}", e.name))?;
        if out.len() as u64 != f.size() { return Err(format!("zip entry `{}` has the wrong size", e.name)); }
        Ok(out)
    }

    pub fn read_by_name(&self, name: &str, max: usize) -> Result<Vec<u8>, String> {
        let e = self.find(name).ok_or_else(|| format!("no `{name}` in the file"))?;
        self.read(e, max)
    }
}

/// Test archives: stored entries.
#[cfg(test)]
pub struct ZipWriter(zip::ZipWriter<Cursor<Vec<u8>>>);

#[cfg(test)]
impl ZipWriter {
    pub fn new() -> Self { Self(zip::ZipWriter::new(Cursor::new(Vec::new()))) }

    pub fn add(&mut self, name: &str, data: &[u8]) -> zip::result::ZipResult<()> {
        use std::io::Write;
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        self.0.start_file(name, o)?;
        self.0.write_all(data)?;
        Ok(())
    }

    pub fn finish(self) -> zip::result::ZipResult<Vec<u8>> { Ok(self.0.finish()?.into_inner()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_back_within_their_cap() {
        let mut z = ZipWriter::new();
        z.add("a/Brush.archive", b"hello").unwrap();
        z.add("b.png", &[7u8; 300]).unwrap();
        let bytes = z.finish().unwrap();
        let r = ZipReader::new(&bytes).unwrap();
        assert_eq!(r.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a/Brush.archive", "b.png"]);
        assert_eq!(r.read_by_name("a/Brush.archive", 10).unwrap(), b"hello");
        assert!(r.read_by_name("b.png", 299).is_err());
        assert!(r.read_by_name("missing", 10).is_err());
    }

    #[test]
    fn damaged_archives_are_refused() {
        for bytes in [&b""[..], b"PK\x03\x04 nope", b"just text"] { assert!(ZipReader::new(bytes).is_err()); }
        let mut z = ZipWriter::new();
        z.add("x", b"payload payload payload").unwrap();
        let mut bytes = z.finish().unwrap();
        // The payload changed: its CRC no longer matches.
        let at = bytes.windows(7).position(|w| w == b"payload").unwrap();
        bytes[at] ^= 1;
        let r = ZipReader::new(&bytes).unwrap();
        assert!(r.read_by_name("x", 100).is_err());
    }
}

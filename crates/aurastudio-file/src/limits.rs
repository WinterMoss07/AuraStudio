//! The bounds every read keeps: a damaged size can't make it allocate more.

use std::io::{Read, Seek};

/// File extension of AuraStudio documents.
pub const EXTENSION: &str = "aurs";
pub(crate) const MIMETYPE: &str = "application/x-aurastudio";
/// Largest document.json accepted (the tree and the step list are small).
pub(crate) const MAX_JSON: u64 = 64 << 20;
/// Largest preview, ICC profile, history step.
pub(crate) const MAX_PREVIEW: u64 = 16 << 20;
pub(crate) const MAX_ICC: u64 = 64 << 20;
pub(crate) const MAX_STEP: u64 = 1 << 30;
pub(crate) const PREVIEW_MAX: u32 = 512;

/// Bytes of one archive entry, at most `limit`.
pub(crate) fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, limit: u64) -> Result<Vec<u8>, String> {
    let entry = zip.by_name(name).map_err(|_| format!("damaged file ({name} missing)"))?;
    if entry.size() > limit { return Err(format!("damaged file ({name} too big)")); }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry.take(limit).read_to_end(&mut out).map_err(|_| format!("damaged file ({name} unreadable)"))?;
    Ok(out)
}

/// LZ4 data with its size first, expected to unpack to exactly `len`
/// bytes (0.1's whole-layer entries).
pub(crate) fn unpack(blob: &[u8], len: usize) -> Option<Vec<u8>> {
    let size = u32::from_le_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    if size != len { return None; }
    let mut out = vec![0u8; len];
    let n = lz4_flex::block::decompress_into(blob.get(4..)?, &mut out).ok()?;
    (n == len).then_some(out)
}

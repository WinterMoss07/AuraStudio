//! Plane codecs brush files use: raw and PackBits RLE (PSD counts, 2
//! bytes per row). Decoded data is planar bytes in big-endian sample order:
//! `planes × height × row_bytes` (depth 1 packs 8 pixels per byte).

use crate::import::psd::error::{PsdError, Result};
use crate::import::psd::header::row_bytes;
use crate::import::psd::io::Reader;

/// Most bytes one decode may produce: a backstop against decompression
/// bombs (the decoders bound their output by their input on their own:
/// RLE expands at most 64×, raw needs every byte present).
pub const MAX_DECODED_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// 0: uncompressed.
    Raw,
    /// 1: PackBits RLE with a per-row byte-count table.
    Rle,
    /// Anything else (ZIP is never used in brush files): refused.
    Unknown(u16),
}

/// PackBits (Apple / TIFF) run-length coding as used by Photoshop RLE.
pub mod packbits {
    use crate::import::psd::error::{PsdError, Result};

    /// Encodes one row, appending to `out`. Never emits the `-128` no-op.
    #[cfg(test)]
    pub fn encode(src: &[u8], out: &mut Vec<u8>) {
        let n = src.len();
        let mut i = 0;
        while i < n {
            // Length of the run starting at i.
            let mut run = 1;
            while i + run < n && run < 128 && src[i + run] == src[i] {
                run += 1;
            }
            // A literal never stops before a 2-byte run, so any run of >= 2
            // seen here starts a packet boundary: a run packet is never larger.
            if run >= 2 {
                out.push((1i16 - run as i16) as i8 as u8);
                out.push(src[i]);
                i += run;
                continue;
            }
            // Literal: extend until a run of >= 3 begins or 128 bytes.
            let start = i;
            let mut len = 0;
            while i < n && len < 128 {
                if i + 2 < n && src[i] == src[i + 1] && src[i] == src[i + 2] {
                    break;
                }
                i += 1;
                len += 1;
            }
            out.push((len - 1) as u8);
            out.extend_from_slice(&src[start..start + len]);
        }
    }

    /// Decodes `src` into exactly `expected` bytes appended to `out`.
    ///
    /// Errors if the input is exhausted early or a packet would overflow the
    /// row. Trailing input after the row is complete is ignored except for
    /// `-128` no-op bytes (which are always permitted). Errors, rather than
    /// overflowing, when `out.len() + expected` does not fit in `usize`.
    pub(crate) fn decode_into(src: &[u8], expected: usize, out: &mut Vec<u8>) -> Result<()> {
        let target = out.len().checked_add(expected).ok_or(PsdError::LimitExceeded("PackBits output length overflows usize"))?;
        let mut i = 0;
        while out.len() < target {
            let Some(&h) = src.get(i) else {
                return Err(PsdError::Decompress("PackBits row ended early".into()));
            };
            i += 1;
            let h = h as i8;
            if h >= 0 {
                let len = h as usize + 1;
                let Some(lit) = src.get(i..i + len) else {
                    return Err(PsdError::Decompress("PackBits literal truncated".into()));
                };
                if len > target - out.len() {
                    return Err(PsdError::Decompress("PackBits literal overflows row".into()));
                }
                out.extend_from_slice(lit);
                i += len;
            } else if h != -128 {
                let len = 1 - h as isize;
                let len = len as usize;
                let Some(&b) = src.get(i) else {
                    return Err(PsdError::Decompress("PackBits run truncated".into()));
                };
                i += 1;
                if len > target - out.len() {
                    return Err(PsdError::Decompress("PackBits run overflows row".into()));
                }
                out.extend(std::iter::repeat_n(b, len));
            }
        }
        Ok(())
    }

    /// Decodes into a new vector of exactly `expected` bytes.
    #[cfg(test)]
    pub fn decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
        let mut v = Vec::with_capacity(expected.min(src.len().saturating_mul(64)));
        decode_into(src, expected, &mut v)?;
        Ok(v)
    }
}

/// Geometry of a set of planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLayout {
    /// Number of planes.
    pub planes: usize,
    /// Pixels per row.
    pub width:  usize,
    /// Rows per plane.
    pub height: usize,
    /// Bits per sample (1, 8, 16, 32).
    pub depth:  u16,
}

impl PlaneLayout {
    /// Bytes per row.
    pub fn row_bytes(&self) -> usize { row_bytes(self.width, self.depth) }

    /// Total rows over all planes.
    pub fn rows(&self) -> Result<usize> {
        self.planes.checked_mul(self.height).ok_or(PsdError::LimitExceeded("row count overflow"))
    }

    /// Total decoded bytes without the [`MAX_DECODED_BYTES`] cap.
    pub fn total_bytes(&self) -> Result<u64> {
        (self.planes as u64)
            .checked_mul(self.height as u64)
            .and_then(|v| v.checked_mul(self.row_bytes() as u64))
            .ok_or(PsdError::LimitExceeded("decoded size overflow"))
    }

    /// Total decoded bytes, checked against [`MAX_DECODED_BYTES`].
    pub fn decoded_len(&self) -> Result<usize> {
        let total = self.total_bytes()?;
        if total > MAX_DECODED_BYTES {
            return Err(PsdError::LimitExceeded("decoded data exceeds MAX_DECODED_BYTES"));
        }
        usize::try_from(total).map_err(|_| PsdError::LimitExceeded("decoded size exceeds address space"))
    }
}

/// Decodes planar data (without the compression field).
pub fn decode_planes(compression: Compression, data: &[u8], layout: &PlaneLayout) -> Result<Vec<u8>> {
    let total = layout.decoded_len()?;
    match compression {
        Compression::Raw => {
            if data.len() < total {
                return Err(PsdError::UnexpectedEof { offset: data.len(), needed: total - data.len() });
            }
            let mut out = Vec::new();
            out.try_reserve_exact(total).map_err(|_| PsdError::OutOfMemory("not enough memory for the decoded data"))?;
            out.extend_from_slice(&data[..total]);
            Ok(out)
        }
        Compression::Rle => decode_rle(data, layout, total),
        Compression::Unknown(c) => Err(PsdError::Unsupported(format!("compression method {c}"))),
    }
}

fn decode_rle(data: &[u8], layout: &PlaneLayout, total: usize) -> Result<Vec<u8>> {
    let rows = layout.rows()?;
    let rb = layout.row_bytes();
    let mut r = Reader::new(data);
    r.check_count(rows as u64, 2)?;
    let mut counts = Vec::with_capacity(rows);
    for _ in 0..rows { counts.push(r.u16()? as usize); }
    // PackBits expands at most 64x (2 input bytes -> 128 output bytes).
    if (total as u64) > (r.remaining() as u64).saturating_mul(64) {
        return Err(PsdError::Decompress("RLE data too short for declared size".into()));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| PsdError::OutOfMemory("not enough memory for the decoded data"))?;
    for c in counts {
        let row = r.bytes(c)?;
        packbits::decode_into(row, rb, &mut out)?;
    }
    Ok(out)
}

/// Encodes planar data (test files).
#[cfg(test)]
pub fn encode_planes(compression: Compression, decoded: &[u8], layout: &PlaneLayout) -> Result<Vec<u8>> {
    if decoded.len() != layout.decoded_len()? { return Err(PsdError::Invalid("decoded buffer size".into())); }
    match compression {
        Compression::Raw => Ok(decoded.to_vec()),
        Compression::Rle => {
            let (rows, rb) = (layout.rows()?, layout.row_bytes());
            let mut out = vec![0u8; rows * 2];
            let mut enc = Vec::new();
            for row in 0..rows {
                let start = enc.len();
                packbits::encode(&decoded[row * rb..(row + 1) * rb], &mut enc);
                let n = u16::try_from(enc.len() - start).map_err(|_| PsdError::LimitExceeded("RLE row too long"))?;
                out[row * 2..row * 2 + 2].copy_from_slice(&n.to_be_bytes());
            }
            out.extend_from_slice(&enc);
            Ok(out)
        }
        Compression::Unknown(c) => Err(PsdError::Unsupported(format!("compression method {c}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_round_trips() {
        for (rows, rb) in [(0usize, 4usize), (1, 4), (7, 300), (33, 3000)] {
            let data: Vec<u8> = (0..rows * rb).map(|i| if i % 97 < 40 { 7 } else { (i * 31 % 251) as u8 }).collect();
            let layout = PlaneLayout { planes: 1, width: rb, height: rows, depth: 8 };
            let enc = encode_planes(Compression::Rle, &data, &layout).unwrap();
            assert_eq!(decode_planes(Compression::Rle, &enc, &layout).unwrap(), data);
        }
    }

    #[test]
    fn packbits_rejects_truncated_rows() {
        assert!(packbits::decode(&[5, 1, 2], 6).is_err());
        assert_eq!(packbits::decode(&[0xFE, 9], 3).unwrap(), vec![9, 9, 9]);
    }
}

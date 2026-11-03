//! The size of a plane's row.

/// Bytes per row for `width` samples at `depth` bits.
pub fn row_bytes(width: usize, depth: u16) -> usize {
    match depth {
        1 => width.div_ceil(8),
        d => width.saturating_mul(usize::from(d / 8).max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_bytes_values() {
        assert_eq!(row_bytes(9, 1), 2);
        assert_eq!(row_bytes(8, 1), 1);
        assert_eq!(row_bytes(0, 1), 0);
        assert_eq!(row_bytes(3, 8), 3);
        assert_eq!(row_bytes(3, 16), 6);
        assert_eq!(row_bytes(3, 32), 12);
    }
}

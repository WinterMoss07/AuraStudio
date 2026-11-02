//! Mouse events Windows makes out of pen and touch input carry a
//! signature in their extra info: `0xFF515700` in the high 24 bits
//! (`MI_WP_SIGNATURE`), bit 7 set for touch.

const MI_WP_SIGNATURE: usize = 0xFF51_5700;
const SIGNATURE_MASK: usize = 0xFFFF_FF00;

/// The extra info of a mouse message says pen or touch made it.
pub fn is_pen_or_touch(extra: usize) -> bool { extra & SIGNATURE_MASK == MI_WP_SIGNATURE }

#[cfg(test)]
mod tests {
    #[test]
    fn signature() {
        assert!(super::is_pen_or_touch(0xFF51_5700));
        assert!(super::is_pen_or_touch(0xFF51_5780));
        assert!(!super::is_pen_or_touch(0));
        assert!(!super::is_pen_or_touch(0x1234_5678));
    }
}

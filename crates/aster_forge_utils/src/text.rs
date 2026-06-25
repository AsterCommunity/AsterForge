//! Text length and UTF-8-safe truncation helpers.
//!
//! Aster services often need conservative limits for display text, file names, status messages, or
//! external-system error snippets. This module keeps byte-based truncation UTF-8-safe and provides a
//! small character-count helper for product validation rules that are expressed in Unicode scalar
//! values instead of bytes.

/// Returns the number of Unicode scalar values in a string.
///
/// This is intentionally not a grapheme-cluster count. Product validation rules that need
/// user-perceived characters should use a dedicated Unicode segmentation policy at the product
/// boundary.
pub fn char_count(value: &str) -> usize {
    value.chars().count()
}

/// Truncates a string to at most `max_bytes` bytes without splitting a UTF-8 code point.
///
/// If `value` is already within the limit it is returned unchanged as an owned string. A zero limit
/// always returns an empty string.
pub fn truncate_utf8_to_max_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::{char_count, truncate_utf8_to_max_bytes};

    #[test]
    fn char_count_counts_unicode_scalars() {
        assert_eq!(char_count("Aster"), 5);
        assert_eq!(char_count("你好世界"), 4);
        assert_eq!(char_count("e\u{301}"), 2);
    }

    #[test]
    fn truncate_utf8_to_max_bytes_keeps_short_ascii_unchanged() {
        assert_eq!(
            truncate_utf8_to_max_bytes("AsterYggdrasil", 32),
            "AsterYggdrasil"
        );
    }

    #[test]
    fn truncate_utf8_to_max_bytes_truncates_ascii_by_bytes() {
        assert_eq!(truncate_utf8_to_max_bytes("AsterYggdrasil", 5), "Aster");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_preserves_char_boundaries() {
        assert_eq!(truncate_utf8_to_max_bytes("你好世界", 7), "你好");
        assert_eq!(truncate_utf8_to_max_bytes("éclair", 1), "");
        assert_eq!(truncate_utf8_to_max_bytes("éclair", 2), "é");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_handles_zero_limit() {
        assert_eq!(truncate_utf8_to_max_bytes("AsterYggdrasil", 0), "");
        assert_eq!(truncate_utf8_to_max_bytes("你好", 0), "");
    }
}

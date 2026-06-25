//! Public display text and asset URL validation helpers.
//!
//! Aster services often expose runtime branding or UI-shell configuration such as
//! titles, descriptions, favicons, wordmarks, and provider icons. Product crates
//! still own the concrete keys, defaults, and visibility rules. This module only
//! centralizes the repeated mechanics for trimming bounded display text,
//! rejecting control characters, and validating asset URL strings that are safe
//! to place into generated frontend HTML.

use crate::{Result, ValidationError};

/// Normalizes a short display text value.
///
/// The value is trimmed, byte-length limited, and rejected when it contains
/// control characters. Empty values are allowed so product configuration can use
/// an empty string as a "reset to default" signal.
pub fn normalize_bounded_display_text(
    field_name: &str,
    value: &str,
    max_len: usize,
) -> Result<String> {
    let normalized = value.trim();
    if normalized.len() > max_len {
        return Err(ValidationError::new(format!(
            "{field_name} exceeds {max_len} characters"
        )));
    }
    if strip_control_chars(normalized) != normalized {
        return Err(ValidationError::new(format!(
            "{field_name} cannot contain control characters"
        )));
    }
    Ok(normalized.to_string())
}

/// Removes Unicode control characters from a display string.
pub fn strip_control_chars(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_control()).collect()
}

/// Returns a normalized display string or a product default.
///
/// This helper is intended for runtime reads where invalid persisted values
/// should not break public pages. It first strips control characters, then
/// applies the same length and trimming rules as [`normalize_bounded_display_text`].
pub fn display_text_or_default(
    value: Option<String>,
    default: &str,
    field_name: &str,
    max_len: usize,
) -> String {
    value
        .map(|value| strip_control_chars(&value))
        .and_then(|value| normalize_bounded_display_text(field_name, &value, max_len).ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Normalizes a public frontend asset URL.
///
/// Empty values are allowed for optional branding assets. Non-empty values are
/// trimmed, byte-length limited, rejected when they contain whitespace, and
/// accepted when they are either a leading-slash path or an absolute `http(s)`
/// URL. The predicate intentionally mirrors the historical Aster branding
/// behavior so existing product configuration keeps the same storage semantics.
pub fn normalize_public_asset_url(field_name: &str, value: &str, max_len: usize) -> Result<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(String::new());
    }
    if normalized.len() > max_len {
        return Err(ValidationError::new(format!(
            "{field_name} exceeds {max_len} characters"
        )));
    }
    if normalized.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(format!(
            "{field_name} cannot contain whitespace"
        )));
    }
    if !is_public_asset_url(normalized) {
        return Err(ValidationError::new(format!(
            "{field_name} must be an absolute http(s) URL or a root-relative path"
        )));
    }
    Ok(normalized.to_string())
}

/// Returns whether a value is accepted by [`normalize_public_asset_url`].
pub fn is_public_asset_url(value: &str) -> bool {
    value.starts_with('/') || value.starts_with("https://") || value.starts_with("http://")
}

/// Returns a public asset URL or a product default.
pub fn public_asset_url_or_default(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| is_public_asset_url(value))
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        display_text_or_default, is_public_asset_url, normalize_bounded_display_text,
        normalize_public_asset_url, public_asset_url_or_default, strip_control_chars,
    };

    #[test]
    fn display_text_trims_allows_empty_and_rejects_control_characters() {
        assert_eq!(
            normalize_bounded_display_text("title", "  Aster  ", 20).unwrap(),
            "Aster"
        );
        assert_eq!(
            normalize_bounded_display_text("title", "  ", 20).unwrap(),
            ""
        );
        assert!(normalize_bounded_display_text("title", "abc", 2).is_err());
        assert!(normalize_bounded_display_text("title", "hello\nworld", 20).is_err());
    }

    #[test]
    fn display_text_default_reader_strips_control_characters_before_fallback() {
        assert_eq!(strip_control_chars("A\u{0000}ster"), "Aster");
        assert_eq!(
            display_text_or_default(
                Some("  Site\u{0000} Name  ".to_string()),
                "Default",
                "title",
                20
            ),
            "Site Name"
        );
        assert_eq!(
            display_text_or_default(Some("  ".to_string()), "Default", "title", 20),
            "Default"
        );
    }

    #[test]
    fn public_asset_url_trims_allows_empty_and_rejects_invalid_values() {
        assert_eq!(
            normalize_public_asset_url("favicon", "  /assets/icon.svg?v=1  ", 2048).unwrap(),
            "/assets/icon.svg?v=1"
        );
        assert_eq!(
            normalize_public_asset_url("favicon", "  ", 2048).unwrap(),
            ""
        );
        assert!(
            normalize_public_asset_url("favicon", "https://cdn.example.com/icon 1.svg", 2048)
                .is_err()
        );
        assert!(normalize_public_asset_url("favicon", "javascript:alert(1)", 2048).is_err());
        assert!(normalize_public_asset_url("favicon", "icons/favicon.svg", 2048).is_err());
    }

    #[test]
    fn public_asset_default_reader_accepts_same_url_predicate() {
        assert!(is_public_asset_url("/favicon.svg"));
        assert!(is_public_asset_url("https://cdn.example.com/favicon.svg"));
        assert!(is_public_asset_url("http://cdn.example.com/favicon.svg"));
        assert!(!is_public_asset_url("favicon.svg"));
        assert_eq!(
            public_asset_url_or_default(Some("/custom.svg".to_string()), "/favicon.svg"),
            "/custom.svg"
        );
        assert_eq!(
            public_asset_url_or_default(Some("bad url".to_string()), "/favicon.svg"),
            "/favicon.svg"
        );
    }
}

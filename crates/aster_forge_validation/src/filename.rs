//! File and folder name validation helpers.
//!
//! This module validates names against cross-platform filesystem constraints, normalizes Unicode to
//! NFC, and generates collision-copy names that respect byte limits. It also includes blob-key
//! sharding helpers used when mapping logical file identifiers to storage paths.

use crate::{Result, ValidationError};
use unicode_normalization::UnicodeNormalization;

/// Maximum filename length in UTF-8 bytes.
///
/// This is intentionally byte-based rather than scalar-count-based. It is more
/// conservative than NTFS/APFS "255 characters" and remains compatible with the
/// common ext4 255-byte component limit.
pub const MAX_FILENAME_LEN: usize = 255;
const COPY_FALLBACK_STEM: &str = "copy";

const FORBIDDEN_CHARS: &[char] = &['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'];

const WINDOWS_RESERVED_BASENAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Parsed pieces of a filename used when generating copy names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyNameTemplate {
    /// Base filename without the extension and generated copy suffix.
    pub base_name: String,
    /// Extension including the leading dot, when the name has one.
    pub ext: Option<String>,
    /// Copy number inferred from the input name.
    pub next_copy_number: u32,
}

/// Normalizes a name to Unicode NFC.
pub fn normalize_name(name: &str) -> String {
    name.nfc().collect()
}

/// Counts Unicode scalar values in a string.
pub fn char_count(value: &str) -> usize {
    value.chars().count()
}

/// Normalizes a file or folder name and validates the normalized result.
pub fn normalize_validate_name(name: &str) -> Result<String> {
    let normalized = normalize_name(name);
    validate_normalized_name(&normalized)?;
    Ok(normalized)
}

/// Validates a file or folder name after Unicode normalization.
pub fn validate_name(name: &str) -> Result<()> {
    let normalized = normalize_name(name);
    validate_normalized_name(&normalized)
}

fn validate_normalized_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ValidationError::new("name cannot be empty"));
    }
    if name.len() > MAX_FILENAME_LEN {
        return Err(ValidationError::new(format!(
            "name too long (max {MAX_FILENAME_LEN} bytes)"
        )));
    }
    if name == "." || name == ".." {
        return Err(ValidationError::new("invalid name"));
    }
    if is_windows_reserved_name(name) {
        return Err(ValidationError::new(
            "name cannot use a Windows reserved device name",
        ));
    }
    if let Some(c) = name.chars().find(|c| FORBIDDEN_CHARS.contains(c)) {
        return Err(ValidationError::new(format!(
            "name contains forbidden character '{c}'"
        )));
    }
    if name.chars().any(|c| c.is_ascii_control()) {
        return Err(ValidationError::new("name contains control characters"));
    }
    if name != name.trim() || name.ends_with('.') {
        return Err(ValidationError::new(
            "name cannot start/end with spaces or end with a dot",
        ));
    }
    Ok(())
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    WINDOWS_RESERVED_BASENAMES.contains(&upper.as_str())
}

/// Builds a two-level sharded storage path from a blob key.
///
/// The key must be long enough to supply the two shard segments and must be an ASCII storage token,
/// not a path supplied by a caller. This keeps the helper from panicking on short or non-UTF-8
/// boundary inputs and prevents accidental nested paths from bypassing the intended sharding
/// layout.
pub fn storage_path_from_blob_key(blob_key: &str) -> Result<String> {
    validate_blob_key_for_storage_path(blob_key)?;

    Ok(format!(
        "{}/{}/{}",
        &blob_key[..2],
        &blob_key[2..4],
        blob_key
    ))
}

fn validate_blob_key_for_storage_path(blob_key: &str) -> Result<()> {
    if blob_key.len() < 4 {
        return Err(ValidationError::new(
            "blob key must contain at least 4 ASCII characters",
        ));
    }
    if !blob_key.is_ascii()
        || blob_key
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'/' | b'\\'))
    {
        return Err(ValidationError::new(
            "blob key must be an ASCII token without path separators",
        ));
    }
    Ok(())
}

/// Parses a name into the template used to generate copy names.
pub fn copy_name_template(name: &str) -> CopyNameTemplate {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], Some(name[dot..].to_string())),
        _ => (name, None),
    };

    let (base_name, next_copy_number) = if let Some(paren_start) = stem.rfind(" (") {
        let after_paren = &stem[paren_start + 2..];
        if let Some(num_str) = after_paren.strip_suffix(')') {
            if let Ok(n) = num_str.parse::<u32>() {
                match n.checked_add(1) {
                    Some(next) => (stem[..paren_start].to_string(), next),
                    // The copy-number space is exhausted. Keep the full stem and start a
                    // fresh " (1)" layer on top of it, exactly like the unparseable-suffix
                    // branches: falling back to "file (1)" would produce a name that very
                    // likely already exists and could be overwritten by one-shot callers.
                    None => (stem.to_string(), 1),
                }
            } else {
                (stem.to_string(), 1)
            }
        } else {
            (stem.to_string(), 1)
        }
    } else {
        (stem.to_string(), 1)
    };

    CopyNameTemplate {
        base_name,
        ext,
        next_copy_number,
    }
}

/// Formats a copy name using the default filename length limit.
pub fn format_copy_name(template: &CopyNameTemplate, copy_number: u32) -> String {
    format_copy_name_with_limit(template, copy_number, MAX_FILENAME_LEN)
}

/// Formats a copy name while keeping the result within `max_len` UTF-8 bytes.
pub fn format_copy_name_with_limit(
    template: &CopyNameTemplate,
    copy_number: u32,
    max_len: usize,
) -> String {
    let suffix = format!(" ({copy_number})");
    let ext = template.ext.as_deref().unwrap_or("");
    let ext = bounded_copy_extension(ext, suffix.len(), max_len);
    let max_base_len = max_len.saturating_sub(suffix.len() + ext.len());
    let mut base = truncate_utf8_to_max_bytes(&template.base_name, max_base_len);
    if base.is_empty() {
        base = truncate_utf8_to_max_bytes(COPY_FALLBACK_STEM, max_base_len);
    }

    format!("{base}{suffix}{ext}")
}

/// Truncates a string to at most `max_len` bytes without splitting a UTF-8 code point.
pub fn truncate_utf8_to_max_bytes(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }

    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn bounded_copy_extension(ext: &str, suffix_len: usize, max_len: usize) -> String {
    if ext.is_empty() {
        return String::new();
    }

    let max_ext_len = max_len
        .saturating_sub(COPY_FALLBACK_STEM.len())
        .saturating_sub(suffix_len);
    if max_ext_len < 2 {
        return String::new();
    }

    let mut candidate = truncate_utf8_to_max_bytes(ext, max_ext_len);
    while candidate.ends_with('.') || candidate.ends_with(' ') {
        candidate.pop();
    }
    if candidate.len() < 2 || !candidate.starts_with('.') {
        String::new()
    } else {
        candidate
    }
}

/// Returns the next copy name for a file or folder.
pub fn next_copy_name(name: &str) -> String {
    let template = copy_name_template(name);
    format_copy_name(&template, template.next_copy_number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_and_rejects_expected_values() {
        assert!(validate_name("hello.txt").is_ok());
        assert!(validate_name(".gitignore").is_ok());
        assert!(validate_name("file (1).txt").is_ok());
        assert!(validate_name("cafe\u{0301}.txt").is_ok());

        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name("a*b").is_err());
        assert!(validate_name("a?b").is_err());
        assert!(validate_name("a\"b").is_err());
        assert!(validate_name("a<b").is_err());
        assert!(validate_name("a>b").is_err());
        assert!(validate_name("a|b").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a\x01b").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name("a\tb").is_err());
        assert!(validate_name(" leading").is_err());
        assert!(validate_name("trailing ").is_err());
        assert!(validate_name("ends.").is_err());

        assert!(validate_name(&"a".repeat(256)).is_err());
        assert!(validate_name(&"a".repeat(255)).is_ok());
    }

    #[test]
    fn normalize_validate_name_normalizes_nfd_to_nfc() {
        let normalized = normalize_validate_name("cafe\u{0301}.txt").unwrap();
        assert_eq!(normalized, "caf\u{00e9}.txt");
    }

    #[test]
    fn validate_name_rejects_windows_reserved_names() {
        for name in [
            "CON", "con", "PRN.txt", "aux", "NUL.log", "COM1", "com9.txt", "LPT1", "lpt9.prn",
        ] {
            assert!(validate_name(name).is_err(), "{name} should be rejected");
        }

        assert!(validate_name("console.txt").is_ok());
        assert!(validate_name("LPT10.txt").is_ok());
    }

    #[test]
    fn next_copy_name_matches_platform_copy_pattern() {
        assert_eq!(next_copy_name("test.txt"), "test (1).txt");
        assert_eq!(next_copy_name("test (1).txt"), "test (2).txt");
        assert_eq!(next_copy_name("test (99).txt"), "test (100).txt");
        assert_eq!(next_copy_name("folder"), "folder (1)");
        assert_eq!(next_copy_name("folder (3)"), "folder (4)");
        assert_eq!(next_copy_name("my.file.tar.gz"), "my.file.tar (1).gz");
        assert_eq!(next_copy_name("photo (1).jpg"), "photo (2).jpg");
        assert_eq!(next_copy_name(".hidden"), ".hidden (1)");
    }

    #[test]
    fn next_copy_name_keeps_result_within_filename_limit() {
        let candidate = next_copy_name(&"a".repeat(MAX_FILENAME_LEN));
        assert!(candidate.ends_with(" (1)"));
        assert!(candidate.len() <= MAX_FILENAME_LEN);
        assert!(validate_name(&candidate).is_ok());

        let candidate = next_copy_name(&format!("{}.txt", "a".repeat(MAX_FILENAME_LEN - 4)));
        assert!(candidate.ends_with(" (1).txt"));
        assert!(candidate.len() <= MAX_FILENAME_LEN);
        assert!(validate_name(&candidate).is_ok());
    }

    #[test]
    fn next_copy_name_truncates_on_utf8_boundary() {
        let candidate = next_copy_name(&format!("{}.txt", "猫".repeat(90)));
        assert!(candidate.ends_with(" (1).txt"));
        assert!(candidate.len() <= MAX_FILENAME_LEN);
        assert!(candidate.is_char_boundary(candidate.len()));
        assert!(validate_name(&candidate).is_ok());
    }

    #[test]
    fn format_copy_name_handles_tiny_limits_and_bad_extensions() {
        let template = CopyNameTemplate {
            base_name: "abcdef".to_string(),
            ext: Some(".txt".to_string()),
            next_copy_number: 1,
        };

        assert_eq!(format_copy_name_with_limit(&template, 1, 4), " (1)");
        assert_eq!(format_copy_name_with_limit(&template, 1, 8), "abcd (1)");

        let template = CopyNameTemplate {
            base_name: String::new(),
            ext: Some(". ".to_string()),
            next_copy_number: 1,
        };
        assert_eq!(format_copy_name_with_limit(&template, 1, 12), "copy (1)");
    }

    #[test]
    fn truncate_utf8_to_max_bytes_handles_zero_and_multibyte_boundaries() {
        assert_eq!(truncate_utf8_to_max_bytes("abc", 0), "");
        assert_eq!(truncate_utf8_to_max_bytes("猫猫", 1), "");
        assert_eq!(truncate_utf8_to_max_bytes("猫猫", 3), "猫");
        assert_eq!(truncate_utf8_to_max_bytes("猫猫", 4), "猫");
    }

    #[test]
    fn copy_name_template_parses_existing_suffix() {
        let template = copy_name_template("photo (41).jpg");
        assert_eq!(template.base_name, "photo");
        assert_eq!(template.ext.as_deref(), Some(".jpg"));
        assert_eq!(template.next_copy_number, 42);
        assert_eq!(
            format_copy_name(&template, template.next_copy_number),
            "photo (42).jpg"
        );
    }

    #[test]
    fn copy_name_template_starts_fresh_layer_when_copy_number_space_is_exhausted() {
        // u32::MAX + 1 must not panic (debug) or wrap to 0 (release). It also must not
        // fall back to "file (1)", which almost certainly exists; the whole stem is kept
        // and a fresh copy layer starts on top of it.
        let template = copy_name_template("file (4294967295).txt");
        assert_eq!(template.base_name, "file (4294967295)");
        assert_eq!(template.next_copy_number, 1);
        assert_eq!(
            next_copy_name("file (4294967295).txt"),
            "file (4294967295) (1).txt"
        );

        // The boundary just below still increments normally.
        let template = copy_name_template("file (4294967294).txt");
        assert_eq!(template.base_name, "file");
        assert_eq!(template.next_copy_number, u32::MAX);
        assert_eq!(
            next_copy_name("file (4294967294).txt"),
            "file (4294967295).txt"
        );
    }

    #[test]
    fn storage_path_from_blob_key_uses_two_level_sharding() {
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        assert_eq!(
            storage_path_from_blob_key(hash).unwrap(),
            format!("ab/cd/{hash}")
        );
    }

    #[test]
    fn storage_path_from_blob_key_rejects_values_that_cannot_be_safely_sharded() {
        assert!(storage_path_from_blob_key("abc").is_err());
        assert!(storage_path_from_blob_key("ab/cd").is_err());
        assert!(storage_path_from_blob_key("ab\\cd").is_err());
        assert!(storage_path_from_blob_key("猫猫猫猫").is_err());
    }
}

//! Object-storage key normalization helpers.
//!
//! These helpers keep object keys relative, slash-separated, and safe to join with storage
//! prefixes. They reject path escape attempts while preserving existing prefix placement rules that
//! may matter for S3 bucket policies or migrated objects.

use crate::{Result, StorageCoreError};

const INVALID_RELATIVE_KEY_MESSAGE: &str = "object key must be a safe relative storage path";

/// Normalize an external object key into a slash-separated relative key.
///
/// Empty/root-like input is represented as `"."`, so callers can distinguish the scoped root from
/// a real object named with an empty string. Backslashes are treated as separators to prevent
/// Windows-style escape attempts from bypassing `..` checks.
pub fn normalize_relative_key(value: &str) -> Result<String> {
    let value = value.trim_start_matches('/').replace('\\', "/");
    if value.is_empty() {
        return Ok(".".to_string());
    }

    let mut segments = Vec::new();
    for segment in value.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                return Err(StorageCoreError::InvalidObjectKey(
                    INVALID_RELATIVE_KEY_MESSAGE.to_string(),
                ));
            }
            segment => segments.push(segment),
        }
    }

    if segments.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(segments.join("/"))
    }
}

/// Normalizes an object key and rejects the storage namespace root.
///
/// Use this for concrete object operations such as get, put, delete, exists,
/// and metadata. It accepts leading slashes and Windows separators but rejects
/// empty/root-like values and parent-directory escape attempts.
pub fn normalize_object_key(value: &str) -> Result<String> {
    let key = normalize_relative_key(value.trim())?;
    if key == "." {
        return Err(StorageCoreError::InvalidObjectKey(
            "object key cannot target the storage namespace root".to_string(),
        ));
    }
    Ok(key)
}

/// Normalizes a storage prefix.
///
/// Empty and root-like inputs map to an empty prefix. Concrete object keys
/// should use [`normalize_object_key`] instead.
pub fn normalize_object_prefix(value: &str) -> Result<String> {
    let prefix = normalize_relative_key(value.trim())?;
    if prefix == "." {
        Ok(String::new())
    } else {
        Ok(prefix)
    }
}

/// Join a storage prefix and object key without producing duplicate separators.
///
/// This deliberately only trims trailing slashes from the prefix. Existing S3 policies may have
/// been configured with a leading slash, and preserving that keeps object placement stable.
pub fn join_key_prefix(prefix: &str, key: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let key = key.trim_start_matches('/');

    if prefix.is_empty() {
        key.to_string()
    } else if key.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{key}")
    }
}

/// Strip `prefix` from `key` only when the prefix matches a complete slash-separated segment.
pub fn strip_key_prefix<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return Some(key.trim_start_matches('/'));
    }

    if key == prefix {
        return Some("");
    }

    key.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('/'))
}

#[cfg(test)]
mod tests {
    use super::{
        join_key_prefix, normalize_object_key, normalize_object_prefix, normalize_relative_key,
        strip_key_prefix,
    };

    #[test]
    fn normalize_relative_key_collapses_slashes_and_dot_segments() {
        assert_eq!(
            normalize_relative_key("/folder//./file.txt").unwrap(),
            "folder/file.txt"
        );
        assert_eq!(normalize_relative_key("").unwrap(), ".");
        assert_eq!(normalize_relative_key("/").unwrap(), ".");
    }

    #[test]
    fn normalize_relative_key_rejects_escape_segments() {
        assert!(normalize_relative_key("../secret.txt").is_err());
        assert!(normalize_relative_key("folder/../secret.txt").is_err());
        assert!(normalize_relative_key("folder\\..\\secret.txt").is_err());
    }

    #[test]
    fn normalize_relative_key_handles_windows_separators_and_root_like_values() {
        assert_eq!(
            normalize_relative_key("\\folder\\.\\file.txt").unwrap(),
            "folder/file.txt"
        );
        assert_eq!(normalize_relative_key("////").unwrap(), ".");
        assert_eq!(normalize_relative_key("././").unwrap(), ".");
    }

    #[test]
    fn normalize_object_key_rejects_root_like_values() {
        assert_eq!(
            normalize_object_key("/folder//file.txt").unwrap(),
            "folder/file.txt"
        );
        assert!(normalize_object_key("").is_err());
        assert!(normalize_object_key("/").is_err());
        assert!(normalize_object_key("../secret.txt").is_err());
    }

    #[test]
    fn normalize_object_prefix_allows_root_like_values() {
        assert_eq!(normalize_object_prefix("").unwrap(), "");
        assert_eq!(normalize_object_prefix("/").unwrap(), "");
        assert_eq!(
            normalize_object_prefix("/folder//prefix/").unwrap(),
            "folder/prefix"
        );
        assert!(normalize_object_prefix("folder/../secret").is_err());
    }

    #[test]
    fn join_key_prefix_handles_empty_and_slash_edge_cases() {
        assert_eq!(join_key_prefix("", "/files/a.txt"), "files/a.txt");
        assert_eq!(join_key_prefix("base/", "/files/a.txt"), "base/files/a.txt");
        assert_eq!(join_key_prefix("base", ""), "base");
        assert_eq!(
            join_key_prefix("/base/", "/files/a.txt"),
            "/base/files/a.txt"
        );
    }

    #[test]
    fn strip_key_prefix_matches_only_segment_boundaries() {
        assert_eq!(strip_key_prefix("", "/files/a.txt"), Some("files/a.txt"));
        assert_eq!(
            strip_key_prefix("base", "base/files/a.txt"),
            Some("files/a.txt")
        );
        assert_eq!(strip_key_prefix("base/", "base"), Some(""));
        assert_eq!(strip_key_prefix("base", "baseball/files/a.txt"), None);
        assert_eq!(
            strip_key_prefix("/base/", "/base/files/a.txt"),
            Some("files/a.txt")
        );
    }

    #[test]
    fn strip_key_prefix_rejects_partial_and_directional_mismatches() {
        assert_eq!(strip_key_prefix("base/files", "base/file"), None);
        assert_eq!(
            strip_key_prefix("base/files", "base/files-extra/a.txt"),
            None
        );
        assert_eq!(strip_key_prefix("base/files", "other/files/a.txt"), None);
    }
}

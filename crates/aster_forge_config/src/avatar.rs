//! Gravatar configuration helpers.
//!
//! This module owns the configuration-facing parts of Gravatar handling:
//! normalization of the stored base URL and runtime fallback to the conventional
//! default base URL. Product crates still own avatar source policy, upload
//! routes, cache headers, and which sizes they expose.

use crate::{ConfigCoreError, Result};

use aster_forge_utils::url::{HttpBaseUrlOptions, normalize_http_base_url};

/// Default Gravatar base URL used by Aster services.
pub const DEFAULT_GRAVATAR_BASE_URL: &str = "https://www.gravatar.com/avatar";

/// Normalizes a Gravatar base URL configuration value.
///
/// Empty values fall back to [`DEFAULT_GRAVATAR_BASE_URL`]. Non-empty values
/// must be absolute HTTP(S) base URLs without query or fragment components.
pub fn normalize_gravatar_base_url_config_value(value: &str) -> Result<String> {
    normalize_http_base_url(
        value,
        "gravatar_base_url",
        HttpBaseUrlOptions::optional_without_query_fragment(),
    )
    .map_err(|error| ConfigCoreError::invalid_value(error.to_string()))
    .map(|normalized| normalized.unwrap_or_else(|| DEFAULT_GRAVATAR_BASE_URL.to_string()))
}

/// Returns a normalized Gravatar base URL or [`DEFAULT_GRAVATAR_BASE_URL`].
pub fn gravatar_base_url_or_default(value: Option<&str>) -> String {
    let normalized = value
        .unwrap_or(DEFAULT_GRAVATAR_BASE_URL)
        .trim()
        .trim_end_matches('/')
        .to_string();
    if normalized.is_empty() {
        DEFAULT_GRAVATAR_BASE_URL.to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_GRAVATAR_BASE_URL, gravatar_base_url_or_default,
        normalize_gravatar_base_url_config_value,
    };

    #[test]
    fn gravatar_base_url_normalization_accepts_empty_and_http_base_urls() {
        assert_eq!(
            normalize_gravatar_base_url_config_value("  ").unwrap(),
            DEFAULT_GRAVATAR_BASE_URL
        );
        assert_eq!(
            normalize_gravatar_base_url_config_value(" https://mirror.example/avatar/ ").unwrap(),
            "https://mirror.example/avatar"
        );
        assert!(normalize_gravatar_base_url_config_value("ftp://example.com/avatar").is_err());
        assert!(
            normalize_gravatar_base_url_config_value("https://example.com/avatar?x=1").is_err()
        );
        assert!(
            normalize_gravatar_base_url_config_value("https://example.com/avatar#frag").is_err()
        );
    }

    #[test]
    fn gravatar_base_url_reader_defaults_blank_values() {
        assert_eq!(
            gravatar_base_url_or_default(None),
            DEFAULT_GRAVATAR_BASE_URL
        );
        assert_eq!(
            gravatar_base_url_or_default(Some("   ")),
            DEFAULT_GRAVATAR_BASE_URL
        );
        assert_eq!(
            gravatar_base_url_or_default(Some("https://mirror.example/avatar/")),
            "https://mirror.example/avatar"
        );
    }
}

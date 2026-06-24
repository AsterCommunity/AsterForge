//! URL and origin normalization helpers.
//!
//! This module contains product-neutral URL rules shared by Aster services. It normalizes HTTP
//! origins for CORS and public-site matching, validates HTTP base URLs used by integrations, and
//! exposes small predicates for OAuth-style redirect and endpoint checks. Callers still decide
//! whether failures are configuration errors, validation errors, or domain-specific errors.

use http::Uri;
use url::Url;

use crate::{Result, UtilsError, net::is_loopback_host};

/// Options for [`normalize_http_base_url`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpBaseUrlOptions {
    /// Whether empty input should return `None`.
    pub allow_empty: bool,
    /// Whether query strings and fragments should be rejected.
    pub forbid_query_fragment: bool,
}

impl HttpBaseUrlOptions {
    /// Creates options for a required base URL without query or fragment components.
    pub const fn required_without_query_fragment() -> Self {
        Self {
            allow_empty: false,
            forbid_query_fragment: true,
        }
    }

    /// Creates options for an optional base URL without query or fragment components.
    pub const fn optional_without_query_fragment() -> Self {
        Self {
            allow_empty: true,
            forbid_query_fragment: true,
        }
    }
}

/// Returns whether `url` uses `http` or `https`.
pub fn has_http_scheme(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// Returns whether `url` is HTTPS or an HTTP loopback URL.
///
/// This is useful for development-friendly security checks where plain HTTP is accepted only for
/// localhost and loopback IP addresses.
pub fn is_https_or_loopback_http(url: &Url) -> bool {
    url.scheme() == "https"
        || (url.scheme() == "http" && url.host_str().is_some_and(is_loopback_host))
}

/// Parses an absolute URL and maps parser failures into [`UtilsError`].
pub fn parse_absolute_url(value: &str, label: &str) -> Result<Url> {
    Url::parse(value).map_err(|error| {
        UtilsError::invalid_value(format!("{label} must be an absolute URL: {error}"))
    })
}

/// Normalizes an HTTP base URL.
///
/// Surrounding whitespace and trailing slashes are removed before parsing. The URL must be absolute,
/// use `http` or `https`, and include a host. When `options.forbid_query_fragment` is set, query
/// strings and fragments are rejected so callers can safely append paths.
pub fn normalize_http_base_url(
    value: &str,
    label: &str,
    options: HttpBaseUrlOptions,
) -> Result<Option<String>> {
    let normalized = value.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        if options.allow_empty {
            return Ok(None);
        }
        return Err(UtilsError::invalid_value(format!(
            "{label} cannot be empty"
        )));
    }

    let parsed = Url::parse(&normalized).map_err(|error| {
        UtilsError::invalid_value(format!(
            "{label} must be an absolute http/https URL: {error}"
        ))
    })?;
    if !has_http_scheme(&parsed) || parsed.host_str().is_none() {
        return Err(UtilsError::invalid_value(format!(
            "{label} must use http or https and include a host"
        )));
    }
    if options.forbid_query_fragment && (parsed.query().is_some() || parsed.fragment().is_some()) {
        return Err(UtilsError::invalid_value(format!(
            "{label} cannot include query or fragment"
        )));
    }

    Ok(Some(normalized))
}

/// Normalizes an HTTP origin for CORS and public-site comparisons.
///
/// The returned value is lowercase `scheme://authority`. Paths other than `/`, query strings,
/// fragments, and userinfo are rejected. When `allow_wildcard` is true, `*` is returned unchanged.
pub fn normalize_origin(origin: &str, allow_wildcard: bool) -> Result<String> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err(UtilsError::invalid_value("origin cannot be empty"));
    }

    if allow_wildcard && trimmed == "*" {
        return Ok("*".to_string());
    }

    let uri: Uri = trimmed
        .parse()
        .map_err(|_| UtilsError::invalid_value(format!("invalid origin '{trimmed}'")))?;

    let scheme = uri.scheme_str().ok_or_else(|| {
        UtilsError::invalid_value(format!(
            "origin must include http:// or https://: '{trimmed}'"
        ))
    })?;

    if scheme != "http" && scheme != "https" {
        return Err(UtilsError::invalid_value(format!(
            "origin must use http or https: '{trimmed}'"
        )));
    }

    let authority = uri.authority().ok_or_else(|| {
        UtilsError::invalid_value(format!("origin must include a host: '{trimmed}'"))
    })?;

    if authority.as_str().contains('@') {
        return Err(UtilsError::invalid_value(format!(
            "origin must not include userinfo: '{trimmed}'"
        )));
    }

    if uri.path_and_query().and_then(|pq| pq.query()).is_some() {
        return Err(UtilsError::invalid_value(format!(
            "origin must not include query parameters: '{trimmed}'"
        )));
    }

    let path = uri.path();
    if !path.is_empty() && path != "/" {
        return Err(UtilsError::invalid_value(format!(
            "origin must not include a path: '{trimmed}'"
        )));
    }

    Ok(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.as_str().to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        HttpBaseUrlOptions, has_http_scheme, is_https_or_loopback_http, normalize_http_base_url,
        normalize_origin, parse_absolute_url,
    };

    #[test]
    fn http_base_url_normalization_trims_and_removes_trailing_slashes() {
        assert_eq!(
            normalize_http_base_url(
                " https://example.test/root// ",
                "demo_url",
                HttpBaseUrlOptions::required_without_query_fragment(),
            )
            .unwrap(),
            Some("https://example.test/root".to_string())
        );
    }

    #[test]
    fn http_base_url_normalization_handles_empty_values() {
        assert_eq!(
            normalize_http_base_url(
                "  ",
                "demo_url",
                HttpBaseUrlOptions::optional_without_query_fragment(),
            )
            .unwrap(),
            None
        );
        assert!(
            normalize_http_base_url(
                "  ",
                "demo_url",
                HttpBaseUrlOptions::required_without_query_fragment(),
            )
            .is_err()
        );
    }

    #[test]
    fn http_base_url_normalization_rejects_bad_scheme_and_query_fragment() {
        assert!(
            normalize_http_base_url(
                "ftp://example.test/root",
                "demo_url",
                HttpBaseUrlOptions::required_without_query_fragment(),
            )
            .is_err()
        );
        assert!(
            normalize_http_base_url(
                "https://example.test/root?x=1",
                "demo_url",
                HttpBaseUrlOptions::required_without_query_fragment(),
            )
            .is_err()
        );
        assert!(
            normalize_http_base_url(
                "https://example.test/root#frag",
                "demo_url",
                HttpBaseUrlOptions::required_without_query_fragment(),
            )
            .is_err()
        );
    }

    #[test]
    fn normalize_origin_trims_trailing_slash_and_lowercases() {
        assert_eq!(
            normalize_origin(" HTTPS://Example.COM:8443/ ", false).unwrap(),
            "https://example.com:8443"
        );
    }

    #[test]
    fn normalize_origin_accepts_wildcard_only_when_allowed() {
        assert_eq!(normalize_origin("*", true).unwrap(), "*");
        assert!(normalize_origin("*", false).is_err());
    }

    #[test]
    fn normalize_origin_rejects_invalid_origin_components() {
        assert!(normalize_origin("https://app.example.com/path", false).is_err());
        assert!(normalize_origin("https://app.example.com?x=1", false).is_err());
        assert!(normalize_origin("https://user@app.example.com", false).is_err());
        assert!(normalize_origin("ftp://app.example.com", false).is_err());
        assert!(normalize_origin("https:///missing-host", false).is_err());
    }

    #[test]
    fn url_scheme_predicates_match_http_and_loopback_rules() {
        let https = parse_absolute_url("https://example.com/callback", "callback").unwrap();
        let http_loopback = parse_absolute_url("http://127.0.0.1/callback", "callback").unwrap();
        let http_public = parse_absolute_url("http://example.com/callback", "callback").unwrap();
        let ftp = parse_absolute_url("ftp://example.com/file", "file").unwrap();

        assert!(has_http_scheme(&https));
        assert!(has_http_scheme(&http_loopback));
        assert!(!has_http_scheme(&ftp));
        assert!(is_https_or_loopback_http(&https));
        assert!(is_https_or_loopback_http(&http_loopback));
        assert!(!is_https_or_loopback_http(&http_public));
    }
}

//! Framework-neutral CSRF token and request-source validation.
use http::{
    Method,
    header::{HeaderName, InvalidHeaderName},
};
use rand::RngExt;
use std::sync::OnceLock;
use subtle::ConstantTimeEq;

/// Default CSRF cookie name used by compatibility helpers.
///
/// Prefer [`CsrfTokenNames`] when a product can share a browser origin with another Aster service.
pub const CSRF_COOKIE: &str = "aster_csrf";
/// Default CSRF request header name used by compatibility helpers.
///
/// Prefer [`CsrfTokenNames`] when a product can share a browser origin with another Aster service.
pub const CSRF_HEADER: &str = "X-CSRF-Token";
const DEFAULT_CSRF_HEADER_LOWER: &str = "x-csrf-token";

const MAX_REQUEST_SCHEME_LEN: usize = 16;
const MAX_REQUEST_HOST_LEN: usize = 512;
const MAX_REFERER_AUTHORITY_LEN: usize = MAX_REQUEST_HOST_LEN + 16;
const MAX_SOURCE_HEADER_LEN: usize = 2048;
const MAX_SEC_FETCH_SITE_LEN: usize = 64;

/// Whether source headers are required or only validated when present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSourceMode {
    /// Accept requests without source headers, but validate them when present.
    OptionalWhenPresent,
    /// Require a trusted `Origin` or `Referer` header for unsafe cookie-authenticated actions.
    Required,
}

/// Product-neutral CSRF failure category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfErrorKind {
    /// A configured CSRF cookie or header name was invalid.
    ///
    /// This can only be returned while constructing [`CsrfTokenNames`], not while validating a
    /// normal request.
    TokenNameInvalid,
    /// The CSRF cookie was missing.
    CookieMissing,
    /// The CSRF header was missing.
    HeaderMissing,
    /// The CSRF cookie and header did not match.
    TokenInvalid,
    /// `Sec-Fetch-Site` reported an untrusted source.
    RequestSourceUntrusted,
    /// `Origin` was present but not trusted.
    RequestOriginUntrusted,
    /// `Referer` was present but not trusted.
    RequestRefererUntrusted,
    /// Required source headers were missing.
    RequestSourceMissing,
    /// Request scheme was malformed or too long.
    RequestSchemeInvalid,
    /// Request host was malformed or too long.
    RequestHostInvalid,
    /// Origin header was malformed or too long.
    RequestOriginInvalid,
    /// Referer header was malformed or too long.
    RequestRefererInvalid,
    /// Generic source header validation failure.
    RequestHeaderValueInvalid,
}

/// Error returned by CSRF helper functions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CsrfError {
    kind: CsrfErrorKind,
    message: String,
}

impl CsrfError {
    pub(crate) fn new(kind: CsrfErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the product-neutral failure category.
    #[must_use]
    pub fn kind(&self) -> CsrfErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Result type returned by CSRF helper functions.
pub type Result<T> = std::result::Result<T, CsrfError>;

/// Cookie and header names used by the double-submit token check.
///
/// Services that share a browser origin should configure service-specific names during startup to
/// avoid cookie/header collisions. Store this value in the product's startup state, app data, or a
/// process-wide `OnceLock`; do not switch names while a process is serving traffic because active
/// browser sessions would still hold the previous cookie name and frontend code may still send the
/// previous header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfTokenNames {
    cookie_name: String,
    header_name: HeaderName,
}

impl CsrfTokenNames {
    /// Builds CSRF token names after validating the cookie and header names.
    ///
    /// Cookie names are validated against the conservative RFC 6265 token character set. Header
    /// names are parsed through the framework-neutral HTTP header type and stored in lower-case
    /// form, which makes comparisons and CORS allow-list generation stable.
    ///
    /// # Errors
    ///
    /// Returns [`CsrfError`] when either token name is empty or contains invalid characters.
    pub fn new(cookie_name: impl Into<String>, header_name: impl AsRef<str>) -> Result<Self> {
        let cookie_name = cookie_name.into();
        validate_cookie_name(&cookie_name)?;
        let header_name = parse_header_name(header_name.as_ref())?;
        Ok(Self {
            cookie_name,
            header_name,
        })
    }

    /// Returns the configured CSRF cookie name.
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    /// Returns the configured CSRF request header name.
    pub fn header_name(&self) -> &HeaderName {
        &self.header_name
    }

    /// Returns the configured CSRF request header name as a lower-case string.
    ///
    /// This is useful when building `Access-Control-Allow-Headers` values for browser preflight
    /// responses.
    pub fn header_name_str(&self) -> &str {
        self.header_name.as_str()
    }
}

impl Default for CsrfTokenNames {
    fn default() -> Self {
        Self {
            cookie_name: CSRF_COOKIE.to_string(),
            header_name: HeaderName::from_static(DEFAULT_CSRF_HEADER_LOWER),
        }
    }
}

/// Returns the shared default CSRF token names.
///
/// This is intended for compatibility helpers and tests. Product integrations that support
/// service-specific names should construct and store their own [`CsrfTokenNames`] instead.
pub fn default_csrf_token_names() -> &'static CsrfTokenNames {
    static DEFAULT_NAMES: OnceLock<CsrfTokenNames> = OnceLock::new();
    DEFAULT_NAMES.get_or_init(CsrfTokenNames::default)
}

/// Returns whether `method` can mutate state and should be protected by CSRF checks.
#[must_use]
pub fn is_unsafe_method(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Builds a URL-safe random CSRF token.
#[must_use]
pub fn build_csrf_token() -> String {
    use base64::Engine;

    let mut bytes = [0_u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Validates raw source header values against the request and public-site origins.
///
/// # Errors
///
/// Returns [`CsrfError`] when a header is too long, malformed, missing when required, or untrusted.
pub fn ensure_headers_allowed(
    origin: Option<&str>,
    referer: Option<&str>,
    sec_fetch_site: Option<&str>,
    request_origin: &str,
    public_site_origins: &[String],
    mode: RequestSourceMode,
) -> Result<()> {
    let fetch_site = source_header_value(
        sec_fetch_site,
        MAX_SEC_FETCH_SITE_LEN,
        "Sec-Fetch-Site",
        CsrfErrorKind::RequestHeaderValueInvalid,
    )?
    .map(str::to_ascii_lowercase);

    if fetch_site.as_ref().is_some_and(|value| !value.is_ascii()) {
        return Err(CsrfError::new(
            CsrfErrorKind::RequestHeaderValueInvalid,
            "invalid Sec-Fetch-Site header",
        ));
    }

    if let Some("cross-site" | "none") = fetch_site.as_deref() {
        return Err(CsrfError::new(
            CsrfErrorKind::RequestSourceUntrusted,
            "untrusted request source for cookie-authenticated action",
        ));
    }
    let same_site_fetch = fetch_site.as_deref() == Some("same-site");

    if let Some(origin) = source_header_value(
        origin,
        MAX_SOURCE_HEADER_LEN,
        "Origin",
        CsrfErrorKind::RequestOriginInvalid,
    )?
    .map(|value| normalize_origin(value, CsrfErrorKind::RequestOriginInvalid))
    .transpose()?
    {
        if origin_is_trusted(&origin, request_origin, public_site_origins) {
            return Ok(());
        }
        return Err(CsrfError::new(
            CsrfErrorKind::RequestOriginUntrusted,
            "untrusted request origin for cookie-authenticated action",
        ));
    }

    if let Some(referer) = trimmed_header_value(referer) {
        let referer_origin = origin_from_url(referer)?;
        if origin_is_trusted(&referer_origin, request_origin, public_site_origins) {
            return Ok(());
        }
        return Err(CsrfError::new(
            CsrfErrorKind::RequestRefererUntrusted,
            "untrusted request referer for cookie-authenticated action",
        ));
    }

    if same_site_fetch {
        return Err(CsrfError::new(
            CsrfErrorKind::RequestSourceUntrusted,
            "missing trusted request source for same-site cookie-authenticated action",
        ));
    }

    match mode {
        RequestSourceMode::OptionalWhenPresent => Ok(()),
        RequestSourceMode::Required => Err(CsrfError::new(
            CsrfErrorKind::RequestSourceMissing,
            "missing request source for cookie-authenticated action",
        )),
    }
}

fn validate_cookie_name(cookie_name: &str) -> Result<()> {
    if cookie_name.is_empty() {
        return Err(CsrfError::new(
            CsrfErrorKind::TokenNameInvalid,
            "CSRF cookie name cannot be empty",
        ));
    }
    if cookie_name
        .bytes()
        .any(|byte| byte <= 0x20 || byte >= 0x7f || b"()<>@,;:\\\"/[]?={}".contains(&byte))
    {
        return Err(CsrfError::new(
            CsrfErrorKind::TokenNameInvalid,
            "CSRF cookie name contains invalid characters",
        ));
    }
    Ok(())
}

fn parse_header_name(header_name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(header_name.as_bytes()).map_err(|error| header_name_error(&error))
}

fn header_name_error(error: &InvalidHeaderName) -> CsrfError {
    CsrfError::new(
        CsrfErrorKind::TokenNameInvalid,
        format!("invalid CSRF header name: {error}"),
    )
}

/// Builds a bounded, normalized origin from product-trusted scheme and host values.
///
/// # Errors
/// Returns a classified error for malformed or oversized input.
pub fn request_origin(scheme: &str, host: &str) -> Result<String> {
    ensure_value_len(
        scheme,
        MAX_REQUEST_SCHEME_LEN,
        "request scheme",
        CsrfErrorKind::RequestSchemeInvalid,
    )?;
    ensure_value_len(
        host,
        MAX_REQUEST_HOST_LEN,
        "request host",
        CsrfErrorKind::RequestHostInvalid,
    )?;
    normalize_origin(
        &format!("{scheme}://{host}"),
        CsrfErrorKind::RequestHostInvalid,
    )
    .map_err(|_| CsrfError::new(CsrfErrorKind::RequestHostInvalid, "invalid request host"))
}

fn normalize_origin(origin: &str, kind: CsrfErrorKind) -> Result<String> {
    aster_forge_utils::url::normalize_origin(origin, false)
        .map_err(|_| CsrfError::new(kind, "invalid origin"))
}

fn origin_is_trusted(origin: &str, request_origin: &str, public_site_origins: &[String]) -> bool {
    origin == request_origin || public_site_origins.iter().any(|allowed| allowed == origin)
}

fn source_header_value<'a>(
    value: Option<&'a str>,
    max_len: usize,
    label: &str,
    kind: CsrfErrorKind,
) -> Result<Option<&'a str>> {
    let Some(value) = trimmed_header_value(value) else {
        return Ok(None);
    };
    ensure_value_len(value, max_len, label, kind)?;
    Ok(Some(value))
}

fn trimmed_header_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn ensure_value_len(value: &str, max_len: usize, label: &str, kind: CsrfErrorKind) -> Result<()> {
    if value.len() > max_len {
        return Err(CsrfError::new(
            kind,
            format!("{label} exceeds {max_len} bytes"),
        ));
    }
    Ok(())
}

fn origin_from_url(url: &str) -> Result<String> {
    let scheme_end = url.find("://").ok_or_else(|| {
        CsrfError::new(
            CsrfErrorKind::RequestSchemeInvalid,
            "invalid Referer header",
        )
    })?;
    let scheme = &url[..scheme_end];
    ensure_value_len(
        scheme,
        MAX_REQUEST_SCHEME_LEN,
        "Referer scheme",
        CsrfErrorKind::RequestSchemeInvalid,
    )?;

    let authority_start = scheme_end + 3;
    let authority_tail = &url[authority_start..];
    let authority_end = authority_tail
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, '/' | '?' | '#').then_some(authority_start + idx))
        .unwrap_or(url.len());
    let authority = &url[authority_start..authority_end];
    ensure_value_len(
        authority,
        MAX_REFERER_AUTHORITY_LEN,
        "Referer authority",
        CsrfErrorKind::RequestRefererInvalid,
    )?;

    normalize_origin(
        &format!("{}://{}", scheme.to_ascii_lowercase(), authority),
        CsrfErrorKind::RequestRefererInvalid,
    )
    .map_err(|_| {
        CsrfError::new(
            CsrfErrorKind::RequestRefererInvalid,
            "invalid Referer header",
        )
    })
}

/// Compares decoded double-submit values in constant time for equal-length tokens.
///
/// # Errors
/// Returns a classified error for missing, empty, or mismatched values.
pub fn ensure_token_values(cookie: Option<&str>, header: Option<&str>) -> Result<()> {
    let cookie = cookie
        .ok_or_else(|| CsrfError::new(CsrfErrorKind::CookieMissing, "missing CSRF cookie"))?;
    let header = header
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CsrfError::new(CsrfErrorKind::HeaderMissing, "missing CSRF header"))?;
    // Issued token lengths are public. Compare the secret bytes in constant time.
    if header.len() != cookie.len() || !bool::from(header.as_bytes().ct_eq(cookie.as_bytes())) {
        return Err(CsrfError::new(
            CsrfErrorKind::TokenInvalid,
            "invalid CSRF token",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_comparison_checks_every_byte_and_public_length() {
        let token = build_csrf_token();
        assert!(ensure_token_values(Some(&token), Some(&token)).is_ok());
        for index in 0..token.len() {
            let mut changed = token.as_bytes().to_vec();
            changed[index] = if changed[index] == b'a' { b'b' } else { b'a' };
            let changed = String::from_utf8(changed).unwrap();
            assert_eq!(
                ensure_token_values(Some(&token), Some(&changed))
                    .unwrap_err()
                    .kind(),
                CsrfErrorKind::TokenInvalid
            );
        }
        assert_eq!(
            ensure_token_values(Some(&token), Some(&token[..token.len() - 1]))
                .unwrap_err()
                .kind(),
            CsrfErrorKind::TokenInvalid
        );
    }
    #[test]
    fn accepts_same_origin_and_public_site_origin() {
        assert!(
            ensure_headers_allowed(
                Some("http://localhost"),
                None,
                Some("same-origin"),
                "http://localhost",
                &["https://forge.example.com".to_string()],
                RequestSourceMode::Required,
            )
            .is_ok()
        );

        assert!(
            ensure_headers_allowed(
                Some("https://forge.example.com"),
                None,
                Some("same-origin"),
                "http://127.0.0.1:3000",
                &["https://forge.example.com".to_string()],
                RequestSourceMode::Required,
            )
            .is_ok()
        );
    }
    #[test]
    fn same_site_fetch_metadata_requires_trusted_origin_or_referer() {
        assert!(
            ensure_headers_allowed(
                Some("https://panel.example.com"),
                None,
                Some("same-site"),
                "https://api.example.com",
                &[
                    "https://api.example.com".to_string(),
                    "https://panel.example.com".to_string(),
                ],
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );

        assert!(
            ensure_headers_allowed(
                None,
                Some("https://panel.example.com/settings"),
                Some("same-site"),
                "https://api.example.com",
                &[
                    "https://api.example.com".to_string(),
                    "https://panel.example.com".to_string(),
                ],
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );

        let err = ensure_headers_allowed(
            None,
            None,
            Some("same-site"),
            "https://api.example.com",
            &["https://api.example.com".to_string()],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestSourceUntrusted);
        assert!(err.message().contains("missing trusted request source"));
    }
    #[test]
    fn rejects_untrusted_fetch_metadata_values() {
        for fetch_site in ["cross-site", "none"] {
            let err = ensure_headers_allowed(
                None,
                None,
                Some(fetch_site),
                "https://forge.example.com",
                &[],
                RequestSourceMode::OptionalWhenPresent,
            )
            .unwrap_err();
            assert_eq!(err.kind(), CsrfErrorKind::RequestSourceUntrusted);
            assert!(err.message().contains("untrusted request source"));
        }
    }
    #[test]
    fn rejects_untrusted_origin_and_missing_required_source() {
        let err = ensure_headers_allowed(
            Some("https://evil.example.com"),
            None,
            None,
            "https://forge.example.com",
            &[],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestOriginUntrusted);

        let err = ensure_headers_allowed(
            None,
            None,
            None,
            "https://forge.example.com",
            &[],
            RequestSourceMode::Required,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestSourceMissing);
    }
    #[test]
    fn referer_source_check_ignores_long_path_after_bounded_origin() {
        let long_referer = format!("https://forge.example.com/settings/{}", "a".repeat(10_000));

        assert!(
            ensure_headers_allowed(
                None,
                Some(&long_referer),
                Some("same-origin"),
                "https://forge.example.com",
                &[],
                RequestSourceMode::Required,
            )
            .is_ok()
        );
    }
    #[test]
    fn invalid_referer_missing_scheme_reports_invalid_scheme() {
        let err = ensure_headers_allowed(
            None,
            Some("forge.example.com/settings"),
            None,
            "https://forge.example.com",
            &[],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();

        assert_eq!(err.kind(), CsrfErrorKind::RequestSchemeInvalid);
    }
    #[test]
    fn accepts_missing_optional_source() {
        assert!(
            ensure_headers_allowed(
                None,
                None,
                None,
                "https://forge.example.com",
                &[],
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );
    }
    #[test]
    fn build_csrf_token_returns_url_safe_random_value() {
        let token_a = build_csrf_token();
        let token_b = build_csrf_token();

        assert_ne!(token_a, token_b);
        assert!(token_a.len() >= 32);
        assert!(
            token_a
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
    }
    #[test]
    fn csrf_token_names_reject_invalid_cookie_and_header_names() {
        let err = CsrfTokenNames::new("", "X-CSRF-Token").unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::TokenNameInvalid);

        let err = CsrfTokenNames::new("aster csrf", "X-CSRF-Token").unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::TokenNameInvalid);

        let err = CsrfTokenNames::new("aster_csrf", "bad header").unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::TokenNameInvalid);
    }
}

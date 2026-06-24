//! CSRF helpers for Actix Web services.
//!
//! This module implements product-neutral CSRF mechanics: URL-safe token generation, double-submit
//! cookie/header checks, and request source validation using `Origin`, `Referer`, and
//! `Sec-Fetch-Site`. Callers map [`CsrfErrorKind`] into their own product error codes.
//!
//! The default cookie and header names are compatibility defaults, not a requirement. Services
//! that share a browser origin should pass [`CsrfTokenNames`] into the `*_with_names` helpers so
//! each product can use names that will not collide with other products on the same domain.

use actix_web::{
    HttpRequest,
    dev::ServiceRequest,
    http::{
        Method, header,
        header::{HeaderName, InvalidHeaderName},
    },
};
use rand::RngExt;
use std::sync::OnceLock;

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
    fn new(kind: CsrfErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the product-neutral failure category.
    pub fn kind(&self) -> CsrfErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
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
    /// names are parsed through Actix's HTTP header type and are stored in canonical lower-case
    /// form, which makes comparisons and CORS allow-list generation stable.
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
pub fn is_unsafe_method(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Builds a URL-safe random CSRF token.
pub fn build_csrf_token() -> String {
    use base64::Engine;

    let mut bytes = [0_u8; 32];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Ensures an Actix request contains matching CSRF cookie and header values.
///
/// This uses [`default_csrf_token_names`]. Prefer [`ensure_double_submit_token_with_names`] in
/// products that can run beside another Aster service on the same browser origin.
pub fn ensure_double_submit_token(req: &HttpRequest) -> Result<()> {
    ensure_double_submit_token_with_names(req, default_csrf_token_names())
}

/// Ensures an Actix request contains matching CSRF cookie and header values using custom names.
///
/// The helper only performs the double-submit comparison. Product middleware should decide when to
/// call it, usually for unsafe methods authenticated by cookies. Pair it with request-source
/// validation to reject cross-site writes before checking the token value.
pub fn ensure_double_submit_token_with_names(
    req: &HttpRequest,
    names: &CsrfTokenNames,
) -> Result<()> {
    let cookie_token = req
        .cookie(names.cookie_name())
        .map(|cookie| cookie.value().to_string())
        .ok_or_else(|| CsrfError::new(CsrfErrorKind::CookieMissing, "missing CSRF cookie"))?;
    let header_token = req
        .headers()
        .get(names.header_name())
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CsrfError::new(
                CsrfErrorKind::HeaderMissing,
                format!("missing {} header", names.header_name_str()),
            )
        })?;

    if header_token != cookie_token {
        return Err(CsrfError::new(
            CsrfErrorKind::TokenInvalid,
            "invalid CSRF token",
        ));
    }

    Ok(())
}

/// Ensures an Actix service request contains matching CSRF cookie and header values.
///
/// This uses [`default_csrf_token_names`]. Prefer [`ensure_service_double_submit_token_with_names`]
/// in products that configure service-specific token names.
pub fn ensure_service_double_submit_token(req: &ServiceRequest) -> Result<()> {
    ensure_double_submit_token(req.request())
}

/// Ensures an Actix service request contains matching CSRF cookie and header values using custom
/// names.
pub fn ensure_service_double_submit_token_with_names(
    req: &ServiceRequest,
    names: &CsrfTokenNames,
) -> Result<()> {
    ensure_double_submit_token_with_names(req.request(), names)
}

/// Validates source headers for an Actix request.
pub fn ensure_request_source_allowed(
    req: &HttpRequest,
    public_site_origins: &[String],
    mode: RequestSourceMode,
) -> Result<()> {
    let conn = req.connection_info();
    let request_origin = request_origin(conn.scheme(), conn.host())?;
    ensure_headers_allowed(
        header_value(req, header::ORIGIN),
        header_value(req, header::REFERER),
        header_value(req, header::HeaderName::from_static("sec-fetch-site")),
        &request_origin,
        public_site_origins,
        mode,
    )
}

/// Validates source headers for an Actix service request.
pub fn ensure_service_request_source_allowed(
    req: &ServiceRequest,
    public_site_origins: &[String],
    mode: RequestSourceMode,
) -> Result<()> {
    let conn = req.connection_info();
    let request_origin = request_origin(conn.scheme(), conn.host())?;
    ensure_headers_allowed(
        header_value(req.request(), header::ORIGIN),
        header_value(req.request(), header::REFERER),
        header_value(
            req.request(),
            header::HeaderName::from_static("sec-fetch-site"),
        ),
        &request_origin,
        public_site_origins,
        mode,
    )
}

/// Validates raw source header values against the request and public-site origins.
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
    .map(|value| value.to_ascii_lowercase());

    if let Some(fetch_site) = fetch_site.as_deref() {
        match fetch_site {
            "same-origin" | "same-site" => {}
            "cross-site" | "none" => {
                return Err(CsrfError::new(
                    CsrfErrorKind::RequestSourceUntrusted,
                    "untrusted request source for cookie-authenticated action",
                ));
            }
            _ => {}
        }
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

fn header_value(req: &HttpRequest, name: header::HeaderName) -> Option<&str> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
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
    HeaderName::from_bytes(header_name.as_bytes()).map_err(header_name_error)
}

fn header_name_error(error: InvalidHeaderName) -> CsrfError {
    CsrfError::new(
        CsrfErrorKind::TokenNameInvalid,
        format!("invalid CSRF header name: {error}"),
    )
}

fn request_origin(scheme: &str, host: &str) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use actix_web::cookie::Cookie;

    use super::{
        CSRF_COOKIE, CSRF_HEADER, CsrfErrorKind, CsrfTokenNames, RequestSourceMode,
        build_csrf_token, ensure_double_submit_token, ensure_double_submit_token_with_names,
        ensure_headers_allowed, ensure_request_source_allowed,
    };

    fn host_with_len(len: usize) -> String {
        let suffix = ".example.com";
        format!("{}{}", "a".repeat(len - suffix.len()), suffix)
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
    fn rejects_oversized_request_source_values_before_normalization() {
        let max_host = host_with_len(512);
        let req = actix_web::test::TestRequest::post()
            .insert_header(("Host", max_host.as_str()))
            .insert_header(("Origin", format!("http://{max_host}")))
            .to_http_request();
        assert!(ensure_request_source_allowed(&req, &[], RequestSourceMode::Required).is_ok());

        let long_host = host_with_len(513);
        let req = actix_web::test::TestRequest::post()
            .insert_header(("Host", long_host))
            .insert_header(("Origin", "https://forge.example.com"))
            .to_http_request();
        let err =
            ensure_request_source_allowed(&req, &[], RequestSourceMode::Required).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestHostInvalid);

        let req = actix_web::test::TestRequest::post()
            .insert_header(("Host", "forge.example.com"))
            .insert_header(("X-Forwarded-Proto", "x".repeat(17)))
            .insert_header(("Origin", "https://forge.example.com"))
            .to_http_request();
        let err =
            ensure_request_source_allowed(&req, &[], RequestSourceMode::Required).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestSchemeInvalid);

        let max_origin = format!("https://{}", host_with_len(2040));
        assert_eq!(max_origin.len(), 2048);
        assert!(
            ensure_headers_allowed(
                Some(&max_origin),
                None,
                None,
                "https://forge.example.com",
                std::slice::from_ref(&max_origin),
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );

        let long_origin = format!("https://{}", host_with_len(2041));
        assert_eq!(long_origin.len(), 2049);
        let err = ensure_headers_allowed(
            Some(&long_origin),
            None,
            None,
            "https://forge.example.com",
            &[],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestOriginInvalid);

        let max_referer_authority = host_with_len(528);
        let max_referer_origin = format!("https://{max_referer_authority}");
        let max_referer = format!("{max_referer_origin}/settings");
        assert!(
            ensure_headers_allowed(
                None,
                Some(&max_referer),
                None,
                "https://forge.example.com",
                &[max_referer_origin],
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );

        let long_referer_authority = format!("https://{}.example.com/settings", "a".repeat(600));
        let err = ensure_headers_allowed(
            None,
            Some(&long_referer_authority),
            None,
            "https://forge.example.com",
            &[],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestRefererInvalid);

        let max_fetch_site = "x".repeat(64);
        assert!(
            ensure_headers_allowed(
                None,
                None,
                Some(&max_fetch_site),
                "https://forge.example.com",
                &[],
                RequestSourceMode::OptionalWhenPresent,
            )
            .is_ok()
        );

        let long_fetch_site = "x".repeat(65);
        let err = ensure_headers_allowed(
            None,
            None,
            Some(&long_fetch_site),
            "https://forge.example.com",
            &[],
            RequestSourceMode::OptionalWhenPresent,
        )
        .unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::RequestHeaderValueInvalid);
    }

    #[test]
    fn accepts_ipv6_request_host_origin_match() {
        let req = actix_web::test::TestRequest::post()
            .insert_header(("Host", "[2001:db8::1]:8443"))
            .insert_header(("Origin", "http://[2001:db8::1]:8443"))
            .to_http_request();

        assert!(ensure_request_source_allowed(&req, &[], RequestSourceMode::Required).is_ok());
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
    fn csrf_token_check_requires_cookie_for_cookie_authenticated_writes() {
        let req = actix_web::test::TestRequest::post()
            .uri("/api/v1/auth/profile")
            .to_http_request();

        let err = ensure_double_submit_token(&req).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::CookieMissing);
    }

    #[test]
    fn csrf_token_check_requires_matching_cookie_and_header() {
        let req = actix_web::test::TestRequest::patch()
            .uri("/api/v1/auth/profile")
            .insert_header(("Origin", "http://localhost"))
            .cookie(Cookie::new(CSRF_COOKIE, "token-a"))
            .insert_header((CSRF_HEADER, "token-a"))
            .to_http_request();
        assert!(ensure_double_submit_token(&req).is_ok());

        let missing_header = actix_web::test::TestRequest::patch()
            .uri("/api/v1/auth/profile")
            .insert_header(("Origin", "http://localhost"))
            .cookie(Cookie::new(CSRF_COOKIE, "token-a"))
            .to_http_request();
        let err = ensure_double_submit_token(&missing_header).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::HeaderMissing);

        let mismatch = actix_web::test::TestRequest::patch()
            .uri("/api/v1/auth/profile")
            .insert_header(("Origin", "http://localhost"))
            .cookie(Cookie::new(CSRF_COOKIE, "token-a"))
            .insert_header((CSRF_HEADER, "token-b"))
            .to_http_request();
        let err = ensure_double_submit_token(&mismatch).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::TokenInvalid);
    }

    #[test]
    fn csrf_token_check_accepts_custom_cookie_and_header_names() {
        let names = CsrfTokenNames::new("aster_yggdrasil_csrf", "X-Yggdrasil-CSRF-Token")
            .expect("custom CSRF token names should be valid");
        assert_eq!(names.cookie_name(), "aster_yggdrasil_csrf");
        assert_eq!(names.header_name_str(), "x-yggdrasil-csrf-token");

        let req = actix_web::test::TestRequest::patch()
            .cookie(Cookie::new("aster_yggdrasil_csrf", "token-a"))
            .insert_header(("X-Yggdrasil-CSRF-Token", "token-a"))
            .to_http_request();
        assert!(ensure_double_submit_token_with_names(&req, &names).is_ok());

        let default_req = actix_web::test::TestRequest::patch()
            .cookie(Cookie::new(CSRF_COOKIE, "token-a"))
            .insert_header((CSRF_HEADER, "token-a"))
            .to_http_request();
        let err = ensure_double_submit_token_with_names(&default_req, &names).unwrap_err();
        assert_eq!(err.kind(), CsrfErrorKind::CookieMissing);
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

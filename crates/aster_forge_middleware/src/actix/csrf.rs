//! Actix request/cookie adapter for the shared CSRF core.
use crate::shared::csrf::request_origin;
pub use crate::shared::csrf::{
    CSRF_COOKIE, CSRF_HEADER, CsrfError, CsrfErrorKind, RequestSourceMode, Result,
    build_csrf_token, ensure_headers_allowed,
};
use actix_web::{HttpRequest, dev::ServiceRequest, http::header};
use std::sync::OnceLock;

/// CSRF names adapted to Actix's HTTP 0.2 header type. Validation remains in the shared core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfTokenNames {
    shared: crate::shared::csrf::CsrfTokenNames,
    header_name: header::HeaderName,
}

impl CsrfTokenNames {
    /// Validates product names and adapts the validated header to Actix.
    ///
    /// # Errors
    /// Returns a classified error for invalid cookie/header names.
    pub fn new(cookie_name: impl Into<String>, header_name: impl AsRef<str>) -> Result<Self> {
        let shared = crate::shared::csrf::CsrfTokenNames::new(cookie_name, header_name)?;
        let header_name = header::HeaderName::from_bytes(shared.header_name_str().as_bytes())
            .map_err(|error| CsrfError::new(CsrfErrorKind::TokenNameInvalid, error.to_string()))?;
        Ok(Self {
            shared,
            header_name,
        })
    }
    /// Configured cookie name.
    #[must_use]
    pub fn cookie_name(&self) -> &str {
        self.shared.cookie_name()
    }
    /// Actix header name, preserving the existing transport API.
    #[must_use]
    pub fn header_name(&self) -> &header::HeaderName {
        &self.header_name
    }
    /// Canonical lowercase name for CORS and string-based header APIs.
    #[must_use]
    pub fn header_name_str(&self) -> &str {
        self.shared.header_name_str()
    }
}

impl Default for CsrfTokenNames {
    fn default() -> Self {
        Self {
            shared: crate::shared::csrf::CsrfTokenNames::default(),
            header_name: header::HeaderName::from_static("x-csrf-token"),
        }
    }
}

/// Immutable default names adapted to Actix's header type.
#[must_use]
pub fn default_csrf_token_names() -> &'static CsrfTokenNames {
    static NAMES: OnceLock<CsrfTokenNames> = OnceLock::new();
    NAMES.get_or_init(CsrfTokenNames::default)
}
/// Ensures an Actix request contains matching CSRF cookie and header values.
///
/// This uses [`default_csrf_token_names`]. Prefer [`ensure_double_submit_token_with_names`] in
/// products that can run beside another Aster service on the same browser origin.
///
/// # Errors
///
/// Returns [`CsrfError`] when the cookie or header is missing, empty, or does not match.
pub fn ensure_double_submit_token(req: &HttpRequest) -> Result<()> {
    ensure_double_submit_token_with_names(req, default_csrf_token_names())
}

/// Ensures an Actix request contains matching CSRF cookie and header values using custom names.
///
/// The helper only performs the double-submit comparison. Product middleware should decide when to
/// call it, usually for unsafe methods authenticated by cookies. Pair it with request-source
/// validation to reject cross-site writes before checking the token value.
///
/// # Errors
///
/// Returns [`CsrfError`] when the configured cookie or header is missing, empty, or does not match.
pub fn ensure_double_submit_token_with_names(
    req: &HttpRequest,
    names: &CsrfTokenNames,
) -> Result<()> {
    let cookie = req.cookie(names.cookie_name());
    crate::shared::csrf::ensure_token_values(
        cookie.as_ref().map(actix_web::cookie::Cookie::value),
        req.headers()
            .get(names.header_name_str())
            .and_then(|value| value.to_str().ok()),
    )
}

/// Ensures an Actix service request contains matching CSRF cookie and header values.
///
/// This uses [`default_csrf_token_names`]. Prefer [`ensure_service_double_submit_token_with_names`]
/// in products that configure service-specific token names.
///
/// # Errors
///
/// Returns [`CsrfError`] when the cookie or header is missing, empty, or does not match.
pub fn ensure_service_double_submit_token(req: &ServiceRequest) -> Result<()> {
    ensure_double_submit_token(req.request())
}

/// Ensures an Actix service request contains matching CSRF cookie and header values using custom
/// names.
///
/// # Errors
///
/// Returns [`CsrfError`] when the configured cookie or header is missing, empty, or does not match.
pub fn ensure_service_double_submit_token_with_names(
    req: &ServiceRequest,
    names: &CsrfTokenNames,
) -> Result<()> {
    ensure_double_submit_token_with_names(req.request(), names)
}

/// Validates source headers for an Actix request.
///
/// # Errors
///
/// Returns [`CsrfError`] when the request origin or source headers are malformed or untrusted.
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
///
/// # Errors
///
/// Returns [`CsrfError`] when the request origin or source headers are malformed or untrusted.
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

/// Whether an Actix method needs CSRF protection.
#[must_use]
pub fn is_unsafe_method(method: &actix_web::http::Method) -> bool {
    !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE")
}
fn header_value(req: &HttpRequest, name: header::HeaderName) -> Option<&str> {
    req.headers()
        .get(name)
        .map(|value| value.to_str().unwrap_or("\u{fffd}"))
}

#[cfg(test)]
mod tests {
    use actix_web::cookie::Cookie;

    use super::{
        CSRF_COOKIE, CSRF_HEADER, CsrfErrorKind, CsrfTokenNames, RequestSourceMode,
        ensure_double_submit_token, ensure_double_submit_token_with_names, ensure_headers_allowed,
        ensure_request_source_allowed,
    };

    fn host_with_len(len: usize) -> String {
        let suffix = ".example.com";
        format!("{}{}", "a".repeat(len - suffix.len()), suffix)
    }

    #[test]
    fn configured_header_name_remains_usable_with_actix_header_apis() {
        let names = CsrfTokenNames::new("custom_csrf", "X-Custom-CSRF").unwrap();
        let req = actix_web::test::TestRequest::post()
            .cookie(Cookie::new("custom_csrf", "token"))
            .insert_header((names.header_name(), "token"))
            .to_http_request();
        assert!(ensure_double_submit_token_with_names(&req, &names).is_ok());
    }

    #[test]
    fn opaque_source_headers_are_rejected_in_optional_mode() {
        for (name, expected) in [
            ("origin", CsrfErrorKind::RequestOriginInvalid),
            ("referer", CsrfErrorKind::RequestSchemeInvalid),
            ("sec-fetch-site", CsrfErrorKind::RequestHeaderValueInvalid),
        ] {
            let req = actix_web::test::TestRequest::post()
                .insert_header((
                    name,
                    actix_web::http::header::HeaderValue::from_bytes(&[0xff]).unwrap(),
                ))
                .to_http_request();
            assert_eq!(
                ensure_request_source_allowed(&req, &[], RequestSourceMode::OptionalWhenPresent)
                    .unwrap_err()
                    .kind(),
                expected
            );
        }
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
    fn csrf_token_check_rejects_tokens_of_different_lengths() {
        let req = actix_web::test::TestRequest::patch()
            .uri("/api/v1/auth/profile")
            .cookie(Cookie::new(CSRF_COOKIE, "token-a"))
            .insert_header((CSRF_HEADER, "token-a-with-a-longer-value"))
            .to_http_request();
        let err = ensure_double_submit_token(&req).unwrap_err();
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
}

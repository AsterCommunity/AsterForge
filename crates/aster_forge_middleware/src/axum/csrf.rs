//! Axum CSRF adapter with explicit product route, source and error boundaries.
pub use crate::shared::csrf::{CsrfError, CsrfErrorKind, CsrfTokenNames, RequestSourceMode};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http::{HeaderMap, header};
use std::sync::Arc;

/// Runtime source policy supplied by product configuration.
#[derive(Clone)]
pub struct RequestSourcePolicy {
    /// Trusted canonical request scheme (e.g. configured HTTPS), not raw forwarded headers.
    pub scheme: String,
    /// Trusted canonical host/authority; product validates proxy provenance before using it.
    pub host: String,
    /// Already-normalized public site origins.
    pub public_site_origins: Vec<String>,
    /// Whether a trusted Origin/Referer is mandatory.
    pub mode: RequestSourceMode,
}

/// Checks decoded cookies and headers using the shared constant-time comparison.
///
/// # Errors
/// Returns a classified error for missing, empty or mismatched tokens.
pub fn ensure_double_submit_token_with_names(
    headers: &HeaderMap,
    names: &CsrfTokenNames,
) -> crate::shared::csrf::Result<()> {
    let cookie = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|value| cookie::Cookie::parse_encoded(value.trim()).ok())
        .find(|cookie| cookie.name() == names.cookie_name());
    crate::shared::csrf::ensure_token_values(
        cookie.as_ref().map(cookie::Cookie::value),
        headers
            .get(names.header_name())
            .and_then(|value| value.to_str().ok()),
    )
}

/// Checks tokens using the default names.
///
/// # Errors
/// Returns a classified error for missing, empty or mismatched tokens.
pub fn ensure_double_submit_token(headers: &HeaderMap) -> crate::shared::csrf::Result<()> {
    ensure_double_submit_token_with_names(headers, crate::shared::csrf::default_csrf_token_names())
}

/// Checks request source headers against explicit trusted scheme/host and public origins.
/// Opaque non-UTF8 headers are invalid, including in optional mode.
///
/// # Errors
/// Returns a classified error for malformed, missing or untrusted source values.
pub fn ensure_request_source_allowed(
    headers: &HeaderMap,
    policy: &RequestSourcePolicy,
) -> crate::shared::csrf::Result<()> {
    let origin = crate::shared::csrf::request_origin(&policy.scheme, &policy.host)?;
    let value = |name| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap_or("\u{fffd}"))
    };
    crate::shared::csrf::ensure_headers_allowed(
        value("origin"),
        value("referer"),
        value("sec-fetch-site"),
        &origin,
        &policy.public_site_origins,
        policy.mode,
    )
}

type Predicate = dyn Fn(&Request) -> bool + Send + Sync;
type SourceResolver = dyn Fn(&Request) -> Result<RequestSourcePolicy, Box<Response>> + Send + Sync;
type ErrorMapper = dyn Fn(CsrfError) -> Response + Send + Sync;

/// State for `from_fn_with_state(config, csrf)`. Product predicates decide cookie authentication.
#[derive(Clone)]
pub struct CsrfConfig {
    names: CsrfTokenNames,
    protect: Arc<Predicate>,
    source: Arc<SourceResolver>,
    map_error: Arc<ErrorMapper>,
}

impl CsrfConfig {
    /// Builds middleware with explicit product callbacks; safe methods always bypass protection.
    pub fn new<P, S, M>(names: CsrfTokenNames, protect: P, source: S, map_error: M) -> Self
    where
        P: Fn(&Request) -> bool + Send + Sync + 'static,
        S: Fn(&Request) -> Result<RequestSourcePolicy, Box<Response>> + Send + Sync + 'static,
        M: Fn(CsrfError) -> Response + Send + Sync + 'static,
    {
        Self {
            names,
            protect: Arc::new(protect),
            source: Arc::new(source),
            map_error: Arc::new(map_error),
        }
    }
}

/// Enforces source and double-submit checks on product-selected unsafe requests.
pub async fn csrf(State(config): State<CsrfConfig>, request: Request, next: Next) -> Response {
    if crate::shared::csrf::is_unsafe_method(request.method()) && (config.protect)(&request) {
        let policy = match (config.source)(&request) {
            Ok(policy) => policy,
            Err(response) => return *response,
        };
        let result = ensure_request_source_allowed(request.headers(), &policy)
            .and_then(|()| ensure_double_submit_token_with_names(request.headers(), &config.names));
        if let Err(error) = result {
            return (config.map_error)(error);
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {

    use super::{CsrfConfig, CsrfTokenNames, RequestSourceMode, RequestSourcePolicy, csrf};
    use axum::{
        Router,
        body::{Body, to_bytes},
        middleware,
        response::IntoResponse,
        routing::any,
    };
    use http::{HeaderValue, Request, StatusCode};
    use tower::ServiceExt;

    fn policy() -> RequestSourcePolicy {
        RequestSourcePolicy {
            scheme: "https".into(),
            host: "api.example.com".into(),
            public_site_origins: vec!["https://panel.example.com".into()],
            mode: RequestSourceMode::Required,
        }
    }

    #[tokio::test]
    async fn product_source_resolver_failure_is_returned_unchanged() {
        let config = CsrfConfig::new(
            CsrfTokenNames::default(),
            |_| true,
            |_| {
                Err(Box::new(
                    (StatusCode::SERVICE_UNAVAILABLE, "source policy unavailable").into_response(),
                ))
            },
            |_| StatusCode::FORBIDDEN.into_response(),
        );
        let router = Router::new()
            .fallback(any(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, csrf));
        assert_eq!(
            result(
                router,
                Request::builder()
                    .method("POST")
                    .body(Body::empty())
                    .unwrap()
            )
            .await,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "source policy unavailable".into()
            )
        );
    }

    fn app(names: CsrfTokenNames, source: RequestSourcePolicy) -> Router {
        let config = CsrfConfig::new(
            names,
            |req| req.uri().path() != "/public",
            move |_| Ok(source.clone()),
            |error| (StatusCode::FORBIDDEN, format!("{:?}", error.kind())).into_response(),
        );
        Router::new()
            .fallback(any(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, csrf))
    }

    async fn result(app: Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn safe_methods_and_product_exempt_routes_bypass_checks() {
        for method in ["GET", "HEAD", "OPTIONS", "TRACE"] {
            let (status, _) = result(
                app(CsrfTokenNames::default(), policy()),
                Request::builder()
                    .method(method)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{method}");
        }
        for method in ["POST", "PATCH", "PUT", "DELETE"] {
            let router = app(CsrfTokenNames::default(), policy());
            let (status, body) = result(
                router.clone(),
                Request::builder()
                    .method(method)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                (status, body.as_str()),
                (StatusCode::FORBIDDEN, "RequestSourceMissing")
            );
            let (status, _) = result(
                router,
                Request::builder()
                    .method(method)
                    .uri("/public")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn token_failures_are_mapped_before_the_handler_runs() {
        for (cookie, header, kind) in [
            (None, None, "CookieMissing"),
            (Some("aster_csrf=token"), None, "HeaderMissing"),
            (Some("aster_csrf=token"), Some(""), "HeaderMissing"),
            (Some("aster_csrf=token"), Some("  "), "HeaderMissing"),
            (Some("aster_csrf="), Some("token"), "TokenInvalid"),
            (Some("aster_csrf=token"), Some("wrong"), "TokenInvalid"),
            (
                Some("aster_csrf=token"),
                Some("longer-token"),
                "TokenInvalid",
            ),
            (Some("aster_csrf=token"), Some(" token "), "called"),
        ] {
            let mut req = Request::builder()
                .method("POST")
                .header("origin", "https://api.example.com");
            if let Some(cookie) = cookie {
                req = req.header("cookie", cookie);
            }
            if let Some(header) = header {
                req = req.header("x-csrf-token", header);
            }
            let (status, body) = result(
                app(CsrfTokenNames::default(), policy()),
                req.body(Body::empty()).unwrap(),
            )
            .await;
            assert_eq!(body, kind);
            assert_eq!(
                status,
                if kind == "called" {
                    StatusCode::OK
                } else {
                    StatusCode::FORBIDDEN
                }
            );
        }
    }

    #[tokio::test]
    async fn custom_names_and_encoded_multi_cookie_headers_work() {
        let names = CsrfTokenNames::new("product_csrf", "X-Product-CSRF").unwrap();
        let router = app(names, policy());
        let request = Request::builder()
            .method("PATCH")
            .header("origin", "https://panel.example.com")
            .header("cookie", "other=value; invalid")
            .header("cookie", "product_csrf=token%2Da")
            .header("X-Product-CSRF", "token-a")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            result(router.clone(), request).await,
            (StatusCode::OK, "called".into())
        );
        let request = Request::builder()
            .method("PATCH")
            .header("origin", "https://api.example.com")
            .header("cookie", "aster_csrf=token-a")
            .header("X-CSRF-Token", "token-a")
            .body(Body::empty())
            .unwrap();
        assert_eq!(result(router, request).await.1, "CookieMissing");
    }

    #[tokio::test]
    async fn source_origin_referer_priority_and_fetch_metadata_match_actix() {
        for (origin, referer, fetch, expected) in [
            (
                Some("https://api.example.com"),
                None,
                Some("same-origin"),
                "called",
            ),
            (
                Some("https://panel.example.com"),
                None,
                Some("same-site"),
                "called",
            ),
            (
                None,
                Some("https://panel.example.com/settings"),
                Some("same-site"),
                "called",
            ),
            (
                Some("https://evil.example.com"),
                Some("https://api.example.com/"),
                None,
                "RequestOriginUntrusted",
            ),
            (
                Some("https://api.example.com"),
                Some("https://evil.example.com/"),
                None,
                "called",
            ),
            (
                None,
                Some("https://evil.example.com/"),
                None,
                "RequestRefererUntrusted",
            ),
            (Some("null"), None, None, "RequestOriginInvalid"),
            (
                None,
                Some("missing-scheme/path"),
                None,
                "RequestSchemeInvalid",
            ),
            (None, None, Some("same-site"), "RequestSourceUntrusted"),
            (
                Some("https://api.example.com"),
                None,
                Some("cross-site"),
                "RequestSourceUntrusted",
            ),
            (
                Some("https://api.example.com"),
                None,
                Some("none"),
                "RequestSourceUntrusted",
            ),
            (None, None, None, "RequestSourceMissing"),
        ] {
            let mut req = Request::builder()
                .method("POST")
                .header("cookie", "aster_csrf=t")
                .header("x-csrf-token", "t");
            if let Some(origin) = origin {
                req = req.header("origin", origin);
            }
            if let Some(referer) = referer {
                req = req.header("referer", referer);
            }
            if let Some(fetch) = fetch {
                req = req.header("sec-fetch-site", fetch);
            }
            assert_eq!(
                result(
                    app(CsrfTokenNames::default(), policy()),
                    req.body(Body::empty()).unwrap()
                )
                .await
                .1,
                expected
            );
        }
    }

    #[tokio::test]
    async fn optional_mode_still_rejects_opaque_and_oversized_headers() {
        let mut source = policy();
        source.mode = RequestSourceMode::OptionalWhenPresent;
        let router = app(CsrfTokenNames::default(), source);
        for (name, value, expected) in [
            (
                "origin",
                HeaderValue::from_bytes(&[0xff]).unwrap(),
                "RequestOriginInvalid",
            ),
            (
                "referer",
                HeaderValue::from_bytes(&[0xff]).unwrap(),
                "RequestSchemeInvalid",
            ),
            (
                "sec-fetch-site",
                HeaderValue::from_bytes(&[0xff]).unwrap(),
                "RequestHeaderValueInvalid",
            ),
            (
                "origin",
                HeaderValue::from_str(&format!("https://{}.com", "a".repeat(2048))).unwrap(),
                "RequestOriginInvalid",
            ),
            (
                "referer",
                HeaderValue::from_str(&format!("https://{}.com/", "a".repeat(529))).unwrap(),
                "RequestRefererInvalid",
            ),
            (
                "sec-fetch-site",
                HeaderValue::from_str(&"a".repeat(65)).unwrap(),
                "RequestHeaderValueInvalid",
            ),
        ] {
            let req = Request::builder()
                .method("POST")
                .header(name, value)
                .header("cookie", "aster_csrf=t")
                .header("x-csrf-token", "t")
                .body(Body::empty())
                .unwrap();
            assert_eq!(result(router.clone(), req).await.1, expected);
        }
        let req = Request::builder()
            .method("POST")
            .header("cookie", "aster_csrf=t")
            .header("x-csrf-token", "t")
            .body(Body::empty())
            .unwrap();
        assert_eq!(result(router, req).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn trusted_scheme_host_are_bounded_and_cannot_be_forged_by_headers() {
        let mut source = policy();
        source.scheme = "x".repeat(17);
        let req = || {
            Request::builder()
                .method("POST")
                .header("origin", "https://api.example.com")
                .header("cookie", "aster_csrf=t")
                .header("x-csrf-token", "t")
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            result(app(CsrfTokenNames::default(), source), req())
                .await
                .1,
            "RequestSchemeInvalid"
        );
        let mut source = policy();
        source.host = "a".repeat(513);
        assert_eq!(
            result(app(CsrfTokenNames::default(), source), req())
                .await
                .1,
            "RequestHostInvalid"
        );
        let req = Request::builder()
            .method("POST")
            .header("origin", "https://evil.example.com")
            .header("host", "evil.example.com")
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "evil.example.com")
            .header("cookie", "aster_csrf=t")
            .header("x-csrf-token", "t")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            result(app(CsrfTokenNames::default(), policy()), req)
                .await
                .1,
            "RequestOriginUntrusted"
        );
    }
}

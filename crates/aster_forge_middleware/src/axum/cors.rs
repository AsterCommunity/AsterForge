//! Dynamic CORS using the same policy/validation/header engine as Actix.
pub use crate::shared::cors::{
    CorsAllowedOrigins, CorsMiddlewareError, CorsMiddlewareErrorKind, RuntimeCorsPolicy,
};
use crate::shared::cors::{CorsDecision, CorsSettings};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use http::{HeaderValue, StatusCode, header};
use std::sync::Arc;

type PolicyResolver = dyn Fn(&Request) -> Result<RuntimeCorsPolicy, Box<Response>> + Send + Sync;
type ExemptPredicate = dyn Fn(&str) -> bool + Send + Sync;
type ErrorMapper = dyn Fn(CorsMiddlewareError) -> Response + Send + Sync;
type OriginResolver = dyn Fn(&Request) -> Result<Option<String>, Box<Response>> + Send + Sync;

/// State for `from_fn_with_state(config, runtime_cors)`. No CORS policy is enabled implicitly.
#[derive(Clone)]
pub struct RuntimeCorsConfig {
    settings: CorsSettings,
    policy: Arc<PolicyResolver>,
    exempt_path: Arc<ExemptPredicate>,
    map_error: Arc<ErrorMapper>,
    request_origin: Arc<OriginResolver>,
}

impl RuntimeCorsConfig {
    /// Supplies dynamic product policy, exempt paths, and classified error mapping.
    pub fn new<P, X, M>(policy: P, exempt_path: X, map_error: M) -> Self
    where
        P: Fn(&Request) -> Result<RuntimeCorsPolicy, Box<Response>> + Send + Sync + 'static,
        X: Fn(&str) -> bool + Send + Sync + 'static,
        M: Fn(CorsMiddlewareError) -> Response + Send + Sync + 'static,
    {
        Self {
            settings: CorsSettings::default(),
            policy: Arc::new(policy),
            exempt_path: Arc::new(exempt_path),
            map_error: Arc::new(map_error),
            request_origin: Arc::new(|_| Ok(None)),
        }
    }

    /// Provides a normalized, product-trusted request origin for same-origin bypass.
    /// The default `None` disables bypass; raw Host/Forwarded headers are never trusted implicitly.
    #[must_use]
    pub fn request_origin<R>(mut self, resolver: R) -> Self
    where
        R: Fn(&Request) -> Result<Option<String>, Box<Response>> + Send + Sync + 'static,
    {
        self.request_origin = Arc::new(resolver);
        self
    }

    /// Sets preflight methods.
    #[must_use]
    pub fn allowed_methods(mut self, methods: impl IntoIterator<Item = &'static str>) -> Self {
        self.settings.allowed_methods = methods.into_iter().collect();
        self
    }
    /// Sets allowed request headers; matching ignores name case.
    #[must_use]
    pub fn allowed_headers(mut self, headers: impl IntoIterator<Item = &'static str>) -> Self {
        self.settings.allowed_headers = headers.into_iter().collect();
        self
    }
    /// Sets response headers exposed to browser JavaScript.
    #[must_use]
    pub fn exposed_headers(mut self, headers: impl IntoIterator<Item = &'static str>) -> Self {
        self.settings.exposed_headers = headers.into_iter().collect();
        self
    }
    /// Adds origin schemes to parsing; full origins must still match the policy.
    #[must_use]
    pub fn additional_origin_schemes(
        mut self,
        schemes: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.settings.additional_origin_schemes = schemes.into_iter().collect();
        self
    }
}

/// Enforces a freshly resolved policy per cross-origin request, including preflight.
pub async fn runtime_cors(
    State(config): State<RuntimeCorsConfig>,
    request: Request,
    next: Next,
) -> Response {
    if (config.exempt_path)(request.uri().path()) || !request.headers().contains_key(header::ORIGIN)
    {
        return next.run(request).await;
    }
    let policy = match (config.policy)(&request) {
        Ok(policy) => policy,
        Err(response) => return *response,
    };
    if !policy.enforces_requests() {
        return next.run(request).await;
    }
    let origin = match (config.request_origin)(&request) {
        Ok(origin) => origin,
        Err(response) => return *response,
    };
    let decision = crate::shared::cors::evaluate(
        request.method().as_str(),
        request.headers(),
        origin.as_deref(),
        &policy,
        &config.settings,
    );
    match decision {
        Err(error) => (config.map_error)(error),
        Ok(CorsDecision::Pass) => next.run(request).await,
        Ok(CorsDecision::Reject) => {
            let mut response = StatusCode::FORBIDDEN.into_response();
            response.headers_mut().insert(
                header::VARY,
                HeaderValue::from_static(
                    "Access-Control-Request-Headers, Access-Control-Request-Method, Origin",
                ),
            );
            response
        }
        Ok(CorsDecision::Preflight(headers)) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            *response.headers_mut() = headers;
            response
        }
        Ok(CorsDecision::Actual(origin)) => {
            let mut response = next.run(request).await;
            let result =
                crate::shared::cors::apply_origin_headers(response.headers_mut(), &policy, &origin)
                    .and_then(|()| {
                        crate::shared::cors::apply_actual_headers(
                            response.headers_mut(),
                            &config.settings,
                        )
                    });
            match result {
                Ok(()) => response,
                Err(error) => (config.map_error)(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CorsAllowedOrigins, CorsMiddlewareErrorKind, RuntimeCorsConfig, RuntimeCorsPolicy,
        runtime_cors,
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        middleware,
        response::{IntoResponse, Response},
        routing::any,
    };
    use http::{HeaderValue, Request, StatusCode, header};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn policy(origins: CorsAllowedOrigins, credentials: bool) -> RuntimeCorsPolicy {
        RuntimeCorsPolicy {
            enabled: true,
            allowed_origins: origins,
            allow_credentials: credentials,
            max_age_secs: 600,
        }
    }
    fn config(policy: RuntimeCorsPolicy) -> RuntimeCorsConfig {
        RuntimeCorsConfig::new(
            move |_| Ok(policy.clone()),
            |path| path == "/public",
            |error| {
                let status = match error.kind() {
                    CorsMiddlewareErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
                    CorsMiddlewareErrorKind::InvalidResponse => StatusCode::INTERNAL_SERVER_ERROR,
                };
                (status, format!("{:?}", error.kind())).into_response()
            },
        )
        .allowed_methods(["GET", "POST", "OPTIONS"])
        .allowed_headers(["Content-Type", "X-CSRF-Token"])
        .exposed_headers(["x-request-id", "content-length"])
    }
    fn listed() -> RuntimeCorsConfig {
        config(policy(
            CorsAllowedOrigins::List(vec!["https://panel.example.com".into()]),
            true,
        ))
    }
    fn app(config: RuntimeCorsConfig) -> Router {
        Router::new()
            .fallback(any(|| async { "called" }))
            .layer(middleware::from_fn_with_state(config, runtime_cors))
    }
    fn request(method: &str, origin: &str) -> http::request::Builder {
        Request::builder().method(method).header("origin", origin)
    }
    async fn send(router: Router, builder: http::request::Builder) -> Response {
        router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn body(response: Response) -> String {
        String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn preflight_allows_case_insensitive_header_names_and_does_not_call_handler() {
        let response = send(
            app(listed()),
            request("OPTIONS", "https://panel.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "content-type, X-Csrf-Token",
                ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://panel.example.com"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS],
            "true"
        );
        assert_eq!(response.headers()[header::ACCESS_CONTROL_MAX_AGE], "600");
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS],
            "GET, POST, OPTIONS"
        );
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "Content-Type, X-CSRF-Token"
        );
        for name in [
            "Origin",
            "Access-Control-Request-Method",
            "Access-Control-Request-Headers",
        ] {
            assert!(
                response.headers()[header::VARY]
                    .to_str()
                    .unwrap()
                    .contains(name)
            );
        }
        assert!(body(response).await.is_empty());
    }

    #[tokio::test]
    async fn preflight_and_actual_requests_deny_disallowed_origins_methods_or_headers() {
        for builder in [
            request("POST", "https://evil.example.com"),
            request("OPTIONS", "https://evil.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST"),
            request("OPTIONS", "https://panel.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE"),
            request("OPTIONS", "https://panel.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-secret"),
        ] {
            let response = send(app(listed()), builder).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(response.headers().contains_key(header::VARY));
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
        }
    }

    #[tokio::test]
    async fn actual_requests_reflect_credentials_or_use_wildcard_without_credentials() {
        for credentials in [false, true] {
            let response = send(
                app(config(policy(CorsAllowedOrigins::Any, credentials))),
                request("GET", "https://panel.example.com"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                if credentials {
                    "https://panel.example.com"
                } else {
                    "*"
                }
            );
            assert_eq!(
                response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
                credentials
            );
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS],
                "x-request-id, content-length"
            );
            assert_eq!(body(response).await, "called");
        }
    }

    #[tokio::test]
    async fn inactive_policy_absent_origin_and_exempt_path_pass_through_without_parsing() {
        for mut policy in [
            policy(CorsAllowedOrigins::None, false),
            policy(CorsAllowedOrigins::Any, false),
        ] {
            if policy.allowed_origins == CorsAllowedOrigins::Any {
                policy.enabled = false;
            }
            let response = send(
                app(config(policy)),
                Request::builder().header("origin", HeaderValue::from_bytes(&[0xff]).unwrap()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
        }
        for builder in [Request::builder(), request("GET", "null").uri("/public")] {
            let response = send(app(listed()), builder).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
        }
    }

    #[tokio::test]
    async fn runtime_policy_changes_take_effect_on_the_same_router() {
        let current = Arc::new(Mutex::new(policy(
            CorsAllowedOrigins::List(vec!["https://one.example.com".into()]),
            false,
        )));
        let state = Arc::clone(&current);
        let config = RuntimeCorsConfig::new(
            move |_| Ok(state.lock().unwrap().clone()),
            |_| false,
            |_| StatusCode::BAD_REQUEST.into_response(),
        );
        let router = app(config);
        assert_eq!(
            send(router.clone(), request("GET", "https://one.example.com"))
                .await
                .status(),
            StatusCode::OK
        );
        *current.lock().unwrap() = policy(
            CorsAllowedOrigins::List(vec!["https://two.example.com".into()]),
            false,
        );
        assert_eq!(
            send(router.clone(), request("GET", "https://one.example.com"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(router, request("GET", "https://two.example.com"))
                .await
                .headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://two.example.com"
        );
    }

    #[tokio::test]
    async fn additional_scheme_requires_an_exact_full_origin_match() {
        let config = config(policy(
            CorsAllowedOrigins::List(vec!["chrome-extension://first-extension".into()]),
            true,
        ))
        .additional_origin_schemes(["chrome-extension"]);
        let router = app(config);
        assert_eq!(
            send(
                router.clone(),
                request("GET", "chrome-extension://first-extension")
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(router, request("GET", "chrome-extension://other-extension"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn same_origin_bypass_requires_a_product_trusted_origin_resolver() {
        let response = send(
            app(listed()),
            request("GET", "https://evil.example.com")
                .header("host", "evil.example.com")
                .header("x-forwarded-proto", "https"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = send(
            app(listed().request_origin(|_| Ok(Some("https://api.example.com".into())))),
            request("GET", "https://api.example.com"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    #[tokio::test]
    async fn vary_merges_all_values_case_insensitively_and_preserves_wildcard() {
        for wildcard in [false, true] {
            let router = Router::new()
                .route(
                    "/",
                    any(move || async move {
                        let mut response = "called".into_response();
                        response.headers_mut().append(
                            header::VARY,
                            HeaderValue::from_static("Accept-Encoding, origin"),
                        );
                        response.headers_mut().append(
                            header::VARY,
                            HeaderValue::from_static(if wildcard {
                                "*"
                            } else {
                                "Accept-Language"
                            }),
                        );
                        response.headers_mut().insert(
                            header::ACCESS_CONTROL_ALLOW_ORIGIN,
                            HeaderValue::from_static("https://product.example.com"),
                        );
                        response
                    }),
                )
                .layer(middleware::from_fn_with_state(listed(), runtime_cors));
            let response = send(router, request("GET", "https://panel.example.com")).await;
            let values = response
                .headers()
                .get_all(header::VARY)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>()
                .join(", ");
            assert!(values.contains("Accept-Encoding"));
            if wildcard {
                assert!(values.contains('*'));
            } else {
                assert!(values.contains("Accept-Language"));
                assert_eq!(values.to_ascii_lowercase().matches("origin").count(), 1);
            }
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "https://product.example.com"
            );
        }
    }

    #[tokio::test]
    async fn request_and_response_errors_keep_distinct_product_mappings() {
        for builder in [
            Request::builder().header("origin", HeaderValue::from_bytes(&[0xff]).unwrap()),
            request("GET", "null"),
            request("OPTIONS", "https://panel.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "bad name"),
        ] {
            let response = send(app(listed()), builder).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(body(response).await, "InvalidRequest");
        }
        let response = send(
            app(listed().exposed_headers(["bad\nheader"])),
            request("GET", "https://panel.example.com"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body(response).await, "InvalidResponse");
        let router = Router::new()
            .fallback(any(|| async {
                let mut response = "called".into_response();
                response
                    .headers_mut()
                    .insert(header::VARY, HeaderValue::from_bytes(&[0xff]).unwrap());
                response
            }))
            .layer(middleware::from_fn_with_state(listed(), runtime_cors));
        let response = send(router, request("GET", "https://panel.example.com")).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body(response).await, "InvalidResponse");
    }

    #[tokio::test]
    async fn product_policy_resolver_errors_are_returned_unchanged() {
        let config = RuntimeCorsConfig::new(
            |_| {
                Err(Box::new(
                    (StatusCode::SERVICE_UNAVAILABLE, "policy unavailable").into_response(),
                ))
            },
            |_| false,
            |_| StatusCode::BAD_REQUEST.into_response(),
        );
        let response = send(app(config), request("GET", "https://panel.example.com")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body(response).await, "policy unavailable");
    }

    #[tokio::test]
    async fn product_origin_resolver_failure_is_returned_unchanged() {
        let config = listed().request_origin(|_| {
            Err(Box::new(
                (StatusCode::SERVICE_UNAVAILABLE, "origin unavailable").into_response(),
            ))
        });
        let response = send(app(config), request("GET", "https://panel.example.com")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body(response).await, "origin unavailable");
    }

    #[tokio::test]
    async fn invalid_generated_preflight_headers_are_response_errors() {
        let response = send(
            app(listed().allowed_headers(["content-type", "bad\nheader"])),
            request("OPTIONS", "https://panel.example.com")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body(response).await, "InvalidResponse");
    }
}

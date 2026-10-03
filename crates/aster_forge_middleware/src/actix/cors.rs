//! Runtime CORS middleware for Actix Web services.
//!
//! Aster products often store CORS settings in runtime configuration rather than in a static Actix
//! builder. This module owns the reusable middleware mechanics: reading `Origin`, rejecting
//! disallowed cross-origin requests, handling preflight requests, applying CORS response headers,
//! and maintaining `Vary`. Product crates provide a policy resolver, exempt-path predicate,
//! allowed/exposed header lists, and error mapping.

use std::rc::Rc;

use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::{
        header,
        header::{HeaderMap, HeaderValue},
    },
};
use futures::future::{LocalBoxFuture, Ready, ok};

pub use crate::shared::cors::{
    CorsAllowedOrigins, CorsMiddlewareError, CorsMiddlewareErrorKind, RuntimeCorsPolicy,
};
use crate::shared::cors::{CorsDecision, CorsSettings};

type PolicyResolver = dyn Fn(&ServiceRequest) -> Result<RuntimeCorsPolicy, Error>;
type ExemptPathPredicate = dyn Fn(&str) -> bool;
type ErrorMapper = dyn Fn(CorsMiddlewareError) -> Error;

/// Runtime CORS middleware configuration.
pub struct RuntimeCorsConfig {
    settings: CorsSettings,
    policy: Rc<PolicyResolver>,
    exempt_path: Rc<ExemptPathPredicate>,
    map_error: Rc<ErrorMapper>,
}

impl RuntimeCorsConfig {
    /// Builds a configuration from product-provided callbacks.
    pub fn new<P, X, M>(policy: P, exempt_path: X, map_error: M) -> Self
    where
        P: Fn(&ServiceRequest) -> Result<RuntimeCorsPolicy, Error> + 'static,
        X: Fn(&str) -> bool + 'static,
        M: Fn(CorsMiddlewareError) -> Error + 'static,
    {
        Self {
            settings: CorsSettings::default(),
            policy: Rc::new(policy),
            exempt_path: Rc::new(exempt_path),
            map_error: Rc::new(map_error),
        }
    }

    /// Sets preflight-allowed methods.
    #[must_use]
    pub fn allowed_methods(mut self, methods: impl IntoIterator<Item = &'static str>) -> Self {
        self.settings.allowed_methods = methods.into_iter().collect();
        self
    }

    /// Sets preflight-allowed request headers.
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

    /// Accepts selected non-HTTP schemes while parsing request origins.
    ///
    /// This does not authorize a scheme by itself. The normalized full origin must still match
    /// [`RuntimeCorsPolicy::allowed_origins`].
    #[must_use]
    pub fn additional_origin_schemes(
        mut self,
        schemes: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.settings.additional_origin_schemes = schemes.into_iter().collect();
        self
    }
}

impl Clone for RuntimeCorsConfig {
    fn clone(&self) -> Self {
        Self {
            settings: self.settings.clone(),
            policy: Rc::clone(&self.policy),
            exempt_path: Rc::clone(&self.exempt_path),
            map_error: Rc::clone(&self.map_error),
        }
    }
}

/// Actix runtime CORS middleware.
pub struct RuntimeCors {
    config: RuntimeCorsConfig,
}

impl RuntimeCors {
    /// Creates runtime CORS middleware from a product configuration.
    #[must_use]
    pub fn new(config: RuntimeCorsConfig) -> Self {
        Self { config }
    }
}

impl<S, B> Transform<S, ServiceRequest> for RuntimeCors
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type InitError = ();
    type Transform = RuntimeCorsMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RuntimeCorsMiddleware {
            service: Rc::new(service),
            config: self.config.clone(),
        })
    }
}

/// Service wrapper installed by [`RuntimeCors`].
pub struct RuntimeCorsMiddleware<S> {
    service: Rc<S>,
    config: RuntimeCorsConfig,
}

impl<S, B> Service<ServiceRequest> for RuntimeCorsMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let svc = Rc::clone(&self.service);
        let config = self.config.clone();

        Box::pin(async move {
            if (config.exempt_path)(req.path()) {
                return Ok(svc.call(req).await?.map_into_left_body());
            }

            if !req.headers().contains_key(header::ORIGIN) {
                return Ok(svc.call(req).await?.map_into_left_body());
            }
            let policy = (config.policy)(&req)?;
            if !policy.enforces_requests() {
                return Ok(svc.call(req).await?.map_into_left_body());
            }
            // Actix's connection_info contract is retained. Products must sanitize proxy headers.
            let request_origin = {
                let conn = req.connection_info();
                format!(
                    "{}://{}",
                    conn.scheme().to_ascii_lowercase(),
                    conn.host().to_ascii_lowercase()
                )
            };
            let headers = to_http_headers(req.headers(), CorsMiddlewareErrorKind::InvalidRequest)
                .map_err(|error| (config.map_error)(error))?;
            let decision = crate::shared::cors::evaluate(
                req.method().as_str(),
                &headers,
                Some(&request_origin),
                &policy,
                &config.settings,
            )
            .map_err(|error| (config.map_error)(error))?;
            match decision {
                CorsDecision::Pass => Ok(svc.call(req).await?.map_into_left_body()),
                CorsDecision::Reject => Ok(forbidden(req).map_into_right_body()),
                CorsDecision::Preflight(headers) => {
                    let mut response = HttpResponse::NoContent().finish();
                    replace_headers(response.headers_mut(), &headers)
                        .map_err(|error| (config.map_error)(error))?;
                    Ok(req.into_response(response).map_into_right_body())
                }
                CorsDecision::Actual(origin) => {
                    let mut response = svc.call(req).await?.map_into_left_body();
                    let mut headers = to_http_headers(
                        response.headers(),
                        CorsMiddlewareErrorKind::InvalidResponse,
                    )
                    .map_err(|error| (config.map_error)(error))?;
                    crate::shared::cors::apply_origin_headers(&mut headers, &policy, &origin)
                        .and_then(|()| {
                            crate::shared::cors::apply_actual_headers(
                                &mut headers,
                                &config.settings,
                            )
                        })
                        .map_err(|error| (config.map_error)(error))?;
                    replace_headers(response.headers_mut(), &headers)
                        .map_err(|error| (config.map_error)(error))?;
                    Ok(response)
                }
            }
        })
    }
}

// Actix uses http 0.2; the shared engine and Axum use http 1.
fn to_http_headers(
    headers: &HeaderMap,
    kind: CorsMiddlewareErrorKind,
) -> Result<http::HeaderMap, CorsMiddlewareError> {
    let mut result = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|_| CorsMiddlewareError::new(kind, "invalid header name"))?;
        let value = http::HeaderValue::from_bytes(value.as_bytes())
            .map_err(|_| CorsMiddlewareError::new(kind, "invalid header value"))?;
        result.append(name, value);
    }
    Ok(result)
}

fn replace_headers(
    headers: &mut HeaderMap,
    values: &http::HeaderMap,
) -> Result<(), CorsMiddlewareError> {
    let mut result = HeaderMap::new();
    for (name, value) in values {
        let name = header::HeaderName::from_bytes(name.as_str().as_bytes()).map_err(|_| {
            CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidResponse,
                "invalid header name",
            )
        })?;
        let value = HeaderValue::from_bytes(value.as_bytes()).map_err(|_| {
            CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidResponse,
                "invalid header value",
            )
        })?;
        result.append(name, value);
    }
    *headers = result;
    Ok(())
}

fn forbidden(req: ServiceRequest) -> ServiceResponse {
    let mut response = HttpResponse::Forbidden().finish();
    response.headers_mut().insert(
        header::VARY,
        HeaderValue::from_static(
            "Access-Control-Request-Headers, Access-Control-Request-Method, Origin",
        ),
    );
    req.into_response(response)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use actix_web::{
        App, HttpResponse,
        http::{
            StatusCode,
            header::{self, HeaderValue},
        },
        test, web,
    };

    use super::{
        CorsAllowedOrigins, CorsMiddlewareErrorKind, RuntimeCors, RuntimeCorsConfig,
        RuntimeCorsPolicy,
    };

    fn test_config() -> RuntimeCorsConfig {
        RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::List(vec![
                        "https://panel.example.com".to_string(),
                    ]),
                    allow_credentials: true,
                    max_age_secs: 600,
                })
            },
            |path| path == "/",
            |error| actix_web::error::ErrorBadRequest(error.to_string()),
        )
        .allowed_methods(["GET", "POST", "OPTIONS"])
        .allowed_headers([
            "authorization",
            "content-type",
            "x-csrf-token",
            "x-request-id",
        ])
        .exposed_headers(["content-length", "x-request-id"])
    }

    #[actix_web::test]
    async fn shared_header_adapter_preserves_multiple_vary_and_cookie_values() {
        let app = test::init_service(App::new().wrap(RuntimeCors::new(test_config())).route(
            "/api/demo",
            web::get().to(|| async {
                HttpResponse::Ok()
                    .append_header((header::VARY, "Accept-Encoding, origin"))
                    .append_header((header::VARY, "Accept-Language"))
                    .append_header((header::SET_COOKIE, "a=1"))
                    .append_header((header::SET_COOKIE, "b=2"))
                    .finish()
            }),
        ))
        .await;
        let response = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/demo")
                .insert_header((header::ORIGIN, "https://panel.example.com"))
                .to_request(),
        )
        .await;
        let vary = response
            .headers()
            .get(header::VARY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(vary.contains("Accept-Encoding"));
        assert!(vary.contains("Accept-Language"));
        assert_eq!(vary.to_ascii_lowercase().matches("origin").count(), 1);
        assert_eq!(
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .map(|value| value.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["a=1", "b=2"]
        );
    }

    #[actix_web::test]
    async fn cors_middleware_allows_configured_preflight() {
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(test_config()))
                .route("/api/demo", web::post().to(HttpResponse::Ok)),
        )
        .await;

        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/api/demo")
            .insert_header((header::ORIGIN, "https://panel.example.com"))
            .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "POST"))
            .insert_header((
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "content-type, x-csrf-token",
            ))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 204);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://panel.example.com"))
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&HeaderValue::from_static("true"))
        );
    }

    #[actix_web::test]
    async fn cors_middleware_matches_configured_allowed_headers_case_insensitively() {
        let config = RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::List(vec![
                        "https://panel.example.com".to_string(),
                    ]),
                    allow_credentials: true,
                    max_age_secs: 600,
                })
            },
            |path| path == "/",
            |error| actix_web::error::ErrorBadRequest(error.to_string()),
        )
        .allowed_methods(["GET", "POST", "OPTIONS"])
        .allowed_headers(["Content-Type", "X-CSRF-Token"]);
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(config))
                .route("/api/demo", web::post().to(HttpResponse::Ok)),
        )
        .await;

        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/api/demo")
            .insert_header((header::ORIGIN, "https://panel.example.com"))
            .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "POST"))
            .insert_header((
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "content-type, x-csrf-token",
            ))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 204);
    }

    #[actix_web::test]
    async fn cors_middleware_rejects_disallowed_origin() {
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(test_config()))
                .route("/api/demo", web::post().to(HttpResponse::Ok)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/demo")
            .insert_header((header::ORIGIN, "https://evil.example.com"))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 403);
        assert!(response.headers().contains_key(header::VARY));
    }

    #[actix_web::test]
    async fn cors_middleware_does_not_parse_origins_when_policy_is_inactive() {
        let config = RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: false,
                    allowed_origins: CorsAllowedOrigins::None,
                    allow_credentials: false,
                    max_age_secs: 60,
                })
            },
            |_| false,
            |error| actix_web::error::ErrorBadRequest(error.to_string()),
        );
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(config))
                .route("/api/demo", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/demo")
            .insert_header((
                header::ORIGIN,
                "chrome-extension://iikmkjmpaadaobahmlepeloendndfphd",
            ))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 200);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[actix_web::test]
    async fn cors_middleware_accepts_configured_additional_origin_scheme() {
        const EXTENSION_ORIGIN: &str = "chrome-extension://iikmkjmpaadaobahmlepeloendndfphd";

        let config = RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::List(vec![EXTENSION_ORIGIN.to_string()]),
                    allow_credentials: true,
                    max_age_secs: 60,
                })
            },
            |_| false,
            |error| actix_web::error::ErrorBadRequest(error.to_string()),
        )
        .additional_origin_schemes(["chrome-extension"])
        .allowed_methods(["GET", "OPTIONS"])
        .allowed_headers(["authorization"]);
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(config))
                .route("/api/demo", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/api/demo")
            .insert_header((header::ORIGIN, EXTENSION_ORIGIN))
            .insert_header((header::ACCESS_CONTROL_REQUEST_METHOD, "GET"))
            .insert_header((header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization"))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 204);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static(EXTENSION_ORIGIN))
        );
    }

    #[actix_web::test]
    async fn cors_middleware_uses_runtime_policy_resolver() {
        let origins = Arc::new(Mutex::new(vec!["https://one.example.com".to_string()]));
        let config = RuntimeCorsConfig::new(
            {
                let origins = Arc::clone(&origins);
                move |_req| {
                    Ok(RuntimeCorsPolicy {
                        enabled: true,
                        allowed_origins: CorsAllowedOrigins::List(
                            origins.lock().expect("origins lock").clone(),
                        ),
                        allow_credentials: false,
                        max_age_secs: 60,
                    })
                }
            },
            |_| false,
            |error| actix_web::error::ErrorBadRequest(error.to_string()),
        )
        .allowed_methods(["GET"])
        .allowed_headers(["authorization"])
        .exposed_headers(["x-request-id"]);

        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(config))
                .route("/api/demo", web::get().to(HttpResponse::Ok)),
        )
        .await;

        *origins.lock().expect("origins lock") = vec!["https://two.example.com".to_string()];
        let req = test::TestRequest::get()
            .uri("/api/demo")
            .insert_header((header::ORIGIN, "https://two.example.com"))
            .to_request();
        let response = test::call_service(&app, req).await;

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://two.example.com"))
        );
    }

    #[actix_web::test]
    async fn cors_middleware_classifies_invalid_request_headers() {
        let kinds = Arc::new(Mutex::new(Vec::new()));
        let config = RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::Any,
                    allow_credentials: false,
                    max_age_secs: 60,
                })
            },
            |_| false,
            {
                let kinds = Arc::clone(&kinds);
                move |error| {
                    kinds.lock().expect("kinds lock").push(error.kind());
                    actix_web::error::ErrorBadRequest(error.to_string())
                }
            },
        );
        let app = test::init_service(
            App::new()
                .wrap(RuntimeCors::new(config))
                .route("/api/demo", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let invalid_origin = HeaderValue::from_bytes(&[0xff]).expect("opaque header value");
        let req = test::TestRequest::get()
            .uri("/api/demo")
            .insert_header((header::ORIGIN, invalid_origin))
            .to_request();
        let error = test::try_call_service(&app, req)
            .await
            .expect_err("invalid request header should return a service error");

        assert_eq!(
            error.as_response_error().status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            *kinds.lock().expect("kinds lock"),
            vec![CorsMiddlewareErrorKind::InvalidRequest]
        );
    }

    #[actix_web::test]
    async fn cors_middleware_classifies_invalid_response_headers() {
        let kinds = Arc::new(Mutex::new(Vec::new()));
        let config = RuntimeCorsConfig::new(
            |_req| {
                Ok(RuntimeCorsPolicy {
                    enabled: true,
                    allowed_origins: CorsAllowedOrigins::Any,
                    allow_credentials: false,
                    max_age_secs: 60,
                })
            },
            |_| false,
            {
                let kinds = Arc::clone(&kinds);
                move |error| {
                    kinds.lock().expect("kinds lock").push(error.kind());
                    actix_web::error::ErrorInternalServerError(error.to_string())
                }
            },
        );
        let app = test::init_service(App::new().wrap(RuntimeCors::new(config)).route(
            "/api/demo",
            web::get().to(|| async {
                HttpResponse::Ok()
                    .insert_header((
                        header::VARY,
                        HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
                    ))
                    .finish()
            }),
        ))
        .await;

        let req = test::TestRequest::get()
            .uri("/api/demo")
            .insert_header((header::ORIGIN, "https://panel.example.com"))
            .to_request();
        let error = test::try_call_service(&app, req)
            .await
            .expect_err("invalid response header should return a service error");

        assert_eq!(
            error.as_response_error().status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            *kinds.lock().expect("kinds lock"),
            vec![CorsMiddlewareErrorKind::InvalidResponse]
        );
    }
}

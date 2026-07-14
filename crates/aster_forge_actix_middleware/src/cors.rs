//! Runtime CORS middleware for Actix Web services.
//!
//! Aster products often store CORS settings in runtime configuration rather than in a static Actix
//! builder. This module owns the reusable middleware mechanics: reading `Origin`, rejecting
//! disallowed cross-origin requests, handling preflight requests, applying CORS response headers,
//! and maintaining `Vary`. Product crates provide a policy resolver, exempt-path predicate,
//! allowed/exposed header lists, and error mapping.

use std::collections::BTreeSet;
use std::rc::Rc;

use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::{
        Method, header,
        header::{HeaderMap, HeaderValue},
    },
};
use futures::future::{LocalBoxFuture, Ready, ok};

/// Origin list accepted by a runtime CORS policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsAllowedOrigins {
    /// Cross-origin requests are denied.
    None,
    /// Every origin is accepted.
    Any,
    /// Only the listed normalized origins are accepted.
    List(Vec<String>),
}

/// Product-neutral runtime CORS policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCorsPolicy {
    /// Whether CORS processing is enabled.
    pub enabled: bool,
    /// Origins accepted when CORS processing is enabled.
    pub allowed_origins: CorsAllowedOrigins,
    /// Whether credentials are allowed.
    pub allow_credentials: bool,
    /// Browser preflight cache duration.
    pub max_age_secs: u64,
}

impl RuntimeCorsPolicy {
    /// Returns whether requests should be actively checked.
    pub fn enforces_requests(&self) -> bool {
        self.enabled && !matches!(self.allowed_origins, CorsAllowedOrigins::None)
    }

    /// Returns whether a normalized origin is allowed.
    pub fn allows_origin(&self, origin: &str) -> bool {
        match &self.allowed_origins {
            CorsAllowedOrigins::None => false,
            CorsAllowedOrigins::Any => true,
            CorsAllowedOrigins::List(origins) => origins.iter().any(|allowed| allowed == origin),
        }
    }

    /// Returns whether responses should use `Access-Control-Allow-Origin: *`.
    pub fn sends_wildcard_origin(&self) -> bool {
        matches!(self.allowed_origins, CorsAllowedOrigins::Any) && !self.allow_credentials
    }
}

/// CORS middleware failure category for product error mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsMiddlewareErrorKind {
    /// The incoming request contains an invalid origin or preflight header.
    InvalidRequest,
    /// A response header produced or inherited by the middleware is invalid.
    InvalidResponse,
}

/// Product-neutral CORS middleware error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CorsMiddlewareError {
    kind: CorsMiddlewareErrorKind,
    message: String,
}

impl CorsMiddlewareError {
    fn new(kind: CorsMiddlewareErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the failure category.
    pub const fn kind(&self) -> CorsMiddlewareErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

type PolicyResolver = dyn Fn(&ServiceRequest) -> Result<RuntimeCorsPolicy, Error>;
type ExemptPathPredicate = dyn Fn(&str) -> bool;
type ErrorMapper = dyn Fn(CorsMiddlewareError) -> Error;

/// Runtime CORS middleware configuration.
pub struct RuntimeCorsConfig {
    allowed_methods: Vec<&'static str>,
    allowed_headers: Vec<&'static str>,
    exposed_headers: Vec<&'static str>,
    additional_origin_schemes: Vec<&'static str>,
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
            allowed_methods: Vec::new(),
            allowed_headers: Vec::new(),
            exposed_headers: Vec::new(),
            additional_origin_schemes: Vec::new(),
            policy: Rc::new(policy),
            exempt_path: Rc::new(exempt_path),
            map_error: Rc::new(map_error),
        }
    }

    /// Sets preflight-allowed methods.
    pub fn allowed_methods(mut self, methods: impl IntoIterator<Item = &'static str>) -> Self {
        self.allowed_methods = methods.into_iter().collect();
        self
    }

    /// Sets preflight-allowed request headers.
    pub fn allowed_headers(mut self, headers: impl IntoIterator<Item = &'static str>) -> Self {
        self.allowed_headers = headers.into_iter().collect();
        self
    }

    /// Sets response headers exposed to browser JavaScript.
    pub fn exposed_headers(mut self, headers: impl IntoIterator<Item = &'static str>) -> Self {
        self.exposed_headers = headers.into_iter().collect();
        self
    }

    /// Accepts selected non-HTTP schemes while parsing request origins.
    ///
    /// This does not authorize a scheme by itself. The normalized full origin must still match
    /// [`RuntimeCorsPolicy::allowed_origins`].
    pub fn additional_origin_schemes(
        mut self,
        schemes: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        self.additional_origin_schemes = schemes.into_iter().collect();
        self
    }
}

impl Clone for RuntimeCorsConfig {
    fn clone(&self) -> Self {
        Self {
            allowed_methods: self.allowed_methods.clone(),
            allowed_headers: self.allowed_headers.clone(),
            exposed_headers: self.exposed_headers.clone(),
            additional_origin_schemes: self.additional_origin_schemes.clone(),
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

            let Some(origin_header) = req.headers().get(header::ORIGIN).cloned() else {
                return Ok(svc.call(req).await?.map_into_left_body());
            };

            let policy = (config.policy)(&req)?;

            if !policy.enforces_requests() {
                return Ok(svc.call(req).await?.map_into_left_body());
            }

            let origin = origin_header
                .to_str()
                .map_err(|_| {
                    (config.map_error)(CorsMiddlewareError::new(
                        CorsMiddlewareErrorKind::InvalidRequest,
                        "invalid Origin header",
                    ))
                })
                .and_then(|origin| {
                    aster_forge_utils::url::normalize_origin_with_additional_schemes(
                        origin,
                        false,
                        &config.additional_origin_schemes,
                    )
                    .map_err(|error| {
                        (config.map_error)(CorsMiddlewareError::new(
                            CorsMiddlewareErrorKind::InvalidRequest,
                            error.to_string(),
                        ))
                    })
                })?;

            if request_is_same_origin(&req, &origin) {
                return Ok(svc.call(req).await?.map_into_left_body());
            }

            if !policy.allows_origin(&origin) {
                return Ok(forbidden(req).map_into_right_body());
            }

            if is_preflight_request(&req) {
                if !requested_method_is_allowed(&req, &config)
                    || !requested_headers_are_allowed(&req, &config, &config.map_error)?
                {
                    return Ok(forbidden(req).map_into_right_body());
                }

                let mut response = HttpResponse::NoContent().finish();
                apply_origin_headers(response.headers_mut(), &policy, &origin, &config.map_error)?;
                apply_preflight_headers(
                    response.headers_mut(),
                    &policy,
                    &config,
                    &config.map_error,
                )?;
                return Ok(req.into_response(response).map_into_right_body());
            }

            let mut response = svc.call(req).await?.map_into_left_body();
            apply_origin_headers(response.headers_mut(), &policy, &origin, &config.map_error)?;
            apply_actual_headers(response.headers_mut(), &config, &config.map_error)?;
            Ok(response)
        })
    }
}

fn is_preflight_request(req: &ServiceRequest) -> bool {
    req.method() == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

fn request_is_same_origin(req: &ServiceRequest, origin: &str) -> bool {
    let conn = req.connection_info();
    let request_origin = format!(
        "{}://{}",
        conn.scheme().to_ascii_lowercase(),
        conn.host().to_ascii_lowercase()
    );
    request_origin == origin
}

fn requested_method_is_allowed(req: &ServiceRequest, config: &RuntimeCorsConfig) -> bool {
    let Some(method) = req.headers().get(header::ACCESS_CONTROL_REQUEST_METHOD) else {
        return false;
    };

    let Ok(method) = method.to_str() else {
        return false;
    };

    config.allowed_methods.contains(&method)
}

fn requested_headers_are_allowed(
    req: &ServiceRequest,
    config: &RuntimeCorsConfig,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<bool, Error> {
    let Some(request_headers) = req.headers().get(header::ACCESS_CONTROL_REQUEST_HEADERS) else {
        return Ok(true);
    };

    let request_headers = request_headers.to_str().map_err(|_| {
        map_error(CorsMiddlewareError::new(
            CorsMiddlewareErrorKind::InvalidRequest,
            "invalid Access-Control-Request-Headers",
        ))
    })?;

    let allowed_headers = config
        .allowed_headers
        .iter()
        .copied()
        .collect::<BTreeSet<&'static str>>();

    for requested in request_headers.split(',') {
        let requested = requested.trim().to_ascii_lowercase();
        if requested.is_empty() {
            continue;
        }

        let parsed: Result<header::HeaderName, _> = requested.parse();
        if parsed.is_err() {
            return Err(map_error(CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidRequest,
                "invalid Access-Control-Request-Headers",
            )));
        }

        if !allowed_headers.contains(requested.as_str()) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn apply_origin_headers(
    headers: &mut HeaderMap,
    policy: &RuntimeCorsPolicy,
    origin: &str,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<(), Error> {
    if !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN) {
        let value = if policy.sends_wildcard_origin() {
            HeaderValue::from_static("*")
        } else {
            header_value(
                origin,
                "failed to serialize Access-Control-Allow-Origin",
                map_error,
            )?
        };

        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }

    if policy.allow_credentials && !headers.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS) {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }

    ensure_vary(headers, "Origin", map_error)?;
    Ok(())
}

fn apply_preflight_headers(
    headers: &mut HeaderMap,
    policy: &RuntimeCorsPolicy,
    config: &RuntimeCorsConfig,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<(), Error> {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header_value(
            &config.allowed_methods.join(", "),
            "failed to serialize Access-Control-Allow-Methods",
            map_error,
        )?,
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header_value(
            &config.allowed_headers.join(", "),
            "failed to serialize Access-Control-Allow-Headers",
            map_error,
        )?,
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        header_value(
            &policy.max_age_secs.to_string(),
            "failed to serialize Access-Control-Max-Age",
            map_error,
        )?,
    );
    ensure_vary(headers, "Access-Control-Request-Method", map_error)?;
    ensure_vary(headers, "Access-Control-Request-Headers", map_error)?;
    Ok(())
}

fn apply_actual_headers(
    headers: &mut HeaderMap,
    config: &RuntimeCorsConfig,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<(), Error> {
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        header_value(
            &config.exposed_headers.join(", "),
            "failed to serialize Access-Control-Expose-Headers",
            map_error,
        )?,
    );
    Ok(())
}

fn ensure_vary(
    headers: &mut HeaderMap,
    value: &str,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<(), Error> {
    let mut vary_values = BTreeSet::new();

    if let Some(existing) = headers.get(header::VARY) {
        let existing = existing.to_str().map_err(|_| {
            map_error(CorsMiddlewareError::new(
                CorsMiddlewareErrorKind::InvalidResponse,
                "invalid Vary header",
            ))
        })?;
        for item in existing.split(',') {
            let item = item.trim();
            if !item.is_empty() {
                vary_values.insert(item.to_string());
            }
        }
    }

    vary_values.insert(value.to_string());
    let joined = vary_values.into_iter().collect::<Vec<_>>().join(", ");
    let header_value = header_value(&joined, "failed to serialize Vary header", map_error)?;
    headers.insert(header::VARY, header_value);
    Ok(())
}

fn header_value(
    value: &str,
    error_message: &'static str,
    map_error: &Rc<dyn Fn(CorsMiddlewareError) -> Error>,
) -> Result<HeaderValue, Error> {
    HeaderValue::from_str(value).map_err(|_| {
        map_error(CorsMiddlewareError::new(
            CorsMiddlewareErrorKind::InvalidResponse,
            error_message,
        ))
    })
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

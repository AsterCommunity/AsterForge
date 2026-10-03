//! Actix composition based on `AsterDrive`'s app-data, policy resolver and response adapters.
use actix_governor::Governor;
use actix_web::{
    App, Error, HttpResponse, HttpServer,
    dev::{Service, ServiceRequest},
    web,
};
use aster_forge_middleware::actix::{
    cors::{
        CorsAllowedOrigins, CorsMiddlewareError, CorsMiddlewareErrorKind, RuntimeCors,
        RuntimeCorsConfig, RuntimeCorsPolicy,
    },
    csrf::{self, CsrfTokenNames, RequestSourceMode},
    rate_limit::build_ip_governor_config_with_rejection_response,
    request_id::RequestIdMiddleware,
    security_headers::default_headers,
};
use futures::{
    FutureExt,
    future::{Either, ready},
};
use std::{
    io,
    num::{NonZeroU32, NonZeroU64},
    sync::RwLock,
};

struct AppState {
    cors: RwLock<RuntimeCorsPolicy>,
    csrf_names: CsrfTokenNames,
    public_origins: Vec<String>,
}

fn cors_error(error: &CorsMiddlewareError) -> Error {
    match error.kind() {
        CorsMiddlewareErrorKind::InvalidRequest => {
            actix_web::error::ErrorBadRequest(error.to_string())
        }
        CorsMiddlewareErrorKind::InvalidResponse => {
            actix_web::error::ErrorInternalServerError(error.to_string())
        }
    }
}

fn runtime_cors() -> RuntimeCors {
    RuntimeCors::new(
        RuntimeCorsConfig::new(
            |req| {
                let state = req.app_data::<web::Data<AppState>>().ok_or_else(|| {
                    actix_web::error::ErrorInternalServerError("missing example state")
                })?;
                state
                    .cors
                    .read()
                    .map(|policy| policy.clone())
                    .map_err(|_| actix_web::error::ErrorInternalServerError("policy unavailable"))
            },
            |path| path == "/health",
            |error| cors_error(&error),
        )
        .allowed_methods(["GET", "POST", "OPTIONS"])
        .allowed_headers(["content-type", "x-example-csrf"])
        .exposed_headers(["x-request-id"]),
    )
}

fn protect_write(request: &ServiceRequest) -> Result<(), Error> {
    if !csrf::is_unsafe_method(request.method()) {
        return Ok(());
    }
    let state = request
        .app_data::<web::Data<AppState>>()
        .ok_or_else(|| actix_web::error::ErrorInternalServerError("missing example state"))?;
    csrf::ensure_service_request_source_allowed(
        request,
        &state.public_origins,
        RequestSourceMode::Required,
    )
    .and_then(|()| csrf::ensure_service_double_submit_token_with_names(request, &state.csrf_names))
    .map_err(|error| actix_web::error::ErrorForbidden(format!("CSRF: {:?}", error.kind())))
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    let state = web::Data::new(AppState {
        cors: RwLock::new(RuntimeCorsPolicy {
            enabled: true,
            allowed_origins: CorsAllowedOrigins::List(vec!["http://localhost:5173".into()]),
            allow_credentials: true,
            max_age_secs: 600,
        }),
        csrf_names: CsrfTokenNames::new("example_csrf", "X-Example-CSRF")
            .map_err(io::Error::other)?,
        public_origins: vec!["http://localhost:5173".into()],
    });
    let governor = build_ip_governor_config_with_rejection_response(
        NonZeroU64::new(1).ok_or_else(|| io::Error::other("invalid period"))?,
        NonZeroU32::new(20).ok_or_else(|| io::Error::other("invalid burst"))?,
        &[],
        |wait, mut response| {
            response
                .insert_header(("retry-after", wait.to_string()))
                .json(serde_json::json!({"code": "example_limit", "retry_after": wait}))
        },
    );
    HttpServer::new(move || {
        let app = App::new()
            .app_data(state.clone())
            // A product installs this scope after deciding its cookie-authentication boundary.
            .service(
                web::scope("/api")
                    .wrap_fn(|request, service| match protect_write(&request) {
                        Ok(()) => Either::Left(service.call(request).map(|result| {
                            result.map(actix_web::dev::ServiceResponse::map_into_left_body)
                        })),
                        Err(error) => Either::Right(ready(Ok(request
                            .into_response(error.error_response())
                            .map_into_right_body()))),
                    })
                    .route(
                        "/demo",
                        web::get().to(|| async { HttpResponse::Ok().body("ok") }),
                    )
                    .route(
                        "/demo",
                        web::post().to(|| async { HttpResponse::Ok().body("updated") }),
                    ),
            )
            .route(
                "/health",
                web::get().to(|| async { HttpResponse::Ok().body("healthy") }),
            )
            .wrap(Governor::new(&governor))
            .wrap(runtime_cors())
            .wrap(default_headers())
            .wrap(RequestIdMiddleware);
        #[cfg(feature = "metrics")]
        let app = app
            .app_data(web::Data::<dyn aster_forge_metrics::MetricsRecorder>::from(
                aster_forge_metrics::NoopMetrics::arc(),
            ))
            .wrap(aster_forge_middleware::actix::metrics::MetricsMiddleware);
        // This direct-listener example trusts no proxy. Clear origin-affecting proxy headers
        // before connection_info is first read; real products decide trust from the direct peer.
        app.wrap_fn(|mut request, service| {
            for name in ["forwarded", "x-forwarded-host", "x-forwarded-proto"] {
                request.headers_mut().remove(name);
            }
            service.call(request)
        })
    })
    .bind(("127.0.0.1", 3000))?
    .run()
    .await
}

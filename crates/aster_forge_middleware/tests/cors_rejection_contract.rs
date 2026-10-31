//! Public CORS response-factory contracts, including transport parity.
#![cfg(any(feature = "actix", feature = "axum"))]

use aster_forge_middleware::shared::cors::{
    CorsAllowedOrigins, CorsMiddlewareErrorKind, CorsRejection, RuntimeCorsPolicy,
};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Handler,
    Preflight,
    Rejected(CorsRejection),
    Error(CorsMiddlewareErrorKind),
}

#[derive(Clone, Copy, Debug)]
enum Vary {
    Multiple,
    Wildcard,
    Opaque,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    method: &'static str,
    path: &'static str,
    origin: Option<&'static str>,
    requested_method: Option<&'static str>,
    requested_headers: Option<&'static str>,
    vary: Vary,
    custom: bool,
    outcome: Outcome,
}

fn cases() -> Vec<Case> {
    let actual = Case {
        method: "GET",
        path: "/",
        origin: Some("https://denied.test"),
        requested_method: None,
        requested_headers: None,
        vary: Vary::Multiple,
        custom: true,
        outcome: Outcome::Rejected(CorsRejection::OriginNotAllowed),
    };
    let preflight = Case {
        method: "OPTIONS",
        origin: Some("https://allowed.test"),
        requested_method: Some("POST"),
        requested_headers: Some("content-type"),
        outcome: Outcome::Preflight,
        ..actual
    };
    let method_denied = Case {
        requested_method: Some("DELETE"),
        outcome: Outcome::Rejected(CorsRejection::MethodNotAllowed),
        ..preflight
    };
    let headers_denied = Case {
        requested_headers: Some("x-denied"),
        outcome: Outcome::Rejected(CorsRejection::HeadersNotAllowed),
        ..preflight
    };
    vec![
        actual,
        Case {
            origin: actual.origin,
            outcome: actual.outcome,
            ..preflight
        },
        method_denied,
        headers_denied,
        Case {
            custom: false,
            ..actual
        },
        Case {
            custom: false,
            ..method_denied
        },
        Case {
            custom: false,
            ..headers_denied
        },
        Case {
            vary: Vary::Wildcard,
            ..actual
        },
        Case {
            vary: Vary::Opaque,
            outcome: Outcome::Error(CorsMiddlewareErrorKind::InvalidResponse),
            ..actual
        },
        Case {
            origin: Some("null"),
            outcome: Outcome::Error(CorsMiddlewareErrorKind::InvalidRequest),
            ..actual
        },
        Case {
            requested_headers: Some("bad name"),
            outcome: Outcome::Error(CorsMiddlewareErrorKind::InvalidRequest),
            ..preflight
        },
        Case {
            origin: Some("https://allowed.test"),
            outcome: Outcome::Handler,
            ..actual
        },
        Case {
            origin: Some("http://api.test"),
            outcome: Outcome::Handler,
            ..actual
        },
        Case {
            origin: None,
            outcome: Outcome::Handler,
            ..actual
        },
        Case {
            path: "/exempt",
            outcome: Outcome::Handler,
            ..actual
        },
        Case {
            path: "/disabled",
            outcome: Outcome::Handler,
            ..actual
        },
        Case {
            path: "/none",
            outcome: Outcome::Handler,
            ..actual
        },
        preflight,
    ]
}

fn policy(path: &str) -> RuntimeCorsPolicy {
    RuntimeCorsPolicy {
        enabled: path != "/disabled",
        allowed_origins: if path == "/none" {
            CorsAllowedOrigins::None
        } else {
            CorsAllowedOrigins::List(vec!["https://allowed.test".into()])
        },
        allow_credentials: false,
        max_age_secs: 600,
    }
}

#[derive(Default)]
struct Calls {
    rejections: Mutex<Vec<CorsRejection>>,
    errors: Mutex<Vec<CorsMiddlewareErrorKind>>,
    handlers: AtomicUsize,
}

fn grant_headers() -> [http::HeaderName; 6] {
    [
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        header::ACCESS_CONTROL_MAX_AGE,
    ]
}

fn product_headers(vary: Vary) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (header::CONTENT_TYPE, "application/json"),
        (header::CACHE_CONTROL, "no-store"),
        (header::RETRY_AFTER, "7"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.append(header::SET_COOKIE, HeaderValue::from_static("a=1"));
    headers.append(header::SET_COOKIE, HeaderValue::from_static("b=2"));
    headers.append(
        header::VARY,
        HeaderValue::from_static("Accept-Language, origin"),
    );
    headers.append(
        header::VARY,
        match vary {
            Vary::Multiple => HeaderValue::from_static("Accept-Encoding, ORIGIN"),
            Vary::Wildcard => HeaderValue::from_static("*"),
            Vary::Opaque => HeaderValue::from_bytes(&[0xff]).unwrap(),
        },
    );
    for name in grant_headers() {
        headers.append(name.clone(), HeaderValue::from_static("stale"));
        headers.append(name, HeaderValue::from_static("stale-again"));
    }
    headers
}

struct Wire {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

fn assert_wire(case: Case, calls: &Calls, wire: &Wire) {
    let rejections = calls.rejections.lock().unwrap();
    let errors = calls.errors.lock().unwrap();
    assert_eq!(
        calls.handlers.load(Ordering::SeqCst),
        usize::from(matches!(case.outcome, Outcome::Handler)),
        "{case:?}"
    );
    match case.outcome {
        Outcome::Handler | Outcome::Preflight => {
            // Handler deliberately returns an empty 403: rejection must never be guessed from it.
            assert_eq!(
                wire.status,
                if matches!(case.outcome, Outcome::Handler) {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NO_CONTENT
                }
            );
            assert!(wire.body.is_empty());
            assert!(rejections.is_empty());
            assert!(errors.is_empty());
            if case.origin == Some("https://allowed.test") {
                assert_eq!(
                    wire.headers[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                    "https://allowed.test"
                );
            }
        }
        Outcome::Rejected(reason) => {
            assert!(errors.is_empty());
            if case.custom {
                assert_eq!(*rejections, vec![reason]);
                assert_eq!(wire.status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
                    serde_json::json!({"code":"product.cors_denied"})
                );
                for (name, value) in [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::CACHE_CONTROL, "no-store"),
                    (header::RETRY_AFTER, "7"),
                ] {
                    assert_eq!(wire.headers[name], value);
                }
                assert_eq!(wire.headers.get_all(header::SET_COOKIE).iter().count(), 2);
            } else {
                assert!(rejections.is_empty());
                assert_eq!(wire.status, StatusCode::FORBIDDEN);
                assert!(wire.body.is_empty());
            }
            for name in grant_headers() {
                assert!(!wire.headers.contains_key(name));
            }
            let values = wire
                .headers
                .get_all(header::VARY)
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>()
                .join(",");
            let vary = values
                .split(',')
                .map(|name| name.trim().to_ascii_lowercase())
                .collect::<Vec<_>>();
            if matches!(case.vary, Vary::Wildcard) {
                assert!(vary.iter().any(|name| name == "*"));
            } else {
                for name in [
                    "origin",
                    "access-control-request-method",
                    "access-control-request-headers",
                ] {
                    assert_eq!(vary.iter().filter(|value| *value == name).count(), 1);
                }
                if case.custom {
                    for name in ["accept-language", "accept-encoding"] {
                        assert!(vary.iter().any(|value| value == name));
                    }
                }
            }
        }
        Outcome::Error(kind) => {
            assert_eq!(*errors, vec![kind]);
            let status = match kind {
                CorsMiddlewareErrorKind::InvalidRequest => {
                    assert!(rejections.is_empty());
                    StatusCode::BAD_REQUEST
                }
                CorsMiddlewareErrorKind::InvalidResponse => {
                    assert_eq!(*rejections, vec![CorsRejection::OriginNotAllowed]);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            assert_eq!(wire.status, status);
            assert_eq!(wire.body, format!("{kind:?}").as_bytes());
        }
    }
}

#[cfg(feature = "axum")]
async fn axum_wire(case: Case) -> Wire {
    use aster_forge_middleware::axum::cors::{RuntimeCorsConfig, runtime_cors};
    use axum::{
        Router,
        body::{Body, to_bytes},
        middleware,
        response::IntoResponse,
        routing::any,
    };
    use tower::ServiceExt;
    let calls = Arc::new(Calls::default());
    let errors = calls.clone();
    let mut config = RuntimeCorsConfig::new(
        |req| Ok(policy(req.uri().path())),
        |path| path == "/exempt",
        move |error| {
            errors.errors.lock().unwrap().push(error.kind());
            let status = match error.kind() {
                CorsMiddlewareErrorKind::InvalidRequest => StatusCode::BAD_REQUEST,
                CorsMiddlewareErrorKind::InvalidResponse => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, format!("{:?}", error.kind())).into_response()
        },
    )
    .request_origin(|_| Ok(Some("http://api.test".into())))
    .allowed_methods(["GET", "POST"])
    .allowed_headers(["content-type"]);
    if case.custom {
        let rejected = calls.clone();
        config = config.rejection_response(move |reason| {
            rejected.rejections.lock().unwrap().push(reason);
            let mut response = (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({"code":"product.cors_denied"})),
            )
                .into_response();
            *response.headers_mut() = product_headers(case.vary);
            response
        });
    }
    let handlers = calls.clone();
    let app = Router::new()
        .fallback(any(move || {
            handlers.handlers.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::FORBIDDEN }
        }))
        .layer(middleware::from_fn_with_state(config.clone(), runtime_cors));
    let mut request = http::Request::builder()
        .method(case.method)
        .uri(case.path)
        .header("host", "api.test");
    for (name, value) in [
        ("origin", case.origin),
        ("access-control-request-method", case.requested_method),
        ("access-control-request-headers", case.requested_headers),
    ] {
        if let Some(value) = value {
            request = request.header(name, value);
        }
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let wire = Wire {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 4096).await.unwrap().to_vec(),
    };
    assert_wire(case, &calls, &wire);
    wire
}

#[cfg(feature = "actix")]
async fn actix_wire(case: Case) -> Wire {
    use actix_web::{App, HttpResponse, test, web};
    use aster_forge_middleware::actix::cors::{RuntimeCors, RuntimeCorsConfig};
    let calls = Arc::new(Calls::default());
    let errors = calls.clone();
    let mut config = RuntimeCorsConfig::new(
        |req| Ok(policy(req.path())),
        |path| path == "/exempt",
        move |error| {
            errors.errors.lock().unwrap().push(error.kind());
            match error.kind() {
                CorsMiddlewareErrorKind::InvalidRequest => {
                    actix_web::error::ErrorBadRequest(format!("{:?}", error.kind()))
                }
                CorsMiddlewareErrorKind::InvalidResponse => {
                    actix_web::error::ErrorInternalServerError(format!("{:?}", error.kind()))
                }
            }
        },
    )
    .allowed_methods(["GET", "POST"])
    .allowed_headers(["content-type"]);
    if case.custom {
        let rejected = calls.clone();
        config = config.rejection_response(move |reason| {
            rejected.rejections.lock().unwrap().push(reason);
            let mut response = HttpResponse::UnprocessableEntity();
            for (name, value) in &product_headers(case.vary) {
                response.append_header((name.as_str(), value.as_bytes()));
            }
            response.body(r#"{"code":"product.cors_denied"}"#)
        });
    }
    let handlers = calls.clone();
    let app = test::init_service(
        App::new()
            .wrap(RuntimeCors::new(config.clone()))
            .default_service(web::to(move || {
                handlers.handlers.fetch_add(1, Ordering::SeqCst);
                async { HttpResponse::Forbidden().finish() }
            })),
    )
    .await;
    let mut request = test::TestRequest::default()
        .method(case.method.parse().unwrap())
        .uri(case.path)
        .insert_header(("host", "api.test"));
    for (name, value) in [
        ("origin", case.origin),
        ("access-control-request-method", case.requested_method),
        ("access-control-request-headers", case.requested_headers),
    ] {
        if let Some(value) = value {
            request = request.insert_header((name, value));
        }
    }
    let response = match test::try_call_service(&app, request.to_request()).await {
        Ok(response) => response.map_into_boxed_body().into_parts().1,
        Err(error) => error.error_response(),
    };
    let mut headers = HeaderMap::new();
    for (name, value) in response.headers() {
        headers.append(
            http::HeaderName::from_bytes(name.as_str().as_bytes()).unwrap(),
            HeaderValue::from_bytes(value.as_bytes()).unwrap(),
        );
    }
    let wire = Wire {
        status: StatusCode::from_u16(response.status().as_u16()).unwrap(),
        headers,
        body: actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap()
            .to_vec(),
    };
    assert_wire(case, &calls, &wire);
    wire
}

#[cfg(feature = "axum")]
#[tokio::test]
async fn axum_rejection_response_contract() {
    for case in cases() {
        axum_wire(case).await;
    }
}

#[cfg(feature = "actix")]
#[actix_web::test]
async fn actix_rejection_response_contract() {
    for case in cases() {
        actix_wire(case).await;
    }
}

#[cfg(all(feature = "actix", feature = "axum"))]
#[actix_web::test]
async fn rejection_responses_match_across_transports() {
    for case in cases() {
        let left = actix_wire(case).await;
        let right = axum_wire(case).await;
        assert_eq!(left.status, right.status, "{case:?}");
        assert_eq!(left.body, right.body, "{case:?}");
        for name in [
            header::VARY,
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        ] {
            assert_eq!(
                left.headers.get_all(&name).iter().collect::<Vec<_>>(),
                right.headers.get_all(&name).iter().collect::<Vec<_>>(),
                "{case:?}"
            );
        }
    }
}

//! Default security response headers for Actix Web services.
//!
//! These headers are intentionally limited to generic browser hardening. HSTS
//! is not set here because Aster services usually sit behind an HTTPS reverse
//! proxy, and that proxy should own HTTPS termination policy.

use actix_web::middleware::DefaultHeaders;

/// Value used for the `X-Frame-Options` response header.
pub const X_FRAME_OPTIONS_VALUE: &str = "SAMEORIGIN";
/// Value used for the `Referrer-Policy` response header.
pub const REFERRER_POLICY_VALUE: &str = "strict-origin-when-cross-origin";
/// Value used for the `X-Content-Type-Options` response header.
pub const X_CONTENT_TYPE_OPTIONS_VALUE: &str = "nosniff";

/// Builds the default security headers middleware.
pub fn default_headers() -> DefaultHeaders {
    DefaultHeaders::new()
        .add(("X-Frame-Options", X_FRAME_OPTIONS_VALUE))
        .add(("Referrer-Policy", REFERRER_POLICY_VALUE))
        .add(("X-Content-Type-Options", X_CONTENT_TYPE_OPTIONS_VALUE))
}

#[cfg(test)]
mod tests {
    use super::{
        REFERRER_POLICY_VALUE, X_CONTENT_TYPE_OPTIONS_VALUE, X_FRAME_OPTIONS_VALUE, default_headers,
    };
    use actix_web::{HttpResponse, http::header, test, web};

    #[actix_web::test]
    async fn default_headers_adds_security_headers() {
        let app = test::init_service(
            actix_web::App::new()
                .wrap(default_headers())
                .route("/", web::get().to(HttpResponse::Ok)),
        )
        .await;

        let request = test::TestRequest::get().uri("/").to_request();
        let response = test::call_service(&app, request).await;

        assert_eq!(
            response.headers().get("x-frame-options"),
            Some(&header::HeaderValue::from_static(X_FRAME_OPTIONS_VALUE))
        );
        assert_eq!(
            response.headers().get("referrer-policy"),
            Some(&header::HeaderValue::from_static(REFERRER_POLICY_VALUE))
        );
        assert_eq!(
            response.headers().get("x-content-type-options"),
            Some(&header::HeaderValue::from_static(
                X_CONTENT_TYPE_OPTIONS_VALUE
            ))
        );
    }
}

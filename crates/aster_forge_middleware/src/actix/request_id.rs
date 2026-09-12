//! Request id middleware.
//!
//! The middleware assigns a UUID v4 to every request, stores it in request
//! extensions, adds it to the `X-Request-ID` response header, and instruments
//! the downstream service call with a tracing span containing request metadata.

use actix_web::{
    Error, HttpMessage,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::header::{HeaderName, HeaderValue},
};
use futures::future::{LocalBoxFuture, Ready, ok};
use std::rc::Rc;
use tracing::Instrument;

/// Request id value stored in Actix request extensions.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Actix middleware that creates and propagates request ids.
pub struct RequestIdMiddleware;

impl<S, B> Transform<S, ServiceRequest> for RequestIdMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = RequestIdService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RequestIdService {
            service: Rc::new(service),
        })
    }
}

/// Service wrapper installed by [`RequestIdMiddleware`].
pub struct RequestIdService<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for RequestIdService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let svc = self.service.clone();
        let request_id = uuid::Uuid::new_v4().to_string();
        let method = req.method().to_string();
        let path = req.path().to_string();

        req.extensions_mut().insert(RequestId(request_id.clone()));

        let span = tracing::info_span!(
            "request",
            request_id = %request_id,
            method = %method,
            path = %path,
            user_id = tracing::field::Empty,
        );

        Box::pin(
            async move {
                let mut resp = svc.call(req).await?;

                if let Ok(val) = HeaderValue::from_str(&request_id) {
                    resp.headers_mut()
                        .insert(HeaderName::from_static("x-request-id"), val);
                }

                Ok(resp)
            }
            .instrument(span),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RequestId, RequestIdMiddleware};
    use actix_web::{HttpMessage, HttpResponse, http::header, test, web};

    #[actix_web::test]
    async fn request_id_is_stored_and_returned() {
        let app = test::init_service(actix_web::App::new().wrap(RequestIdMiddleware).route(
            "/",
            web::get().to(|req: actix_web::HttpRequest| async move {
                let request_id = req
                    .extensions()
                    .get::<RequestId>()
                    .map(|value| value.0.clone())
                    .unwrap_or_default();
                HttpResponse::Ok().body(request_id)
            }),
        ))
        .await;

        let request = test::TestRequest::get().uri("/").to_request();
        let response = test::call_service(&app, request).await;
        let header_value = response
            .headers()
            .get(header::HeaderName::from_static("x-request-id"))
            .expect("request id header should be present")
            .to_str()
            .expect("request id should be ASCII")
            .to_string();

        let body = test::read_body(response).await;
        assert_eq!(body.as_ref(), header_value.as_bytes());
    }
}

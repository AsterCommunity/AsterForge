//! JWT authentication middleware.

use actix_web::{
    Error, HttpMessage,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    web,
};
use futures::future::{LocalBoxFuture, Ready, ok};
use std::rc::Rc;

use crate::api::middleware::csrf::{self, RequestSourceMode};
use crate::api::request_auth::access_cookie_token;
use crate::errors::AsterError;
use crate::runtime::{AppState, SharedRuntimeState};
use crate::services::auth_service::{self, AuthUserInfo};

pub struct JwtAuth;

impl<S, B> Transform<S, ServiceRequest> for JwtAuth
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = JwtAuthMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(JwtAuthMiddleware {
            service: Rc::new(service),
        })
    }
}

pub struct JwtAuthMiddleware<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for JwtAuthMiddleware<S>
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

        Box::pin(async move {
            let state = req
                .app_data::<web::Data<AppState>>()
                .ok_or_else(|| AsterError::internal_error("AppState not found"))?;
            if access_cookie_token(req.request()).is_some() && csrf::is_unsafe_method(req.method())
            {
                csrf::ensure_service_request_source_allowed(
                    &req,
                    state.get_ref().runtime_config(),
                    RequestSourceMode::OptionalWhenPresent,
                )?;
                csrf::ensure_service_double_submit_token(&req)?;
            }
            let user = auth_service::current_user(state.get_ref(), req.request()).await?;
            let user = AuthUserInfo::from(user);

            tracing::Span::current().record("user_id", user.id);
            req.extensions_mut().insert(user);

            svc.call(req).await
        })
    }
}

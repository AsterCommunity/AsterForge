//! System information API.

use crate::api::response::ApiResponse;
use crate::errors::Result;
use crate::runtime::AppState;
use actix_web::{HttpResponse, web};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct SystemInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub build_time: &'static str,
    pub site_title: String,
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::scope("/system").route("/info", web::get().to(info)));
}

#[api_docs_macros::path(
    get,
    path = "/api/v1/system/info",
    tag = "system",
    operation_id = "system_info",
    responses(
        (status = 200, description = "System information", body = inline(ApiResponse<SystemInfo>)),
    ),
)]
pub async fn info(state: web::Data<AppState>) -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(ApiResponse::ok(SystemInfo {
        name: "AsterForge",
        version: env!("CARGO_PKG_VERSION"),
        build_time: env!("ASTER_BUILD_TIME"),
        site_title: state
            .get_ref()
            .runtime_config
            .get("site.title")
            .unwrap_or_else(|| "AsterForge".to_string()),
    })))
}

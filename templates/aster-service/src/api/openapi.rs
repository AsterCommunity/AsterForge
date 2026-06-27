//! OpenAPI document assembly.
//!
//! Product routes should add their annotated handlers and DTO schemas here. The module is compiled
//! only for debug builds with the `openapi` feature, keeping normal release binaries free of API
//! documentation generation overhead.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
        description = env!("CARGO_PKG_DESCRIPTION"),
        license(name = "MIT"),
    ),
    paths(
        crate::api::routes::health::healthz,
        crate::api::routes::health::readyz,
    ),
    components(
        schemas(
            crate::api::response::HealthResponse,
            crate::api::response::ReadinessComponent,
            crate::api::response::ReadinessResponse,
        )
    )
)]
pub struct ApiDoc;

#![cfg(all(debug_assertions, feature = "openapi"))]
//! OpenAPI generation test.

use std::fs;

use {{crate_name}}::api::openapi::ApiDoc;
use utoipa::OpenApi;

#[test]
fn generate_openapi() {
    let doc = ApiDoc::openapi();
    let json = serde_json::to_string_pretty(&doc).expect("serialize openapi document");

    fs::create_dir_all("./frontend-panel/generated").expect("create frontend generated directory");
    fs::write("./frontend-panel/generated/openapi.json", json)
        .expect("write frontend OpenAPI spec");
}

#[test]
fn generated_openapi_contains_health_route() {
    let doc = ApiDoc::openapi();
    let value = serde_json::to_value(&doc).expect("openapi json value");

    assert!(value["paths"].get("/health").is_some());
    assert!(value["paths"].get("/health/ready").is_some());
    assert!(
        value["components"]["schemas"]
            .get("StatusResponse")
            .is_some()
    );
    assert!(
        value["components"]["schemas"]
            .get("ErrorResponse")
            .is_some()
    );
}

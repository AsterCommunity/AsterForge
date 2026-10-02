//! Integration coverage for the default `path` macro expansion.

#[aster_forge_api_docs_macros::path(
    get,
    path = "/health",
    responses((status = 200, description = "ok"))
)]
fn annotated_value() -> &'static str {
    "ok"
}

#[test]
fn path_macro_leaves_item_callable_without_openapi_feature() {
    assert_eq!(annotated_value(), "ok");
}

#[cfg(all(debug_assertions, feature = "openapi"))]
#[test]
fn path_macro_registers_openapi_operation() {
    use utoipa::OpenApi;

    #[derive(OpenApi)]
    #[openapi(paths(annotated_value))]
    struct ApiDoc;

    let doc = ApiDoc::openapi();
    let operation = doc.paths.paths["/health"].get.as_ref().unwrap();
    let utoipa::openapi::RefOr::T(response) = &operation.responses.responses["200"] else {
        panic!("expected an inline response");
    };
    assert_eq!(response.description, "ok");
}

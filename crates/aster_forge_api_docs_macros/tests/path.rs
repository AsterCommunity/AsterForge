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

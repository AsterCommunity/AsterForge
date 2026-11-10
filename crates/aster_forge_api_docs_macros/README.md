# aster_forge_api_docs_macros

`OpenAPI` attribute macros shared by Aster services.

The crate keeps route annotations lightweight in production builds while preserving
`utoipa::path` metadata when the `openapi` feature is enabled for debug builds. This lets
application code keep a single annotation path without pulling `OpenAPI` generation into normal
release binaries.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_api_docs_macros = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_api_docs_macros)
- [Rust API reference](https://docs.rs/aster_forge_api_docs_macros/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

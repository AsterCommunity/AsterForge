# aster_forge_api

Shared API response and pagination helpers for Aster services.

This crate contains small HTTP-facing types that are useful across service boundaries:
bounded limit query parsing, limit/offset pagination, cursor-page response shapes, cursor
validation helpers, overfetch trimming, and simple sort-order serialization. It deliberately
avoids depending on any concrete web framework or product entity so handlers can adapt it to
Axum, Actix, `OpenAPI` generation, or test-only fixtures.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_api = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_api)
- [Rust API reference](https://docs.rs/aster_forge_api/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

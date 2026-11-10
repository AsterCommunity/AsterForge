# aster_forge_observability

HTTP observability endpoints for Aster services.

This crate owns route-level observability glue with Actix and Axum adapters, such as the
Prometheus text exposition endpoint. Metrics recording traits and concrete backend state remain
in `aster_forge_metrics`; product route modules can call these helpers without carrying
backend-specific `#[cfg]` blocks.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_observability = { git = "https://github.com/AsterCommunity/AsterForge", version = "=0.1.0-alpha.2" }
```

## Features

Default features: `actix`.

Available features: `actix`, `axum`, `prometheus`. The `rustls-aws-lc-rs` metrics feature is forwarded only for OTLP consumers. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_observability)
- [Rust API reference](https://docs.rs/aster_forge_observability/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

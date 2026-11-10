# aster_forge_metrics

Shared metrics recorder traits and subsystem registration primitives.

Applications often share the same infrastructure metrics while exposing different
product-domain metrics. This crate keeps the common recorder surface small and provides a
instance-owned provider so each subsystem can register the metrics it owns without forcing every
product-specific method into a single shared trait. Concrete backends are selected once at the
product entrypoint through Forge feature flags, so business modules record metric semantics
without depending on exporter crates.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Features

Default features: `rustls-aws-lc-rs`, forwarded only when OTLP activates HTTP.

Available features: `allocator-metrics`, `backend-prometheus`, `otlp`, `runtime-health`, `rustls-aws-lc-rs`, `rustls-ring`, `native-tls`. Select features according to the integration documentation.

For a different OTLP TLS backend, disable default features and select it explicitly; ring provider initialization belongs to the product entrypoint. These backend controls are planned for `0.1.0-alpha.2`, which is not yet published.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_metrics)
- [Rust API reference](https://docs.rs/aster_forge_metrics/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

# aster_forge_config

Shared runtime configuration primitives for Aster services.

This crate owns product-neutral configuration mechanics: typed configuration
definitions, registry construction, storage value conversion, in-process
runtime snapshots, reload diffing, and cross-process reload notifications.
Product crates still own their concrete database entities, repositories,
localized labels, config keys, domain-specific normalizers, and any derived
runtime state that is built from configuration values.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_config = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`, `redis-pubsub`, `sea-orm`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_config)
- [Rust API reference](https://docs.rs/aster_forge_config/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

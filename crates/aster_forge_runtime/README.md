# aster_forge_runtime

Shared runtime primitives for Aster services.

This crate contains small process/runtime building blocks that are not tied
to a concrete product domain: health report aggregation, startup phase
coordination, shutdown phase coordination, buffered side-effect writing, and
termination signal handling. Product crates still own runtime state, audit
events, background task shutdown order, database handles, and
service-specific readiness checks.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_runtime = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_runtime)
- [Rust API reference](https://docs.rs/aster_forge_runtime/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

# aster_forge_test

Shared integration-test infrastructure for Aster services.

This crate packages the container machinery that Aster products previously duplicated in
their `tests/common` modules: shared reusable containers serialized by filesystem locks,
per-process resource registries with stale-process pruning, and readiness wait helpers.

Everything here is test support code. Failures panic with descriptive messages instead of
returning recoverable errors, because a broken test environment should fail fast and loud.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_test = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `containers`, `logs`, `mysql`, `postgres`, `process`, `redis`, `smtp`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_test)
- [Rust API reference](https://docs.rs/aster_forge_test/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

# aster_forge_alloc

Shared allocator tracking and memory statistics helpers.

Applications still own their `#[global_allocator]` selection and any
platform-specific allocator configuration. This crate provides the reusable
pieces used by Aster services: a debug tracking allocator for system-allocator
builds, and a single `stats` API that reports either tracked system
allocation counters or jemalloc counters depending on enabled features.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_alloc = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `jemalloc`, `jemalloc-stats`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_alloc)
- [Rust API reference](https://docs.rs/aster_forge_alloc/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

# aster_forge_storage_core

Shared storage utility helpers for Aster services.

The crate contains storage-adjacent primitives that are independent of concrete drivers:
safe relative object-key handling and S3-compatible endpoint normalization. Driver traits,
connector wiring, and credential storage remain in application crates where policy decisions
belong.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_storage_core = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_storage_core)
- [Rust API reference](https://docs.rs/aster_forge_storage_core/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

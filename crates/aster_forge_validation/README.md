# aster_forge_validation

Shared validation helpers for Aster services.

The crate collects validation routines that were previously repeated in service code, including
display text, frontend asset URLs, email handling, and filename handling. It keeps validation
errors as plain messages so API layers and domain services can decide how to present or translate
them.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_validation = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_validation)
- [Rust API reference](https://docs.rs/aster_forge_validation/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

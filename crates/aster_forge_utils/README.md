# aster_forge_utils

Shared low-level utility helpers for Aster services.

This crate holds small, dependency-light helpers that do not belong to a single domain module:
boolean-like string parsing, HTTP range/validator handling, checked numeric conversions, path
rendering helpers, loopback host detection, UUID/token helpers, and RAII cleanup guards. The
shared error type is intentionally simple so callers can map it into richer product errors.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_utils = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_utils)
- [Rust API reference](https://docs.rs/aster_forge_utils/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

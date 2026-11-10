# aster_forge_middleware

Shared HTTP middleware for Aster services.

This crate contains product-neutral middleware mechanics and Actix/Axum adapters.
It keeps framework-specific code out of `aster_forge_api`,
which remains focused on framework-neutral response and pagination helpers.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_middleware = "=0.1.0-alpha.2"
```

## Features

Default features: `actix`.

Available features: `actix`, `axum`, `metrics`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_middleware)
- [Rust API reference](https://docs.rs/aster_forge_middleware/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

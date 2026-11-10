# aster_forge_crypto

Shared cryptographic helpers for Aster services.

The crate exposes password hashing, digest utilities, and a versioned authenticated secret
envelope shared by Aster products. It keeps the error surface narrow so services can map
cryptographic failures into their own API or domain errors without depending on
implementation-specific error types.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_crypto = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_crypto)
- [Rust API reference](https://docs.rs/aster_forge_crypto/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

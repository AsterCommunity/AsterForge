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
aster_forge_crypto = { git = "https://github.com/AsterCommunity/AsterForge", features = ["password", "sha256", "hmac-sha256", "secret-envelope"] }
```

## Features

No algorithms are enabled by default; lowercase `bytes_to_hex` and error classifications remain available.

- `password`: Argon2id hashing, bounded verification and password policy.
- `sha256`: SHA-256 digest and incremental hasher.
- `hmac-sha256`: HMAC-SHA-256, including `sha256`.
- `secret-envelope`: existing v1 AES-256-GCM/HKDF-SHA256 envelope, including `sha256`.

Select only the algorithms your application needs. Algorithms, fixed vectors and persisted formats are unchanged. These features target the next release after `0.1.0-alpha.2`.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_crypto)
- [Rust API reference](https://docs.rs/aster_forge_crypto/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

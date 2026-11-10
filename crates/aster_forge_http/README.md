# aster_forge_http

Product-neutral outbound HTTP client mechanics for Aster services.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_http = { git = "https://github.com/AsterCommunity/AsterForge", features = ["json", "rustls-aws-lc-rs"] }
```

## Features

No features are enabled by default. Select request encodings (`json`, `form`, `query`, `multipart`), `stream`, `blocking`, `http2`, `charset`, and `system-proxy` independently.

HTTPS requires an explicit backend: `rustls-aws-lc-rs`, `rustls-ring`, `rustls` with an application-installed provider, or `native-tls` / `native-tls-vendored`. For ring, install `rustls::crypto::ring::default_provider()` at the application entrypoint before building clients. Forge never installs a global crypto provider. Certificate and hostname verification stay enabled.

When Cargo unifies multiple backends, `.tls_backend(TlsBackend::Rustls)` or `TlsBackend::Native` makes the application choice explicit for async and blocking clients. Bounded response reads work without the `stream` feature.

These feature boundaries are planned for `0.1.0-alpha.2`, which is not yet published; use the matching release or Git revision.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_http)
- [Rust API reference](https://docs.rs/aster_forge_http/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

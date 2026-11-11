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

## Sharing clients

`HttpClient::clone()` is a cheap shared handle: it reuses the same underlying
reqwest client, TCP connection pool, transport configuration and observer. It does
not copy the pool. Each `HttpClient::builder().build()` creates an independent pool.
Inject a clone into each session/service once; an extra `Arc<HttpClient>` is unnecessary.

Keep TLS backend, trust roots, proxy and connect/idle settings fixed per pool.
Set account/session User-Agent, credentials and whole-request timeout on each
request (`.header(USER_AGENT, ...)`, `.bearer_auth(...)`, `.timeout(...)`). Request
overrides do not change other sessions or the pool's defaults. Use independent
builds for incompatible transport/trust policies.

For fallible lazy initialization, keep `Arc<tokio::sync::OnceCell<HttpClient>>` in
caller-owned application state and use `get_or_try_init`. Successful initialization
is shared; errors and cancelled initialization leave the cell available for retry.
Do not capture the first account's request policy in the initializer. No HTTP
initialization is needed on local-only command paths, and no global singleton is installed.
Tokio's `sync` feature is selected by the caller; Forge adds no lazy-pool type or runtime dependency.

See the [tested batch CLI / lazy service example](examples/shared_client.rs) and
the [integration guide](https://forge.docs.astercosm.com/crates/aster_forge_http) for
the eager embedded-service alternative. Run the offline path without network I/O:

```bash
cargo run -p aster_forge_http --example shared_client
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_http)
- [Rust API reference](https://docs.rs/aster_forge_http/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

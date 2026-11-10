# aster_forge_mail

Shared mail infrastructure helpers for Aster services.

This crate intentionally does not own product templates, recipients, audit
records, user context, SMTP configuration keys, or database entities. It only
provides small mechanics that recur around mail outbox dispatch: dispatch
counters, retry delay selection, error truncation, and best-effort retry when
persisting a successful SMTP delivery. It also provides a product-neutral
template registry and placeholder rendering helpers for services that own
their own template codes and payloads.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_mail = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`, `persistence`, `runtime-component`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_mail)
- [Rust API reference](https://docs.rs/aster_forge_mail/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

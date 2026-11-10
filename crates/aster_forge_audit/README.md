# aster_forge_audit

Shared audit runtime integration for Aster services.

Product crates own audit actions, detail schemas, authorization rules,
operator-facing presentation. Forge owns the runtime lifecycle contract shared
by Aster products: record a best-effort server shutdown audit event, then stop/join
the caller-owned audit writer before database handles close. With the `db-writer` feature,
Forge also owns the shared buffered database writer used by Aster products.
Products that also use a mail outbox can enable the `mail-outbox-dependency`
feature to make the standard audit constructors depend on the mail outbox drain
component.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_audit = "=0.1.0-alpha.2"
```

## Features

Default features: none.

Available features: `database-container-tests`, `db-writer`, `mail-outbox-dependency`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_audit)
- [Rust API reference](https://docs.rs/aster_forge_audit/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

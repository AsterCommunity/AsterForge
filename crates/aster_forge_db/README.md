# aster_forge_db

Shared database utilities for Aster services.

This crate contains framework-neutral `SeaORM` helpers: connection setup, retry policy, offset
pagination, full-text search query helpers, whitelisted sorting, and transaction wrappers.
Product migrations, entities, and repository-specific query logic intentionally remain outside
this crate.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_db = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `all`, `audit-log`, `database-container-tests`, `mail-outbox`, `runtime-component`, `runtime-lease`, `scheduled-task`, `system-config`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_db)
- [Rust API reference](https://docs.rs/aster_forge_db/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

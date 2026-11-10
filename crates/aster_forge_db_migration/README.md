# aster_forge_db_migration

Shared `SeaORM` migration coordination and schema helpers for Aster products.

Products retain their migration lists, history compatibility policy, table definitions, and
data backfills. This crate owns the reusable execution mechanics and migration-only helpers.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_db_migration = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_db_migration)
- [Rust API reference](https://docs.rs/aster_forge_db_migration/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

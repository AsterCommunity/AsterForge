# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to
`aster_forge_db_migration`. Treat
[`../../docs/crates/aster_forge_db_migration.md`](../../docs/crates/aster_forge_db_migration.md)
as authoritative for migration coordination and schema helpers.

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target module, tests, and the crate documentation.
2. Inspect at least one product migration crate using the API. Products retain their migration
   lists, history policy, business tables, and compatibility decisions.
3. Keep helpers portable across PostgreSQL, MySQL, and SQLite unless the API name explicitly
   declares a backend restriction.

## Ownership Boundaries

- This crate owns product-neutral migration execution mechanics: cross-process serialization,
  backend-specific migration locks, migration-only index helpers, and portable column builders.
- It may provide adapters for `sea_orm_migration::MigratorTrait`, but it does not own a product's
  migration list or historical migration policy.
- Product repositories own migration names, migration ordering, application-schema detection,
  legacy-history compatibility, data backfills, and product table definitions.

## Change Constraints

- PostgreSQL and MySQL lock acquisition must remain connection-bound for the entire callback.
- Callback errors return unchanged when cleanup succeeds. Cleanup failures must be composed without
  hiding the original callback failure.
- Lock namespaces are persistent coordination contracts. Products performing rolling upgrades must
  keep stable MySQL lock names and PostgreSQL advisory keys.
- Migration-only helpers belong here rather than in `aster_forge_db`; avoid compatibility facades
  that leave two public owners for the same API.

## Validation

```bash
cargo test -p aster_forge_db_migration
cargo clippy -p aster_forge_db_migration --all-targets -- -D warnings
```

Coordination changes also require real PostgreSQL and MySQL concurrency coverage through
`aster_forge_test`, plus a downstream product migration test through a local patch.


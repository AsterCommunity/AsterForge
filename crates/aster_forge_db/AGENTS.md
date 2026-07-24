# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_db`. Treat [`../../docs/crates/aster_forge_db.md`](../../docs/crates/aster_forge_db.md) as authoritative for connections, retries, transactions, query helpers, and shared schemas and stores.

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target module, `tests/sqlite.rs`, and the corresponding crate documentation.
2. For shared tables, also read the crate that owns the higher-level mechanics: runtime leases in `aster_forge_runtime`, scheduled tasks in `aster_forge_tasks`, and likewise for mail, audit, and configuration.
3. Write down `old schema/store -> Forge API -> product-owned responsibility -> required behavior tests`. Historical product migrations remain product-owned; do not pull them back into Forge.

## Ownership Boundaries

- This crate owns SeaORM connections and handles, retry classification, transactions, pagination, sorting, search, and schemas, indexes, entities, and stores for product-neutral infrastructure tables.
- It may own generic persistence mechanics for runtime leases, scheduled tasks, system configuration, mail outbox, and audit logs.
- It does not own product entities, historical migrations, business repositories, permission filtering, API DTOs or text, or product statistical definitions.

## Change Constraints

- Keep features split by shared table or runtime capability. The default build must not implicitly create infrastructure tables.
- Schema constants, builders, entities, and store predicates must remain aligned. Treat field-width and index-name changes as compatibility changes.
- Claim, renew, complete, lease takeover, and mail try-claim writes need owner, token, or timestamp fences. Never create a race with a read-then-write sequence.
- Retry only errors backed by driver evidence and safe replay semantics. Do not blindly replay non-idempotent writes or work that already produced external side effects.
- Product errors from transaction callbacks return unchanged. Begin, commit, and rollback failures define the Forge DB boundary, and rollback failure must not replace the original callback error.
- Query helpers accept allowlisted columns or typed inputs, never arbitrary field strings that can be concatenated into SQL.
- Handle PostgreSQL, MySQL, and SQLite dialect differences explicitly. Passing only an in-memory SQLite test proves very little about cross-backend correctness.

## Validation

```bash
cargo test -p aster_forge_db
cargo check -p aster_forge_db --no-default-features
cargo test -p aster_forge_db --all-features
cargo clippy -p aster_forge_db --all-targets --all-features -- -D warnings
```

Schema or store changes also require PostgreSQL/MySQL migration smoke tests or consumer tests through a local patch. Concurrent-claim tests must prove one winner, no update on fence mismatch, expired takeover, and completion postconditions.

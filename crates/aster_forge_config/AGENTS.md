# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_config`. The complete registry, value, snapshot, reload, and notification contract is in [`../../docs/crates/aster_forge_config.md`](../../docs/crates/aster_forge_config.md).

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, and the relevant modules. For notification changes, also read `aster_forge_events`; for persistence-boundary changes, read the `aster_forge_db::system_config` documentation and code.
- Keep four layers explicit: product definitions and normalizers, Forge registry and values, the DB store, and product-derived runtime state. Do not stir them into configuration soup.

## Ownership Boundaries

- This crate owns typed definitions and registries, structured storage-value conversion, synchronous and asynchronous runtime snapshots, diff and restart decisions, and cross-process reload-notification mechanics.
- The `system_config` entity, table builder, and store belong to `aster_forge_db`.
- Products own keys, categories, i18n, defaults, business normalizers and dependencies, administration APIs, permissions, audit, and derived runtime state.

## Change Constraints

- Write-path normalizers should reject bad values strictly. Runtime read helpers may fail soft and use defaults according to the existing contract. Do not conflate the two paths.
- Notifications carry product-neutral reload hints, not secrets or complete configuration snapshots.
- Preserve namespace and origin filtering, self-message suppression, reconnect behavior, cancellation, and observer-failure isolation.
- Configuration marked `requires_restart` must not be represented as hot-reloaded.
- The `openapi`, `sea-orm`, and `redis-pubsub` features must compile independently. The default core must not pull in a backend implicitly.

## Validation

```bash
cargo test -p aster_forge_config
cargo check -p aster_forge_config --no-default-features
cargo test -p aster_forge_config --all-features
cargo clippy -p aster_forge_config --all-targets --all-features -- -D warnings
```

Redis notification changes must run `tests/redis_config_sync.rs` and cover malformed payloads, self and namespace suppression, reconnects, cancellation, observer failures, and changed-key diffs.

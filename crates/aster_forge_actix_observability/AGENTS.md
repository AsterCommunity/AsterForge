# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_actix_observability`. See [`../../docs/crates/aster_forge_actix_observability.md`](../../docs/crates/aster_forge_actix_observability.md) for the complete integration contract.

## Before Making Changes

- Read this crate's `Cargo.toml`, `src/lib.rs`, crate documentation, and the backend/export contract in `aster_forge_metrics`.
- If the change concerns request recording, inspect `aster_forge_actix_middleware` instead of mixing middleware with endpoint glue.

## Ownership Boundaries

- This crate owns Actix route-level observability glue, currently centered on conditionally registering the Prometheus `/metrics` endpoint.
- Recorder traits, the registry, export implementation, and system metrics belong to `aster_forge_metrics`.
- HTTP request metrics middleware belongs to `aster_forge_actix_middleware`.
- Dashboards, alerts, authentication policy, product route structure, and business-metric semantics remain product-owned.

## Change Constraints

- Product routes must be able to call the helper unconditionally. With the feature disabled, it must be a no-op that preserves the original scope.
- The endpoint exports metrics only; it must not initialize the backend implicitly. An uninitialized registry must retain explicit unavailable semantics.
- Do not add product response envelopes, administration permissions, or backend-specific global configuration here.
- New exporter glue must be explicitly feature-gated, leaving the default build lightweight.

## Validation

```bash
cargo test -p aster_forge_actix_observability
cargo test -p aster_forge_actix_observability --features prometheus
cargo clippy -p aster_forge_actix_observability --all-targets --all-features -- -D warnings
```

Cover the feature-disabled route absence, unavailable behavior before backend initialization, and the content type and body after initialization.

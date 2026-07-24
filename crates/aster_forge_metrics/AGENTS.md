# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_metrics`. See [`../../docs/crates/aster_forge_metrics.md`](../../docs/crates/aster_forge_metrics.md) for recorder, catalog, Prometheus backend, and health-bridge behavior.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, and `src/prometheus.rs` when relevant. Also read the related crate documentation for allocator, health, or Actix changes.
- Before adding a metric, write down the cardinality bound for every label. If the bound is unknowable, the label is probably tomorrow's monitoring incident.

## Ownership Boundaries

- This crate owns product-neutral recorder traits, noop implementations, metric descriptors and catalogs, single-backend selection, and the optional Prometheus exporter and system updater.
- The Actix `/metrics` route belongs to `aster_forge_actix_observability`; HTTP middleware belongs to `aster_forge_actix_middleware`.
- Products own the decision to record business metrics, dashboards, alerts, and product label semantics.

## Change Constraints

- A build permits only one `backend-*` feature, while the default feature set must continue to support a complete noop configuration.
- Never put raw SQL, URLs, user/file/configuration keys, topics, endpoints, runtime IDs, error text, or other high-cardinality or sensitive values into labels.
- Descriptor registration must be atomic and retryable. A failed batch rolls back registrations from that batch so it does not leave phantom duplicates.
- Validate handle kind and label count before reaching a backend panic.
- Updaters must respond to cancellation, and one collection failure must not kill the entire update loop.
- Allocator and health bridges translate shared facts only; they do not invent product readiness or alerting policy.

## Validation

```bash
cargo test -p aster_forge_metrics
cargo test -p aster_forge_metrics --all-features
cargo clippy -p aster_forge_metrics --all-targets --all-features -- -D warnings
```

Cover noop behavior, duplicate registration, batch rollback, label counts, wrong kinds, histogram buckets, concurrent recording, updater cancellation, and exporter output. Run Actix observability tests when route glue changes.

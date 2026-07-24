# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_runtime`. Treat [`../../docs/crates/aster_forge_runtime.md`](../../docs/crates/aster_forge_runtime.md) as authoritative for the component graph, health, leases, startup and shutdown, and buffered writer.

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target module, and the crate documentation.
2. For component changes, list every component, dependency, health check, startup phase, task, and shutdown phase. Confirm graph direction and actual stop order.
3. Inspect consumer factories in DB, cache, tasks, mail, audit, and related crates. Subsystems expose bundles or factories; they do not create their own root registry.

## Ownership Boundaries

- This crate owns `AsterRuntime`, component, bundle, and registry graphs, health aggregation, startup and shutdown coordination, signal handling, the runtime-lease supervisor, and the generic buffered batch writer.
- Products own resource creation, `AppState`, business startup, concrete health checks, audit/mail/task/DB adapters, and user-facing health or API representations.
- This crate defines lifecycle mechanics without depending on Actix, SeaORM, or product entities.

## Change Constraints

- Component names, dependencies, and phases are cross-crate public contracts. Detect duplicates, missing dependencies, and cycles rather than relying on registration order.
- Shutdown follows the dependency graph, not handwritten order. When a graph layer runs concurrently, report ordering and error aggregation must remain deterministic and diagnosable.
- Required startup failure stops later startup work. Optional failure continues but must appear in the report.
- Health checks need scope, requirement, timeout, and panic/error isolation. Details and metrics labels remain low-cardinality.
- Lease acquisition, renewal, loss, and cancellation require owner and fence semantics. Losing the lease stops the corresponding worker group.
- Buffered writers must not lose records, double-schedule flushes, or await while holding locks across batch, delay, overflow, and shutdown paths.
- Signal handlers, task cancellation, and resource shutdown must tolerate idempotent calls.

## Validation

```bash
cargo test -p aster_forge_runtime
cargo test -p aster_forge_runtime --all-features
cargo clippy -p aster_forge_runtime --all-targets --all-features -- -D warnings
```

Focus on graph cycles, missing dependencies, duplicates, topological shutdown, phase panics and errors, health timeouts and scopes, signal cancellation, lease loss and takeover, buffer overflow and flush, and composition of multiple subsystem bundles.

# aster_forge_runtime

`aster_forge_runtime` contains small process/runtime primitives shared by Aster services. It is intentionally not an application runtime framework: product crates still own `AppState`, startup mode, audit events, background task shutdown order, database handles, and concrete readiness probes.

## Use Cases

- Represent component and aggregate health reports.
- Compute the worst health status across components.
- Render compact health summaries for logs, task records, or admin APIs.
- Wait for SIGINT/SIGTERM/Ctrl+C with one shared implementation.

Do not put product runtime state, product config keys, audit actions, task kinds, or service-specific health probes in this crate.

## Cargo

```toml
[dependencies]
aster_forge_runtime = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Health

Module: `aster_forge_runtime::health`

Main types:

- `HealthStatus`
- `HealthComponentReport`
- `SystemHealthReport`

Forge owns only the report model and aggregation rules:

- `Healthy`, `Degraded`, and `Unhealthy` statuses.
- Stable lowercase wire values through `HealthStatus::as_str()`.
- Component constructors for healthy, degraded, and unhealthy reports.
- Aggregate `status()`, `has_issues()`, and `summary()`.

Product crates still decide which probes run. For example, Yggdrasil checks database and cache health in its product service, then stores the result in Forge's report model.

Minimal example:

```rust
use aster_forge_runtime::{HealthComponentReport, HealthStatus, SystemHealthReport};

let report = SystemHealthReport::new(vec![
    HealthComponentReport::healthy("database", "database ping succeeded"),
    HealthComponentReport::degraded("cache", "configured redis is using memory fallback"),
]);

assert_eq!(report.status(), HealthStatus::Degraded);
assert_eq!(report.summary(), "database healthy, cache degraded");
```

## Shutdown Signal

Module: `aster_forge_runtime::shutdown`

Main API:

- `wait_for_termination_signal()`
- `TerminationSignal`
- `RuntimeSignalError`

`wait_for_termination_signal()` waits for:

- Unix `SIGINT`
- Unix `SIGTERM`
- cross-platform `Ctrl+C` on non-Unix targets

It returns the observed `TerminationSignal` and logs a shared graceful-shutdown message.

Product crates still own the actual shutdown sequence:

- recording server-shutdown audit events;
- stopping background task handles;
- flushing buffered audit/log/mail queues;
- closing database/cache/storage handles;
- emitting product metrics.

## Error Boundary

`RuntimeSignalError` only describes failure to install or await signal handlers. Product crates should map it into their own startup/runtime error type at the boundary.

## Testing

Shared tests cover health aggregation and signal labels. Product tests should cover their own readiness probes and shutdown ordering.

## Reference Projects

- AsterYggdrasil: uses Forge health models while keeping database/cache probes in product code, and delegates termination signal waiting while keeping product shutdown ordering.

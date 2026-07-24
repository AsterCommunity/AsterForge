# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_tasks`. See [`../../docs/crates/aster_forge_tasks.md`](../../docs/crates/aster_forge_tasks.md) for typed specs, lease and heartbeat, dispatch, runtime, scheduling, steps, and component contracts.

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target module, and the crate documentation.
2. For persistence changes, also read `aster_forge_db::scheduled_task`; for lifecycle changes, read `aster_forge_runtime`.
3. Map `claim -> heartbeat/renew -> execute -> persist outcome -> release/retry`, marking every token fence, cancellation point, and crash point.

## Ownership Boundaries

- This crate owns typed task specs and registries, payload/result codecs, deduplication, steps, claim/heartbeat/lease mechanics, lane concurrency, dispatch and drain, periodic and scheduled runtimes, and component factories.
- Products own task kinds, business payload and result schemas, concrete processors, SeaORM entities and repositories, runtime configuration, metrics labels, audit, and administration APIs.
- The generic scheduled-task catalog store lives in `aster_forge_db`; this crate defines broker-neutral and runtime contracts.

## Change Constraints

- Processing-token, owner, and claim-timestamp fences must cover heartbeat, success, retry, failure, and release. Once a worker loses its lease, it must not write final success.
- Calculate lane capacity before claiming. Do not overclaim and then hold leases behind a semaphore.
- Shutdown stops new claims, cancels or waits for workers, releases recoverable work into retry, and drains handles idempotently.
- Scheduled claims renew throughout execution. An ownership mismatch stops renewal without bluntly cancelling the business future; completion performs the final fence check.
- Retry, permanent, manual, timeout, and shutdown classifications come from explicit contracts, never error-string matching.
- Dedupe-key length, namespace, and wire stability are cross-process persistence contracts.
- Runtime/task registries must keep enums, wire values, presentation codes, and actually scheduled definitions consistent in both directions.
- With `runtime` and `runtime-component` disabled, the spec, retry, and step core remains lightweight.

## Validation

```bash
cargo test -p aster_forge_tasks
cargo check -p aster_forge_tasks --no-default-features
cargo test -p aster_forge_tasks --all-features
cargo clippy -p aster_forge_tasks --all-targets --all-features -- -D warnings
```

Concurrency tests use barriers or rendezvous points and assert one winner, no update on token mismatch, heartbeat loss, lane limits, renew and complete fences, shutdown release, panic mapping, deduplication, and step-transition postconditions.

# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_test`. See [`../../docs/crates/aster_forge_test.md`](../../docs/crates/aster_forge_test.md) for fixture, container reuse, process guard, and resource-cleanup contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the target helper, and the crate documentation. For container changes, also inspect a real consumer integration test.
- Test support may fail fast, but first confirm that a failure means the test environment is broken rather than representing product behavior that a test should assert.

## Ownership Boundaries

- This crate owns cross-product test infrastructure: isolated temporary directories, SQLite fixtures, shared container state, locks and leases, Redis/PostgreSQL/MySQL/Mailpit helpers, real subprocesses, and readiness waiting.
- Products own fixtures and seeds, migrations, business assertions, resource-naming semantics, and CI service orchestration.
- Production crates must not depend on this crate. It belongs only in dev-dependencies and test harnesses.

## Change Constraints

- Multi-checkout isolation, process-unique resource names, file-locked read-modify-write, and stale-PID pruning form one contract. Do not change only one piece.
- `ReuseDirective::Always` means data survives across test runs. Helpers must not assume every reused container starts empty.
- Guard `Drop` performs best-effort cleanup; explicit `shutdown` or `destroy` reports failures. Both paths must be idempotent.
- Subprocess failure reports retain stdout/stderr tails without buffering unbounded output or exposing secret environment variables.
- Temporary files belong in system or explicitly supplied test directories, never fixed repository paths. SQLite sidecars are cleaned with their containing directory.
- Container image tags and readiness conditions are reproducibility contracts; version changes require explicit validation.

## Validation

```bash
cargo test -p aster_forge_test
cargo test -p aster_forge_test --features process
cargo test -p aster_forge_test --all-features
cargo clippy -p aster_forge_test --all-targets --all-features -- -D warnings
```

Container features require a real Docker environment. Cover parallel checkouts, lock-file round trips and corruption, stale-process pruning, orphan cleanup after abnormal exit, port allocation, timeout diagnostics, and idempotent drop behavior.

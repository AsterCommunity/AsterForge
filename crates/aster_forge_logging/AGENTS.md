# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_logging`. See [`../../docs/crates/aster_forge_logging.md`](../../docs/crates/aster_forge_logging.md) for initialization, configuration, degradation, and guard-lifetime contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, and the crate documentation. Inspect existing handling for the global subscriber, file writers, and test isolation.
- Before adding product lifecycle logs, check whether Actix or the runtime already emits the same information. Duplicate startup slogans only pollute logs.

## Ownership Boundaries

- This crate owns tracing-subscriber initialization, `RUST_LOG` precedence, text and JSON formats, stdout and file writers, rotation and retention, and the non-blocking writer guard.
- Products own span fields, request/user/task context, audit events, business logging policy, and deployment configuration sources.

## Change Constraints

- A global subscriber installs once. Repeated initialization in embedded runtimes or shared test processes follows the existing warning path rather than panicking.
- File initialization failure must degrade visibly to stdout with a warning; never swallow it silently.
- The returned `WorkerGuard` lifetime is a correctness requirement. Do not design an API that encourages immediate drop.
- JSON logs must remain machine-parseable. Do not hard-code ANSI, duplicate timestamps, or product-specific fields into the shared formatter.
- Retention cleanup may remove only files explicitly matched as this crate's log files, never neighboring data.

## Validation

```bash
cargo test -p aster_forge_logging
cargo clippy -p aster_forge_logging --all-targets -- -D warnings
```

Isolate global-state tests serially or in subprocesses. Cover text and JSON output, valid and invalid `RUST_LOG`, file-failure fallback, rotation naming, retention boundaries, and repeated initialization.

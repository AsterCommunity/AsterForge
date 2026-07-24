# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_panic`. See [`../../docs/crates/aster_forge_panic.md`](../../docs/crates/aster_forge_panic.md) for hook installation and crash-report contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, and the crate documentation. Separate pure report-rendering tests from real global-hook tests that contaminate process state.

## Ownership Boundaries

- This crate owns the process-wide panic hook, crash log, backtrace, stderr notice, and issue-target construction.
- Products own application metadata and path configuration, supervisors, telemetry reporting, recovery policy, and issue-creation workflows.

## Change Constraints

- The panic path should minimize allocation, locking, and dependencies. File-write failure needs a stderr fallback and must never trigger another panic inside the hook.
- Hook and configuration follow a process-global first-install contract. Do not add mutable global behavior that silently changes targets at runtime.
- Crash reports must not record secrets, tokens, complete request bodies, or product user data.
- Issue URLs and paths come from explicit configuration only. This crate does not perform network requests or submit issues.
- Tests must not write to the repository's `data/` directory. Use isolated temporary paths or subprocesses.

## Validation

```bash
cargo test -p aster_forge_panic
cargo clippy -p aster_forge_panic --all-targets -- -D warnings
```

Cover string and non-string payloads, missing thread names and locations, log-directory failure, report rendering, first-configuration retention, and a real hook in a subprocess. Do not let concurrent unit tests overwrite shared global state.

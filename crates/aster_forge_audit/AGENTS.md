# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_audit`. Treat [`../../docs/crates/aster_forge_audit.md`](../../docs/crates/aster_forge_audit.md) as the source of truth for lifecycle, buffered-writer, and product-boundary behavior.

## Before Making Changes

- Read the crate documentation, `Cargo.toml`, `src/lib.rs`, `src/db_writer.rs`, and the relevant `aster_forge_runtime`, `aster_forge_db`, and `aster_forge_mail` documentation.
- Map the shutdown dependencies first: what stops producing audit records, when lifecycle events are recorded, when buffers flush, and when database handles close.

## Ownership Boundaries

- This crate owns the generic audit runtime component, startup and shutdown phases, buffered DB writer, and flush mechanics.
- Generic `audit_logs` schemas, stores, and queries belong to `aster_forge_db`; this crate reuses them behind features rather than duplicating repositories.
- Products own `AuditAction`, entity types, detail schemas, redaction, permissions, presentation, statistical definitions, and the decision to record a given action.
- `mail-outbox-dependency` expresses a shared shutdown-graph relationship only. It does not let audit code interpret mail business semantics.

## Change Constraints

- Component names, dependencies, and phases are cross-crate contracts. When changing them, inspect the runtime, mail, DB, and consumer graphs together.
- Clearly distinguish best-effort lifecycle audit from mandatory flush failure policy. Do not collapse them into one error class.
- Queue overflow, delayed flush, direct-write fallback, and shutdown flush must not lose records or create uncontrolled duplication.
- The global manager is a compatibility entry point, not an invitation to expand global state. Prefer explicitly passed resources for new APIs.

## Validation

```bash
cargo test -p aster_forge_audit
cargo test -p aster_forge_audit --all-features
cargo clippy -p aster_forge_audit --all-targets --all-features -- -D warnings
```

Cover uninitialized fallback, batching, delay, overflow, flush failures, component dependency order, and the DB/mail feature combinations.

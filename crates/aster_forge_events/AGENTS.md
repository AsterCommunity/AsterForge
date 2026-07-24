# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_events`. See [`../../docs/crates/aster_forge_events.md`](../../docs/crates/aster_forge_events.md) for transport, supervisor, and transient-bus contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the target module, and `tests/redis_event_bus.rs`.
- If a consumer such as configuration reload motivates the change, separate transport facts from product payload semantics and move only the former here.

## Ownership Boundaries

- This crate owns the transient event bus, broker connection lifecycle, reconnects, backoff, cancellation, raw payload delivery, and low-cardinality connection observations.
- Products own payload schemas, authorization, namespace and origin filtering, deserialization-failure policy, and local event semantics.
- Redis Pub/Sub is a transient hint. It does not provide history replay, persistence, exactly-once delivery, or business-ordering guarantees.

## Change Constraints

- Transport code must not parse workspace, user, team, task, storage, or configuration-key semantics.
- Product callbacks handle and skip malformed payloads; malformed payloads must not kill the subscription supervisor.
- Distinguish connection failure, stream termination, and cancellation explicitly. Never reconnect after cancellation.
- Backoff requires a cap and jitter. Tests use controlled policies rather than real long sleeps.
- With the `redis` feature disabled, the local bus and broker-neutral supervisor must remain usable.

## Validation

```bash
cargo test -p aster_forge_events
cargo test -p aster_forge_events --features redis
cargo clippy -p aster_forge_events --all-targets --all-features -- -D warnings
```

Redis changes should cover initial connection failure, runtime disconnection, stream termination, recovery observations, cancellation, and malformed-payload isolation.

# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_cache`. See [`../../docs/crates/aster_forge_cache.md`](../../docs/crates/aster_forge_cache.md) for backend, TTL, fallback, Bloom-filter, and health contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the relevant backend, `tests/cache.rs`, and any container tests involved in the change.
- Read `aster_forge_runtime` for runtime health changes and `aster_forge_test` for real-container fixture changes.

## Ownership Boundaries

- This crate owns the byte-oriented `CacheBackend`, JSON extensions, memory and Redis backends, fallback circuits, reservations, Bloom filters, and the optional runtime health component.
- Products own key naming, invalidation timing, session/token/verification-code semantics, fallback business policy, and error or metrics presentation.
- Keep the trait object-safe. Do not add business generics or product entities to the core interface for one consumer's convenience.

## Change Constraints

- Memory and Redis must expose the same observable semantics for TTL, `take`, set-if-absent, and prefix invalidation.
- `Some(0)`, a zero default TTL, expired reservations, and concurrent winners are hard boundaries. Backend behavior changes require comparative tests.
- Only connectivity and transient availability failures enter fallback. Command or data errors such as WRONGTYPE must not masquerade as Redis outages.
- Bloom rebuilds must switch atomically; readers must never observe a half-built filter.
- Runtime health reports backend mechanics only. It does not decide product readiness policy.

## Validation

```bash
cargo test -p aster_forge_cache
cargo check -p aster_forge_cache --no-default-features
cargo test -p aster_forge_cache --all-features
cargo clippy -p aster_forge_cache --all-targets --all-features -- -D warnings
```

Redis behavior changes must run `tests/redis_container.rs`. Concurrent set-if-absent tests need a rendezvous and one-winner postconditions; do not gamble correctness on sleeps and scheduler luck.

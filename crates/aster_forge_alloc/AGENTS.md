# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_alloc`. See [`../../docs/crates/aster_forge_alloc.md`](../../docs/crates/aster_forge_alloc.md) for integration and feature semantics.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the crate documentation, and how `aster_forge_metrics` consumes `stats()`.
- Before touching allocator or unsafe code, write down the `GlobalAlloc` contract that must remain true. "It should probably be fine" is not a memory-safety argument.

## Ownership Boundaries

- This crate owns the system-allocator tracking wrapper and the unified `stats()` surface.
- Products still own `#[global_allocator]` selection, platform allocator configuration, alert thresholds, metric names, and diagnostic APIs.
- The distinction between `jemalloc` and `jemalloc-stats` is explicit. Do not silently change the zero-value degradation contract when only the former is enabled.

## Change Constraints

- Keep `unsafe_op_in_unsafe_fn` and `undocumented_unsafe_blocks` denied. Every unsafe block needs an accurate `SAFETY:` justification.
- Pointer handling, layouts, and success/failure accounting order must obey `GlobalAlloc`. Failed allocation or reallocation must not corrupt counters.
- Atomic counters are approximate diagnostics, not exact billing or memory-limit semantics.
- Every feature combination must compile independently, and the default build must not acquire a jemalloc-ctl dependency.

## Validation

```bash
cargo test -p aster_forge_alloc
cargo test -p aster_forge_alloc --features jemalloc
cargo test -p aster_forge_alloc --features jemalloc-stats
cargo clippy -p aster_forge_alloc --all-targets --all-features -- -D warnings
```

For accounting changes, cover allocation, zeroed allocation, growing and shrinking reallocation, failure paths, deallocation, and non-decreasing peaks. For metrics changes, also run the corresponding `aster_forge_metrics` feature tests.

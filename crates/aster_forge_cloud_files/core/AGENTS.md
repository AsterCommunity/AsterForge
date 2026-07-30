# AGENTS.md

This file supplements [`../../../AGENTS.md`](../../../AGENTS.md) and applies only to `aster_forge_cloud_files_core`. Treat [`../../../docs/crates/aster_forge_cloud_files_core.md`](../../../docs/crates/aster_forge_cloud_files_core.md) as the public integration reference. The current architecture and platform-contract evidence live in [`../../../tmp/ad-sync-client-cloud-files-foundation.md`](../../../tmp/ad-sync-client-cloud-files-foundation.md) and [`../../../tmp/cloud-files-platform-contract-matrix.md`](../../../tmp/cloud-files-platform-contract-matrix.md).

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target modules, the matching `tests/contract_*.rs` files, and the crate documentation.
2. Read the platform-contract matrix before changing identity, revision, cursor, hydration, mutation, cancellation, lifecycle, materialization, or capability semantics.
3. Map the change across four columns before implementation:

```text
Platform/backend contract -> Forge core mechanism -> Adapter/product responsibility -> Required contract tests
```

4. Use AsterDrive, AsterYggdrasil, and future products only as downstream adapter validation. Their endpoints, DTOs, entities, permissions, root meanings, and compatibility history do not define this crate's public model.

## Ownership Boundaries

- This crate owns operating-system-independent and product-independent cloud-files mechanics: scoped stable identity, metadata/content revisions, page/change/directory continuation types, capability negotiation, hydration coordination, content-storage ownership and sparse coverage, recoverable cache-write installation, immutable local dirty generations, resumable upload checkpoints, guarded eviction recovery, durable mutation/reconciliation mechanics, session fences, and synthetic conformance contracts.
- Platform crates own CFAPI, Apple File Provider, and FUSE native types, callback/request snapshots, identity codecs, error/completion mapping, platform lifecycle, and physical placeholder/materialization effects.
- Product crates own authentication, credentials, endpoints, DTOs, repositories, account/root mapping, permissions, quota and conflict presentation, executable/extension/daemon packaging, installation, and update policy.
- Native identity, local path, filename, inode, mount handle, and platform request ID are adapter mappings. They are not substitutes for `CloudItemKey`.

## Change Constraints

- Keep `CloudItemKey` scoped by namespace and root, stable across rename and same-root move, and independent from paths and native handles.
- Keep `MetadataRevision`, `ContentRevision`, and `ContentDigest` separate. Revisions are opaque equality/precondition tokens and have no implicit ordering or digest semantics.
- Keep `PageCursor`, `ChangeCursor`, and `DirectoryCookie` as distinct types. A native directory cookie is never a durable backend change cursor.
- Model unsupported operations as ordinary capability state. Reserve errors for invalid values, impossible intersections, violated required invariants, or attempted unsupported operations at an executable boundary.
- Put physical range alignment on range/progressive-range capabilities. Do not impose a platform transfer alignment on every backend content API.
- Capability intersection must preserve a real mathematical identity: `default()` is all unsupported, while `unconstrained()` is pass-through state for dimensions a boundary does not own.
- Keep platform-managed, provider-managed, and hybrid materialization ownership distinct. Core state must not claim ownership of a platform-managed physical copy.
- Shared hydration cancellation is waiter-scoped. One cancelled waiter must not terminate work still required by other waiters.
- Persist intent before acknowledging command-driven mutation success. Cursor advancement, remote outcome recording, platform effects, and session completion require explicit crash-recovery boundaries.
- Do not introduce platform bindings, product HTTP clients, product entities, implicit global registries, or user-visible error text into this crate.
- New public types and traits require crate documentation and deterministic contract tests. Avoid freezing serialization formats before store and platform PoCs prove them.

## Validation

```bash
cargo fmt --all -- --check
cargo test -p aster_forge_cloud_files_core
cargo clippy -p aster_forge_cloud_files_core --all-targets -- -D warnings
cargo check --workspace
```

When documentation or navigation changes, also run:

```bash
cd docs
bun run docs:build
```

Contract coverage should follow the touched boundary: scope collisions and rename stability; revision/digest separation; cursor type separation and reset/replay; capability intersection and limits; range revision fences and waiter cancellation; content ownership, sparse range coverage, cache-write physical/coverage ordering, immutable dirty generations, resumable upload offset and stale-completion fences, lease/dirty/pin eviction guards, physical-effect observation and crash recovery; durable mutation intent, idempotency, remote-outcome recovery, session generation, and crash injection at every persisted transition.

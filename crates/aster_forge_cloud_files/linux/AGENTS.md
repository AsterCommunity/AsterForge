# AGENTS.md

This file supplements [`../../../AGENTS.md`](../../../AGENTS.md) and applies to
`aster_forge_cloud_files_linux`. Treat [`../../../docs/crates/aster_forge_cloud_files_linux.md`](../../../docs/crates/aster_forge_cloud_files_linux.md) as the public integration reference. The cross-platform design and FUSE contract evidence are in [`../../../tmp/ad-sync-client-cloud-files-foundation.md`](../../../tmp/ad-sync-client-cloud-files-foundation.md) and [`../../../tmp/cloud-files-platform-contract-matrix.md`](../../../tmp/cloud-files-platform-contract-matrix.md).

## Ownership

- This crate owns Linux FUSE mappings: stable inode/generation records, directory-handle cookie
  snapshots, file-handle leases, FUSE errno/reply lifecycle, bounded callback-to-async dispatch,
  cache invalidation hooks, and mount-session lifecycle.
- `aster_forge_cloud_files_core` owns product-neutral identity, revisions, backend contracts,
  hydration, mutation, content-storage, upload, eviction, and durable-store contracts.
- Product crates own backend adapters, durable inode-record storage, authentication, permissions,
  daemon/service packaging, mount path, desktop integration, update policy, and user-visible
  errors.

## Boundary Rules

- `CloudItemKey` remains the stable scoped identity. An inode and its generation are restored
  adapter records, never a path hash or replacement product identity.
- A directory cookie is scoped to one open directory-handle snapshot. It is not a backend page or
  change cursor.
- FUSE callback threads only validate, reserve bounded capacity, and enqueue work. They do not
  perform remote I/O or block on product database work.
- Every accepted native reply must finish exactly once. Queue saturation, dispatcher closing,
  backend failure, task panic, and stale handle paths all map to a deterministic errno/reply.
- Keep kernel page cache distinct from provider backing content. Do not claim Windows/macOS pin or
  dehydration semantics on Linux.
- Do not import product DTOs, endpoints, account state, database entities, mount UX, or service
  managers.

## Validation

```bash
cargo fmt --all -- --check
cargo test -p aster_forge_cloud_files_linux --all-targets
cargo clippy -p aster_forge_cloud_files_linux --all-targets -- -D warnings
cargo check -p aster_forge_cloud_files_linux --target x86_64-unknown-linux-gnu
```

For real Linux testing, mount the memory example, exercise nested directory enumeration, range
reads, concurrent opens, unmount/restart, and read-only failure paths. Add writeback, interrupt,
and kernel-cache tests only with the matching durable mutation and invalidation mechanism.

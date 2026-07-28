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
- Writable existing-file operations use a caller-injected `LinuxWritebackStore`. A successful
  write/truncate must return a recoverable immutable local generation before the FUSE reply.
- When upload is enabled, the product `LinuxWritebackStore` transaction owns persistence of both
  the exact dirty snapshot and its caller-allocated core `ContentUploadIntent`. Core's
  `ContentUploadRunner` owns chunk/resume/commit/reconcile mechanics; Linux does not allocate
  operation IDs, call product transport, or choose retry scheduling.
- Writable mount startup uses core `SessionGeneration`. `activate_mount` must fence lower mount
  generations before returning recoverable snapshots; `open_recovered` must bind the returned
  session to that same generation and exact immutable snapshot.
- Recovery may bypass remote content hydration, but it still validates restored inode identity and
  backend item shape. The product owns durable bytes, metadata caching, and offline policy.
- Keep the first writable path on direct I/O. Do not enable kernel writeback cache until delayed
  write, append, mmap, flush ordering, and crash recovery have matching tests.
- New file creation requires a durable product-owned `CloudItemKey` plus inode/generation record
  before success. Do not allocate those identities from a path, hash, or transient callback state.
- Regular-file create uses an independently injected `LinuxNamespaceMutationStore`. Its product
  transaction must atomically persist the stable item, inode/generation record, complete core
  create intent, empty staging session, and active mount-generation comparison before returning a
  `LinuxCreateFileAcceptance`. The same concrete product store may implement writeback and
  namespace ports; keep the public mechanisms separate.
- `activate_namespace` returns non-terminal local creates for restart recovery. A newer generation
  may resume an older durable intent, but a future generation, substituted item/parent/session,
  duplicate key/inode/parent-name, or non-empty create acceptance is a contract failure before
  kernel exposure.
- Remote create runs in a product worker through core `MutationRunner`, with product-owned
  `CloudMutationBackend` and `MutationJournalStore` implementations. Product code still owns
  transport, identities, transaction mapping, retry scheduling, and any later upload intent. Do
  not call a product transport from FUSE `create`, and do not claim that the local overlay is a
  committed remote item.
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
cargo zigbuild -p aster_forge_cloud_files_linux \
  --test memory_cloud_drive_example --target aarch64-unknown-linux-gnu
```

For real Linux testing, mount the memory example, exercise nested directory enumeration, range
reads, existing-file write/truncate/flush/fsync/reopen, regular-file create followed by immediate
write/read/fsync/reopen, concurrent opens, duplicate create, unsupported mkdir/rename/unlink/rmdir,
and clean unmount. With the optional synthetic state directory, also crash the provider, clear the
stale mount, activate a higher generation, verify created namespace identity plus dirty bytes/size
before another write, and restart once more. Core upload-runner tests must cover chunk resume, lost
returns, unknown reconciliation, generation takeover, stale dirty completion, and concurrent
execution. Core mutation-runner tests must cover create outcome shape, lost returns, explicit
unknown reconciliation, stalled transitions, generation takeover, and concurrent execution. The
synthetic example worker must also cover remote commit before local outcome persistence, restart
with a higher generation, legacy journal recovery, and remote-ledger persistence failure. Add real
product remote-upload/create integration, interrupt, and kernel-cache VM tests only with the
matching durable identity, transaction, transport, and invalidation mechanisms.

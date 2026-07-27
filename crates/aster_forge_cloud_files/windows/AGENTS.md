# AGENTS.md

This file supplements [`../../../AGENTS.md`](../../../AGENTS.md) and applies to
`aster_forge_cloud_files_windows`.

## Ownership

- This crate owns Windows Cloud Files/CFAPI mappings: native identity encoding, Windows filename
  validation, owned callback/request snapshots, CFAPI structures, callback lifecycle, completion
  mapping, sync-root connection state, and physical placeholder/materialization operations.
- `aster_forge_cloud_files_core` owns product-neutral identity, revision, capability, hydration,
  mutation, content-storage, upload, eviction, and durable-store contracts.
- Product crates own remote APIs, authentication, account/root policy, DTOs, repositories,
  permissions, user-visible conflicts, packaging, registration policy, and update behavior.

## Boundary Rules

- Native identity is an adapter mapping for `CloudItemKey`; paths and filenames never become the
  stable core identity.
- Validate the encoded identity against CFAPI's 4 KiB limit after encoding.
- Keep all native pointers local to the FFI call that consumes them. Public structs own their
  strings and bytes and must not retain borrowed callback pointers.
- Callback threads perform bounded ingress only. Network I/O, database transactions, and remote
  reconciliation run outside the callback.
- Every asynchronous CFAPI request must have one terminal completion across success,
  cancellation, queue rejection, timeout, session closing, and provider shutdown races.
- Windows-only bindings stay behind `cfg(windows)` so product-neutral models and codec tests run on
  every development host.
- Do not import AsterDrive DTOs, endpoint semantics, account types, error text, or database
  entities.

## Validation

```bash
cargo fmt --all -- --check
cargo test -p aster_forge_cloud_files_windows
cargo clippy -p aster_forge_cloud_files_windows --all-targets -- -D warnings
cargo check -p aster_forge_cloud_files_windows --target x86_64-pc-windows-gnu
cargo check --workspace
```

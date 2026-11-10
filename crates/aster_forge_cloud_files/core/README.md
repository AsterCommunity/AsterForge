# aster_forge_cloud_files_core

Product-neutral contracts for native cloud-files integrations.

This crate defines the operating-system-independent value model shared by Windows Cloud Files,
Apple File Provider, and Linux FUSE adapters. Product crates provide authentication, remote API
DTO mapping, permissions, root/account policy, and user-visible conflict behavior.

The current Phase 1 surface includes scoped identities, opaque revisions and cursors,
capability negotiation, read-only backend ports, durable checkpoint/mutation store contracts,
runtime-neutral hydration work sharing with waiter-scoped cancellation, and revision-bound
content-storage ownership with guarded eviction, recoverable provider-cache writes, immutable
local generations, resumable upload recovery, a runtime-neutral upload runner, and a generic
durable mutation runner with remote-outcome reconciliation. Native platform validation has
started in the sibling platform crates while these product-neutral contracts remain
platform-independent.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_cloud_files_core = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_cloud_files_core)
- [Rust API reference](https://docs.rs/aster_forge_cloud_files_core/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

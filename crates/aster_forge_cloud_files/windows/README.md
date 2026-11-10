# aster_forge_cloud_files_windows

Windows Cloud Files bindings for the product-neutral Forge cloud-files core.

This crate owns CFAPI-specific item/sync-root identity encoding, placeholder metadata, filename
validation, persistent registration policies, active connection/session fencing, owned
`FETCH_DATA`/`CANCEL_FETCH_DATA` snapshots, bounded callback ingress, active hydration waiter
cancellation, and native calls. Product authentication, remote DTOs, account policy,
persistence adapters, and user-visible error mapping remain in product crates.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_cloud_files_windows = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_cloud_files_windows)
- [Rust API reference](https://docs.rs/aster_forge_cloud_files_windows/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

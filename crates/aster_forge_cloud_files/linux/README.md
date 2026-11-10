# aster_forge_cloud_files_linux

Product-neutral Linux FUSE bindings for Forge cloud-files core.

This crate owns stable inode/generation mappings, directory-handle snapshots, file-handle
revision fences, direct-I/O writeback, durable namespace mutation acceptance, remote-change
overlays, kernel invalidation plans, mount-generation recovery, FUSE reply mapping, and bounded
callback-to-async dispatch. Product crates retain identity allocation, backend adapters,
durable inode/content storage, change cursors, remote workers, daemon/service packaging, mount
UX, authentication, permissions, and user-visible errors.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_cloud_files_linux = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_cloud_files_linux)
- [Rust API reference](https://docs.rs/aster_forge_cloud_files_linux/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

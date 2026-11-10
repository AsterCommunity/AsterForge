# aster_forge_cloud_files_macos_bridge

Product-neutral Rust bridge contracts for Apple File Provider extensions.

This crate owns persistent identifier/version mapping, owned enumeration results,
extension-session fencing, and the narrow C ABI used by a thin Swift extension shell. Swift
retains all `NSFileProvider*` objects and completion handlers. Product crates retain backend
adapters, authentication, persistence, domain policy, packaging, signing, and user-visible
errors.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_cloud_files_macos_bridge = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_cloud_files_macos_bridge)
- [Rust API reference](https://docs.rs/aster_forge_cloud_files_macos_bridge/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

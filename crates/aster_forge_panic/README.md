# aster_forge_panic

Shared panic hook and crash report writer for Aster services.

This crate implements the full crash-reporting behavior used by Aster
services: a process-wide panic hook, a lazily opened crash log, backtrace
capture for developer diagnostics, user-facing stderr notices, and a
repository issue target. Product crates provide names, versions, repository
URLs, and crash log paths through `PanicHookConfig`.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_panic = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_panic)
- [Rust API reference](https://docs.rs/aster_forge_panic/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

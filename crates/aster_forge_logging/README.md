# aster_forge_logging

Shared tracing subscriber setup for Aster services.

This crate provides the complete logging initialization behavior used by the application
repositories: stdout or file output, optional daily rotation, bounded retained log files,
`RUST_LOG` precedence over configured levels, text or JSON formatting, debug-build file and line
annotations, and a non-blocking writer guard. Applications can use `LoggingConfig` directly
in their deployment configuration schema.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_logging = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_logging)
- [Rust API reference](https://docs.rs/aster_forge_logging/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

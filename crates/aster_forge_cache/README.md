# aster_forge_cache

Shared cache abstractions and backend constructors for Aster services.

The public API is byte-oriented so cache backends can remain object-safe and easy to wrap in
`Arc<dyn CacheBackend>`. JSON convenience methods are provided as an extension trait for common
application values, while concrete memory and Redis implementations live behind feature-gated
modules.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_cache = "=0.1.0-alpha.1"
```

## Features

Default features: `memory`.

Available features: `bloom`, `memory`, `redis`, `runtime-component`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_cache)
- [Rust API reference](https://docs.rs/aster_forge_cache/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

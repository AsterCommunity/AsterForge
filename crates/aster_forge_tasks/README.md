# aster_forge_tasks

Shared background task primitives for Aster services.

This crate owns product-neutral task mechanics: step state transitions, typed payload/result
serialization, retry classification, erased task-spec adapters, registry generation, runtime
worker loops, lease guards, heartbeat loops, lane claiming, dispatch aggregation, drain loops,
and task artifact temporary-directory helpers. It deliberately does not own database entities,
`SeaORM` repositories, product task kind enums, runtime configuration, metrics labels, or concrete
task implementations. Product crates keep those boundaries and register their specs and storage
adapters explicitly.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_tasks = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`, `runtime`, `runtime-component`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_tasks)
- [Rust API reference](https://docs.rs/aster_forge_tasks/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

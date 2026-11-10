# aster_forge_events

Product-neutral transient event transport primitives.

This crate owns broker connection lifecycle, reconnect backoff, shutdown cancellation, and
raw payload delivery. Products own payload schemas, authorization, origin filtering, and the
local event semantics built on top of the transport.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_events = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `redis`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_events)
- [Rust API reference](https://docs.rs/aster_forge_events/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

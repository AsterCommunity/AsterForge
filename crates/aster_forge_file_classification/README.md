# aster_forge_file_classification

File extension parsing and category classification helpers.

This crate turns filenames, MIME hints, and extension filter strings into stable high-level
categories used by API filtering and UI grouping. It intentionally avoids product-specific enum
derives so services can map categories into their own database or `OpenAPI` representations.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_file_classification = "=0.1.0-alpha.1"
```

## Features

Default features: none.

Available features: `openapi`, `sea-orm`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_file_classification)
- [Rust API reference](https://docs.rs/aster_forge_file_classification/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

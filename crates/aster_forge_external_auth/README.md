# aster_forge_external_auth

Feature-gated external authentication provider drivers for Aster services.

This crate contains the provider-neutral external authentication contract plus reusable
implementations for `OpenID` Connect, generic `OAuth2`, and selected fixed-endpoint providers. It
deliberately avoids application database models, account entities, and HTTP handler concerns:
product crates map their stored provider rows into `ExternalAuthProviderConfig` and map
`ExternalAuthError` into their own API error type. Built-in connectors are controlled by Cargo
features so each backend can compile only the providers it supports. The default feature set
enables `oidc` and generic `oauth2`; dedicated connectors such as `github`, `google`,
`microsoft`, and `qq` must be enabled explicitly.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_external_auth = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Features

Default features: `oidc`, `oauth2`, `rustls-aws-lc-rs`.

Available features: `github`, `google`, `microsoft`, `oauth2`, `oidc`, `openapi`, `qq`, `sea-orm`, `rustls-aws-lc-rs`, `rustls-ring`, `native-tls`. Select features according to the integration documentation.

HTTP encodings are explicit; the crypto dependency selects only SHA-256. To replace TLS, disable defaults and select provider and backend features together. Applications using ring must initialize its provider at startup. These backend controls are planned for `0.1.0-alpha.2`, which is not yet published.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_external_auth)
- [Rust API reference](https://docs.rs/aster_forge_external_auth/0.1.0-alpha.2)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

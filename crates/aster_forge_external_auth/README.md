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
aster_forge_external_auth = "=0.1.0-alpha.1"
```

## Features

Default features: `oidc`, `oauth2`.

Available features: `github`, `google`, `microsoft`, `oauth2`, `oidc`, `openapi`, `qq`, `sea-orm`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_external_auth)
- [Rust API reference](https://docs.rs/aster_forge_external_auth/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

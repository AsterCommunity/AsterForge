# aster_forge_auth

Product-neutral local session authentication mechanics.

`aster_forge_auth` owns JWT token contracts, validation policy, token-pair issuance, and the
refresh-rotation coordinator. Product repositories own account state, database transactions,
session tables, cookies, permissions, audit events, and error presentation. Persistence is
accessed through `RefreshSessionStore`, so this crate does not depend on `SeaORM` or a product
schema.

Select exactly one Cargo backend feature: `aws-lc-rs` (default) or `rust-crypto`.
The product entrypoint must call `initialize_jwt_crypto_provider` before any JWT use,
including direct `jsonwebtoken` calls. This installs a process-wide provider, not an
instance-specific backend. Signing algorithms remain a runtime `JwtPolicy` choice.

```
use aster_forge_auth::{AuthTokenType, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
Restriction, SessionId, SessionVersion, Subject};

aster_forge_auth::initialize_jwt_crypto_provider()?;
let key = JwtKey::hmac([42; 32])?; // Product loads a dedicated random signing key.
let policy = JwtPolicy::new("product", "local-web", 900, 604_800)?;
let signer = JwtSigner::with_system_clock(key.clone(), policy.clone());
let verifier = JwtVerifier::with_system_clock(key, policy);
let pair = signer.issue(Subject::new("42")?, SessionId::new("session-1")?,
SessionVersion::new(1), Restriction::None)?;
let verified = verifier.verify(&pair.access_token, AuthTokenType::Access)?;
assert_eq!(verified.claims().sub.as_str(), "42");
// Product persists the session before publishing pair, and checks current authorization.
```

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

```toml
[dependencies]
aster_forge_auth = "=0.1.0-alpha.1"
```

## Features

Default features: `aws-lc-rs`.

Available features: `aws-lc-rs`, `password`, `rust-crypto`. Select features according to the integration documentation.

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_auth)
- [Rust API reference](https://docs.rs/aster_forge_auth/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

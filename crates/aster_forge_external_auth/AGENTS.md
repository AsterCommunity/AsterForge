# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_external_auth`. See [`../../docs/crates/aster_forge_external_auth.md`](../../docs/crates/aster_forge_external_auth.md) for provider contracts, features, profile trust semantics, and the product integration boundary.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, `driver.rs`, `types.rs`, the target provider, the registry, and `tests/registry.rs`.
- Provider protocols and endpoints can drift. Check official provider, OIDC, or OAuth specifications before implementation, and keep product account-binding logic out of this crate.

## Ownership Boundaries

- This crate owns the provider-neutral driver, OIDC and OAuth2 mechanics, built-in connectors, registry, configuration normalization, and generic profile and result types.
- Products own provider tables and migrations, credential encryption, local-user creation and binding, sessions, audit, callback routes, return-URL policy, and final error text.
- The `sea-orm` feature adds persistence mappings for stable kinds and protocols only; it does not take ownership of product entities.

## Change Constraints

- Keep explicit validation boundaries for state, PKCE/verifiers, nonce, return paths, issuers and endpoints, and outbound HTTP failures.
- A provider profile's `email_verified` flag is a trust contract. Set it only when the upstream provider supplies evidence the product can rely on, not merely because an email field exists.
- Provider kind and protocol wire and DB string values are compatibility contracts. Adding a connector requires checking its feature, descriptor, registry entry, and round-trip behavior together.
- The default registry exposes only connectors enabled by features. Consumer code injects a product-specific User-Agent.
- Shared errors must remain classifiable. Do not include access tokens, client secrets, raw callbacks, or sensitive provider responses in Display output or logs.

## Validation

```bash
cargo test -p aster_forge_external_auth --no-default-features
cargo test -p aster_forge_external_auth
cargo test -p aster_forge_external_auth --all-features
cargo clippy -p aster_forge_external_auth --all-targets --all-features -- -D warnings
```

For each provider, cover feature presence, missing fields, option parsing, state/nonce/PKCE, profile normalization, verified-email semantics, and secret redaction.

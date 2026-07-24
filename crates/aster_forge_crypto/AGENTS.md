# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_crypto`. See [`../../docs/crates/aster_forge_crypto.md`](../../docs/crates/aster_forge_crypto.md) for the current helpers and error boundary.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, `src/hash.rs`, the crate documentation, and fixed-vector tests.
- Before adding a primitive, confirm that at least two products share the same mechanical contract. Key lifecycle and product policy do not enter this crate merely because they are also cryptography-related.

## Ownership Boundaries

- This crate owns Argon2 password hashing and verification, SHA-256 digest and hex helpers, and a narrow, mappable error surface.
- Products own password strength, legacy-hash migration, login lockout, reset tokens, pepper and secret management, KMS integration, and user-facing messages.

## Change Constraints

- Use mature upstream primitives. Do not invent cryptographic algorithms, encodings, or randomness schemes.
- Verification behavior must not leak additional account state. Collapse implementation errors into `CryptoError`, then let products map presentation.
- Hash output, parameter upgrades, and wire/storage compatibility are persistence contracts. Changes require a strategy for verifying and migrating old values.
- Digest helpers must define byte inputs and output size explicitly, and hex casing must remain stable.

## Validation

```bash
cargo test -p aster_forge_crypto
cargo clippy -p aster_forge_crypto --all-targets -- -D warnings
```

Cover correct and incorrect passwords, malformed hashes, random salts, fixed SHA-256 vectors, empty inputs, and binary inputs.

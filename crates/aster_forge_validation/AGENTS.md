# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_validation`. See [`../../docs/crates/aster_forge_validation.md`](../../docs/crates/aster_forge_validation.md) for display, email-policy, and filename contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the target module, the crate documentation, and existing boundary tests.
- Decide whether the rule is cross-product input validation or product policy. For example, email syntax can be shared, while deciding which domains may register remains product-owned.

## Ownership Boundaries

- This crate owns display-text and public-asset URL handling, email normalization, email allow/block entry parsing, filename normalization, and generic copy-name mechanics.
- Products own username and password policy, branding defaults, allowlist switches and precedence, uniqueness, permissions, error codes, and localization.
- Internal object-key safety belongs to `aster_forge_storage_core`; do not use filename validation as a substitute.

## Change Constraints

- Every length limit must state whether it counts bytes or characters. Truncation must preserve UTF-8 boundaries.
- Keep strict write normalizers separate from fail-soft runtime readers. Falling back for bad historical values must not weaken validation for new writes.
- Preserve exact email and domain matching semantics. Do not add suffix or wildcard matching implicitly.
- Filename normalization, reserved names, invalid characters, and copy suffixes must remain stable across platforms.
- `ValidationError` remains product-mappable and must not hard-code HTTP status, field paths, or localized envelopes.

## Validation

```bash
cargo test -p aster_forge_validation
cargo clippy -p aster_forge_validation --all-targets -- -D warnings
```

Cover Unicode normalization, multibyte boundaries, control characters, whitespace, Windows reserved names, copy-name limits, email case and domain extraction, exact-domain matching, and public asset URL schemes, paths, and whitespace.

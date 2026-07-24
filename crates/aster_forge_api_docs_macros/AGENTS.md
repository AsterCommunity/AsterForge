# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to the `aster_forge_api_docs_macros` proc-macro crate. See [`../../docs/crates/aster_forge_api_docs_macros.md`](../../docs/crates/aster_forge_api_docs_macros.md) for the behavior contract.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, `tests/path.rs`, and the crate documentation.
- Check how consuming crates forward the OpenAPI feature. Validate macro expansion in caller context; staring at a `TokenStream` until it looks plausible is not validation.

## Ownership Boundaries

- This crate provides product-neutral attribute macros so one route annotation becomes `utoipa::path` under debug plus `openapi`, while other builds preserve the original item.
- OpenAPI document assembly, path and schema lists, tags, security schemes, and product routes belong to consumers.
- Do not make macros guess the handler framework, inject product metadata, or alter function semantics.

## Change Constraints

- The non-OpenAPI path must return the item transparently without changing attributes, visibility, signatures, or diagnostic locations.
- Preserve caller tokens where possible and point errors at the annotation written by the user.
- Add a new macro only for a mechanical annotation pattern genuinely shared by multiple products. Do not move a product documentation DSL into Forge.
- The feature/debug combinations are part of the public build contract; consider all four combinations.

## Validation

```bash
cargo test -p aster_forge_api_docs_macros
cargo test -p aster_forge_api_docs_macros --features openapi
cargo check -p aster_forge_api_docs_macros --release --features openapi
cargo clippy -p aster_forge_api_docs_macros --all-targets --all-features -- -D warnings
```

When route annotations change, also run OpenAPI generation or drift tests in a real product or generated template.

# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_api`. Treat [`../../docs/crates/aster_forge_api.md`](../../docs/crates/aster_forge_api.md) as the source of truth for the API inventory and pagination, cursor, and PATCH examples.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the crate documentation, and existing tests. Confirm that a new type is genuinely reusable across Actix, Axum, OpenAPI, and test fixtures.
- If a capability depends on SeaORM, Actix, or concrete product DTOs, place it in `aster_forge_db`, an Actix crate, or the product repository respectively.

## Ownership Boundaries

- This crate owns framework-neutral typed REST envelopes, pagination queries and responses, cursor-parameter completeness checks, overfetch slicing, generic sort direction, and three-state PATCH values.
- It does not own database queries, product field allowlists, default sort policy, permission filtering, HTTP status codes, error text, or product entities.
- `SortOrder` is a shared API/DB contract. Do not create another structurally identical enum in DB or product code.

## Change Constraints

- Public types must not depend on a specific web framework or ORM.
- Cursor helpers validate generic structure only; they must not guess product indexes or sorting semantics.
- Handle limit, offset, length, and integer-conversion boundaries explicitly. Never truncate or lose sign silently.
- Keep OpenAPI derives consistent with the existing debug-plus-feature pattern so release builds do not absorb documentation-generation overhead.
- Serde wire shapes are compatibility contracts. Changes to field names, defaults, or three-state behavior require serialization tests and documentation updates.
- Keep `lib.rs` focused on modules and public exports. Errors, cursors, pagination, PATCH fields, schema bounds, sorting and envelopes live in their respective files; tests stay beside the owning mechanism.
- Envelope code classification comes from the product. Never supply business codes, HTTP mappings, retryability or diagnostic policy from the shared core. Validate success/error state on construction and deserialization.
- Register full generic envelope instantiations in OpenAPI; Utoipa type aliases can erase generic arguments and collide. Validate actual components, references and generated SDKs, including multiple product codes and DTOs.

## Validation

```bash
cargo test -p aster_forge_api
cargo test -p aster_forge_api --features openapi
cargo clippy -p aster_forge_api --all-targets --all-features -- -D warnings
```

Focus on incomplete cursor tuples, limit and offset extremes, overfetch next cursors, stable sort serialization, and omitted/null/value PATCH states.

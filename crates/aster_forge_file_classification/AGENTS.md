# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_file_classification`. See [`../../docs/crates/aster_forge_file_classification.md`](../../docs/crates/aster_forge_file_classification.md) for category mappings, extension parsing, and feature behavior.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the crate documentation, and existing classification tests.
- Before adding a category or extension, decide whether it is a stable cross-product meaning or a product policy belonging to a previewer, transcoding task, or UI icon.

## Ownership Boundaries

- This crate owns pure classification from filenames and MIME hints into stable high-level categories, extension and compound-extension extraction, and filter normalization.
- Products own preview permissions, processing queues, storage policy, icons, user-facing text, and MIME-sniffing security decisions.
- The `openapi` and `sea-orm` features add representations for the same stable enum; do not create structurally identical product enums.

## Change Constraints

- Lowercase Serde and DB values for `FileCategory`, together with `FILE_CLASSIFICATION_STORAGE_LEN`, are persistence contracts.
- A filename is only a classification hint. Never treat an extension result as a trusted content type or execution permission.
- Preserve existing safety behavior for path-like input, whitespace, punctuation, Unicode, and compound extensions.
- Enforce filter-count and filter-length limits before allocation or aggregation.

## Validation

```bash
cargo test -p aster_forge_file_classification
cargo test -p aster_forge_file_classification --all-features
cargo clippy -p aster_forge_file_classification --all-targets --all-features -- -D warnings
```

New mappings should cover case differences, missing extensions, dotfiles, path-like input, compound extensions, MIME fallback, Serde round trips, and SeaORM round trips.

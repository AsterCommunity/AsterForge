# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_storage_core`. See [`../../docs/crates/aster_forge_storage_core.md`](../../docs/crates/aster_forge_storage_core.md) for object-key and S3-configuration contracts.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, `object_key.rs`, `s3_config.rs`, and the crate documentation.
- Before adding an abstraction, decide whether it is a safety primitive shared by every storage backend or Drive-specific connector, capability, or upload policy. The latter stays product-owned.

## Ownership Boundaries

- This crate owns safe relative object-key and prefix normalization, joining and stripping, and S3-compatible endpoint and bucket parsing.
- Products own driver traits and lifecycle, credentials, region and path-style policy, multipart and presigned workflows, connector descriptors, capability negotiation, and upload business logic.
- User-visible filenames use `aster_forge_validation::filename`; never treat them as internal object keys.

## Change Constraints

- Keys must reject absolute paths, dot-segment escapes, platform-separator ambiguity, and storage-root escape. Preserve the distinction between empty-prefix and empty-object-key semantics.
- Join and strip helpers must not bypass normalization through raw string concatenation.
- Endpoints accept explicit absolute HTTP(S) URLs only and reject query strings and fragments. Missing buckets remain structured errors.
- Do not hard-code provider-specific credentials, regions, or host-style policies in core.
- Keep errors product-neutral so product services can map API and configuration presentation.

## Validation

```bash
cargo test -p aster_forge_storage_core
cargo clippy -p aster_forge_storage_core --all-targets -- -D warnings
```

Cover empty and root values, repeated separators, `.` and `..`, encoding and Unicode, prefix round trips, endpoint trailing slashes, queries and fragments, IPv4/IPv6 and ports, and real MinIO, R2, COS, and AWS configuration examples.

# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_webdav`. See [`../../docs/crates/aster_forge_webdav.md`](../../docs/crates/aster_forge_webdav.md) for protocol ownership, backend ports, errors, and the test matrix. See [`../../docs/crates/aster_forge_xml.md`](../../docs/crates/aster_forge_xml.md) for the underlying XML contract.

## Before Making Changes

1. Read `Cargo.toml`, `src/lib.rs`, the target protocol module, and the corresponding `tests/*.rs` files.
2. For XML changes, first determine whether `aster_forge_xml` should provide a generic upstream primitive. Keep only DAV grammar, selection, and response composition here.
3. Use downstream projects only to validate adapters. Authentication, permissions, workspace scope, storage, quota, persistence, and audit remain product-owned.

## Ownership Boundaries

- This crate owns the WebDAV and DeltaV protocol engine: path, header, and body parsing; preconditions; LOCK; properties; COPY/MOVE/DELETE/PUT/GET/HEAD planning; response grammar; backend ports; events; and the optional Actix adapter.
- `aster_forge_xml` owns bounded parsing, streaming, writing, safety limits, and namespace primitives. This crate owns DAV XML semantics.
- Products own Basic account authentication, rate limiting, principals and workspace permissions, file/blob/quota/storage policy, dead-property and lock persistence, business transactions, audit, and notifications.

## Change Constraints

- The transport-neutral core must not depend on Actix. The Actix layer performs explicit conversions for HTTP types, streams, and bodies.
- `DavBackendErrorKind` compresses only classifications required by the protocol. Keep detailed product errors in product logs and never leak product envelopes.
- Follow protocol contracts for path percent-decoding, dot segments, mount escape, same-origin `Destination`, `If`, ETag and lock-token handling, and `Depth` and `Overwrite` precedence.
- Decide empty, bounded, streaming, or unused body policy before reading. Bound XML size and depth, and never buffer an upload stream as one complete body.
- Keep multi-resource mutation results typed across 207/201/204 and partial failures; do not infer status from strings.
- Handle unknown XML extensions according to each method's grammar. Reject DTD and ENTITY, QName or namespace conflicts, and duplicated mutually exclusive controls.
- Backend traits expose only protocol-required ports. Do not absorb product repositories, authentication context, or storage-driver details.

## Validation

```bash
cargo test -p aster_forge_webdav
cargo test -p aster_forge_webdav --features actix
cargo clippy -p aster_forge_webdav --all-targets --all-features -- -D warnings
```

Run the relevant matrix for path escape, header grammar, conditional precedence, ranges, LOCK and `If`, property XML, partial failures, and Actix body policy. Run `aster_forge_xml` tests for XML primitives, then validate complete compatibility in product Litmus, rclone, curl, or cadaver tests.

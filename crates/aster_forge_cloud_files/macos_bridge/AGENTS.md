# AGENTS.md

This file supplements [`../../../AGENTS.md`](../../../AGENTS.md) and applies to
`aster_forge_cloud_files_macos_bridge`. Treat
[`../../../docs/crates/aster_forge_cloud_files_macos_bridge.md`](../../../docs/crates/aster_forge_cloud_files_macos_bridge.md)
as the public integration reference. The File Provider architecture and platform evidence live in
[`../../../tmp/ad-sync-client-cloud-files-foundation.md`](../../../tmp/ad-sync-client-cloud-files-foundation.md)
and [`../../../tmp/cloud-files-platform-contract-matrix.md`](../../../tmp/cloud-files-platform-contract-matrix.md).

## Ownership

- This crate owns Apple File Provider adapter mechanics: persistent identifier encoding, system
  container classification, item-version mapping, owned enumeration results, extension-session
  fencing, narrow C ABI ownership, panic containment, and Swift completion/error classifications.
- `aster_forge_cloud_files_core` owns product-neutral identity, revisions, backend contracts,
  hydration, mutation, content-storage, upload, eviction, and durable-store contracts.
- The Swift extension owns `NSFileProvider*` objects, completion handlers, `Progress`, domain
  registration, entitlements, App Group access, and framework callback conformance.
- The checked-in Swift package and CMake fixture are synthetic adapter tests. They must remain
  product-neutral and must not become the owner of production signing or remote account policy.
- Product crates own remote APIs, authentication, account/domain mapping, repositories,
  permissions, packaging, signing, installation, update policy, and user-visible errors.

## Boundary Rules

- `CloudItemKey` remains the stable scoped identity. File Provider identifiers are versioned
  adapter encodings and never contain a filename or local path.
- The current reversible identifier envelope is not confidentiality protection. Only encode
  non-sensitive opaque IDs; sensitive product identities require a durable random-token mapping.
- Keep metadata and content versions separate and preserve their opaque bytes exactly.
- Keep sync anchors opaque and at most 500 bytes. The adapter validates transport shape but never
  interprets product cursor contents or derives an anchor from paths, timestamps, or filenames.
- Change batches keep updates and deletions distinct. Reject duplicate identifiers and an
  identifier appearing in both sets; preserve `moreComing` exactly.
- Materialized reconciliation snapshots the native anchor before full pagination, then applies
  every anchored change batch through `moreComing == false`. Persist directories only.
- App Group IDs and directories are caller inputs. The generic file store may persist adapter
  state, but it must not select product paths, account mappings, or production storage policy.
- A replicated extension signals only the working-set container. Do not invent parent-container
  signaling that the macOS framework ignores.
- Root and Apple system containers are distinct identifier classes. A system container is not a
  synthetic product item and must not be decoded as `CloudItemKey`.
- Rust never stores Swift/Objective-C object pointers or completion blocks. Swift converts native
  requests into owned bytes/strings before entering Rust and converts owned Rust results back.
- Each FFI allocation has one matching release function. Null, length, alignment, integer
  conversion, stale session, duplicate release, panic, and closing paths require deterministic
  tests.
- Every accepted request has a non-cloneable session lease. Closing rejects new requests; the
  final lease release closes a disconnected session.
- Keep `unsafe` at the smallest FFI expression and attach an exact `SAFETY:` explanation. Do not
  move platform-independent engine logic into an unsafe block.
- Do not import AsterDrive DTOs, endpoints, account state, database entities, Finder text, or
  signing policy.

## Validation

```bash
cargo fmt --all -- --check
cargo test -p aster_forge_cloud_files_macos_bridge --all-targets
cargo clippy -p aster_forge_cloud_files_macos_bridge --all-targets -- -D warnings
MIRIFLAGS='-Zmiri-strict-provenance' \
  cargo +nightly-2026-04-12 miri test --locked -p aster_forge_cloud_files_macos_bridge
swift test --disable-sandbox \
  --package-path crates/aster_forge_cloud_files/macos_bridge/swift
cmake \
  -S crates/aster_forge_cloud_files/macos_bridge/swift-fixture \
  -B /tmp/aster-forge-macos-fixture \
  -G Xcode \
  -DASTER_FORGE_CODE_SIGNING_ALLOWED=NO
cmake --build /tmp/aster-forge-macos-fixture --config Debug \
  --target AsterForgeCloudFilesFixtureHost AsterForgeCloudFilesMacosMemoryCloudDrive
ctest --test-dir /tmp/aster-forge-macos-fixture -C Debug --output-on-failure
# Optional final Apple-native acceptance; requires a development identity.
crates/aster_forge_cloud_files/macos_bridge/swift-fixture/scripts/run-system-test.sh
cargo check --workspace
```

Swift changes require XCTest coverage; compilation-only smoke is insufficient. Keep SwiftPM,
CMake/Xcode build trees, DerivedData, user state, and `LocalSigning.cmake` ignored by the nearest
directory-level `.gitignore`. The standalone `macos_memory_cloud_drive` example is the daily
end-to-end path for the Swift shell, Rust C ABI, memory data source, enumeration, fetch, change feed,
materialized working set, and CLI error boundaries. The signed system-test runner is optional final
Apple-native acceptance for domain registration, Finder enumeration, system hydration/eviction,
extension termination, and App Group recovery.

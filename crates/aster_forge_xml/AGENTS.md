# AGENTS.md

This file supplements [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_xml`. See [`../../docs/crates/aster_forge_xml.md`](../../docs/crates/aster_forge_xml.md) for source-backed arenas, stream readers and writers, safety policy, compatibility tests, and benchmark interpretation.

## Before Making Changes

- Read `Cargo.toml`, `src/lib.rs`, the target parser, document, stream, or writer module, and the relevant `tests/*.rs` files.
- For WebDAV grammar, implement only generic XML capabilities here. DAV element selection and status mapping stay in `aster_forge_webdav`.
- Before performance work, define the semantic baseline and comparison workload. Do not advertise a lower-bound benchmark as product throughput.

## Ownership Boundaries

- This crate owns bounded XML validation, source-backed flat arenas, owned and borrowed documents, namespace resolution, stream reading, stream writing, and resource-safety policy.
- It does not own WebDAV, WOPI, COS, or other business grammars, product DTOs, HTTP errors, or protocol status codes.

## Change Constraints

- Untrusted input is bounded simultaneously by bytes, depth, elements, attributes, text, and events. Reject exact limit violations before building an unrestricted tree.
- Reject DTD and custom entities, multiple roots, trailing garbage, and invalid encodings by default. Any relaxation is a security-contract change.
- Deep-document parsing, traversal, and drop remain non-recursive to prevent stack overflow. Streaming paths must not construct a DOM implicitly.
- Preserve source-span, borrowed-`Cow`, and owned-value-pool allocation semantics: allocate only when decoding or normalization changes a value.
- Preserve the existing node model for default namespaces, prefix shadowing, undeclaration, and attribute namespaces.
- The writer validates names, namespace bindings, characters, document lifecycle, and output limits. Classify I/O failures without corrupting internal state.
- Benchmarks must state semantic differences. `xmltree` and `roxmltree` remain test references rather than production dependencies.

## Validation

```bash
cargo test -p aster_forge_xml
cargo clippy -p aster_forge_xml --all-targets -- -D warnings
```

Parser or writer changes require compatibility tests, property and proptest regressions, exact depth and count boundaries, mixed content, namespaces, exact source ranges, selective stream capture, I/O failures, and non-recursive 20k/25k-depth cases. Run documented benchmarks and probes only when making performance claims, and record the environment.

# aster_forge_macros

Product-neutral declarative macros. The crate currently provides `wire_enum!`, which generates known-value mappings, an `Unknown(String)` variant, Serde support, and owned `String` conversions for string-valued protocol enums.

It does not define product status names, protocol decisions, validation methods, or runtime behavior. It is a declarative macro crate, not a procedural macro, and has no production dependencies. Consumers must depend directly on `serde` with its `derive` feature enabled.

```rust
use aster_forge_macros::wire_enum;

wire_enum! {
    /// Product-owned status.
    pub enum Status {
        /// Ready to proceed.
        Ready => "ready",
    }
}

assert_eq!(Status::from("future".to_owned()), Status::Unknown("future".into()));
```

Mappings are case-sensitive; empty strings, Unicode, and other unknown values are preserved verbatim. Non-string inputs are rejected during deserialization. `Unknown` is a reserved variant name, at least one known variant is required, and duplicate wire values fail at compile time. Known-variant metadata is used for documentation. Per-variant `cfg` attributes and custom Serde representations are not supported; apply `cfg` to the entire macro invocation when conditionally enabling it. Consumers remain responsible for bounding response bodies, deciding whether an unknown value is executable, and mapping product errors. The generated Serde paths require the consumer dependency to be named `serde`.

```sh
cargo test -p aster_forge_macros --locked
cargo clippy -p aster_forge_macros --all-targets --locked -- -D warnings
```

Tests cover known and unknown string round-trips, case sensitivity, empty values, Unicode, long values, control characters, rejection of non-string inputs, path hygiene, and compile-fail contracts. See [`docs/crates/aster_forge_macros.md`](../../docs/crates/aster_forge_macros.md) for the full integration guide.

License: MIT OR Apache-2.0.

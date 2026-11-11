# aster_forge_panic

Shared panic hook and crash report writer for Aster services.

Executable-only, first-install panic reporting with UTC timestamps, private
full reports and separate human, silent or product-defined diagnostic output.
`ReportWriter` lets products enforce their own secure storage/redaction policy;
`DiagnosticSink` lets CLIs preserve their JSON envelope. Public diagnostics never
receive raw payloads, backtraces, locations, file paths or writer error text.

**Unreleased API:** the example and policies below apply to PR #65, not the published alpha.1.

```rust
use aster_forge_panic::{install_panic_hook, PanicHookConfig, PanicOutput};

install_panic_hook(
    PanicHookConfig::new("my-cli", "1.0", "")
        .with_output(PanicOutput::Silent),
);
```

The default writer opens during installation. On Linux it rejects symlinks in all
path components, unsafe existing file permissions/ownership and hard links;
new directories/files use 0700/0600. Other platforms (including macOS, whose
ACLs are not validated here) require an injected writer for private ACLs. Callbacks must not panic or block. Concurrent panic reporting
is best-effort and skips events while another hook invocation is active.
Repeated installation never replaces a later host hook. No prior hook is chained.
This hook does not recover panics or decide process exit policy.

See the [CLI example](examples/cli.rs) for product-defined JSON output.

Part of [AsterForge](https://github.com/AsterCommunity/AsterForge), the product-neutral shared runtime foundation for Aster services.

## Installation

For the unreleased API shown above, use the PR branch (pin an audited commit
with `rev` when integrating; after merge, use the merged revision):

```toml
[dependencies]
aster_forge_panic = { git = "https://github.com/AsterCommunity/AsterForge", branch = "feat/issue-62-panic-cli-policy" }
```

The published alpha.1 has the earlier hook API and does **not** provide
`PanicOutput`, `DiagnosticSink`, or `ReportWriter`:

```toml
[dependencies]
aster_forge_panic = "=0.1.0-alpha.1"
```

## Documentation

- [Integration guide and ownership boundaries](https://forge.docs.astercosm.com/crates/aster_forge_panic)
- [Published alpha.1 API reference (earlier API)](https://docs.rs/aster_forge_panic/0.1.0-alpha.1)

This is an alpha release; public APIs may change. Product business rules, authorization and user-facing error mapping remain in the consuming application.

## License

Licensed under MIT OR Apache-2.0.

# CI maintenance

`workflows/rust.yml` is the push/PR entry point for Rust, template, Cargo,
toolchain, script and workflow changes. Documentation-only changes do not start
Rust CI. It creates a stable `Required Rust CI` check for matching changes. If
branch protection is enabled, require this check instead of conditional matrix
jobs. The gate fails when change detection fails, an applicable check
fails/cancels, or a selected suite is unexpectedly skipped.

`scripts/ci/changed-crates.py` uses Git changes and `cargo metadata --no-deps` to
select changed workspace members and their transitive reverse dependencies,
including optional, development, build and target-specific dependencies. PRs
use the merge base; pushes compare the exact before/after commits. Renames check
both owners. Shared manifests/lockfile/toolchains, Cargo settings and Rust
workflows conservatively select every crate. Known feature-test scripts select
their owning contracts; selector/policy and unknown scripts fall back to every
crate. Unknown/removed crate paths also fall back to every crate. Root
documentation changes skip Rust compilation.

| Workflow | Scope |
| --- | --- |
| `rust-crate.yml` | Reusable per-crate MSRV check, default-feature tests and Clippy; feature matrices stay in dedicated contracts or full validation |
| `rust-panic.yml` | Panic contract on Linux, macOS and Windows |
| `rust-process.yml` | Process contract on three OSes, plus nonzero-test Miri gate |
| `rust-auth.yml` | Exclusive JWT backends, platform tests and isolated consumers |
| `rust-http-crypto.yml` | Isolated HTTP/crypto consumer feature contracts |
| `rust-xml.yml` | XML fuzz harness build |
| `cloud-files.yml` | Selected Cloud Files packages, native platform fixtures and Miri |
| `template.yml` | Generated service checked against this checkout via a local Git-dependency patch |
| `rust-full.yml` | Weekly/manual workspace MSRV, coverage, complete feature matrix and all specialized workflows |

Template selection reads Forge dependencies from the template manifest, so a
panic change also verifies the generated service, while an unrelated Cloud Files
change does not. A Cloud Files core change selects all dependent platform crates;
a platform-only change selects that platform and its Miri tests.

Workspace coverage/Codecov reports run in full validation, not on every PR.
Documentation deployment and scheduled dependency auditing retain their own
workflows. New workspace members automatically join selection and generic crate
checks; add a dedicated workflow only for additional platform or feature rules.

The current SQLx 0.9.0 release constrains `libsqlite3-sys` to `<0.38.0`, while
RustSec RUSTSEC-2026-0459 requires `>=0.39.0`. `cargo upgrade --incompatible`
only upgrades the direct `jsonwebtoken` requirement and cannot resolve that
transitive SQLite advisory. The exception is recorded in `.cargo/audit.toml`
with its removal condition; delete it when SQLx or SeaORM ships a compatible
release and refresh `Cargo.lock`.

Validate changes locally:

```sh
python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v
actionlint
git diff --check
```

## Release automation

The standalone tool and workflow contract is documented in
[tools/forge-release/README.md](../tools/forge-release/README.md). Each manually
changed, unpublished member version is release intent. Dependency pins sync
without bumping downstream package versions; affected consumers still undergo
normal Rust CI.

Cargo validation jobs use the local sync-forge-dependencies composite action
before dependency resolution. These edits exist only in the runner checkout.
The release bot records the corresponding changes in a separate reviewable PR.
Forge Release Checks validates the tool on Linux/macOS/Windows and verifies a
changed release plan against normalized package archives. Publication requires
both Rust CI and Forge Release Checks to succeed for the exact merge SHA, plus
a same-repository, merged automation/forge-release PR that changes the plan.

One-time setup: configure ASTER_PACKAGE_PUBLISHER_APP_ID (Variable or Secret),
ASTER_PACKAGE_PUBLISHER_PRIVATE_KEY (Secret), and each
crate's trusted publisher for forge-release.yml with environment crates-io.
The App needs repository contents and pull requests write permissions; its
commits trigger ordinary CI. No long-lived crates.io token is required.
Security releases remain explicitly selected through maintainer version bumps.

Local checks:

```sh
cargo test --locked --manifest-path tools/forge-release/Cargo.toml --all-targets
cargo clippy --locked --manifest-path tools/forge-release/Cargo.toml --all-targets -- -D warnings
node --test .github/scripts/forge-release.test.cjs
actionlint
```

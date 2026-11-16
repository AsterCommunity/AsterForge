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
| `rust-crate.yml` | Reusable per-crate MSRV check, default/doc tests, no-default/each/all-feature tests and Clippy |
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

Validate changes locally:

```sh
python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v
actionlint
git diff --check
```

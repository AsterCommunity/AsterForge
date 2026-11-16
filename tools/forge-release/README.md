# forge-release

Repository-local Rust release automation, isolated from Forge's publishable
workspace. Cargo metadata owns package/dependency identities; toml_edit changes
only local dependency requirements, preserving features, comments and package
versions.

## Normal workflow

1. Change a crate's package.version in your normal feature/fix PR. This is release
   intent. Do not bump downstream package versions just because they depend on it.
2. Normal CI syncs local dependency pins in its disposable checkout. Its locked
   mode permits only existing local package version identities to change and
   rejects missing third-party lock entries rather than regenerating the lock,
   then tests the changed crates and their transitive consumers. Fix incompatible
   APIs in the owning repositories; bump additional packages when you intend to
   publish their changes too.
3. After main CI succeeds, Prepare Forge release PR queries crates.io and opens or
   updates automation/forge-release. Its PR records dependency-only manifest edits,
   Cargo.lock and release-plan.json. Existing versions, including yanked versions,
   are never treated as new release intent. Registry failures abort preparation.
4. Review and merge that PR. Both Rust CI and Forge Release Checks must succeed
   for the exact merge SHA. Publication also verifies that this SHA belongs to a
   merged release PR from this repository's automation branch which changes the
   release plan. A label or a plan file alone does not authorize publication.
5. The publisher checks the entire explicit batch, verifies normalized package
   archives, then uploads in dependency order, including versioned dev dependencies. Cargo performs its
   usual per-package build verification and registry visibility wait. The tool
   then re-queries the registry without its preflight cache before reporting
   completion. A failed
   upload stops the run; it does not infer new releases or change version numbers.

Keep source fixes in normal PRs on master; do not add hand-written source commits
to the generated release branch, which the bot refreshes. For a security fix,
maintainers choose which consumer versions to bump and release. The tool cannot
prove compatibility or force external applications to upgrade.

## One-time setup

- Repository or organization Variable/Secret ASTER_PACKAGE_PUBLISHER_APP_ID
  and Secret ASTER_PACKAGE_PUBLISHER_PRIVATE_KEY for a
  GitHub App installed on this repository with Contents and Pull requests write
  permissions. Organization settings must grant AsterForge access. App-created release PRs trigger normal CI (GITHUB_TOKEN-created
  PRs do not normally start another workflow).
- Configure each crate's crates.io Trusted Publisher for this repository, workflow
  forge-release.yml, and GitHub environment crates-io. The workflow exchanges
  GitHub OIDC for a temporary token only after the review and CI gates pass.
- Require the normal CI gates on master. No workflow auto-merges release PRs.

These settings are external prerequisites; adding workflow files does not set up
an App or authorize a crates.io publisher.

## Local commands

Run from the repository root:

```sh
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- check
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- plan \
  --changed aster_forge_http --format json
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- prepare --auto --dry-run
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- prepare --auto
```

For an explicit subset, use prepare --package NAME... instead of --auto. Prepare
refuses already-published versions and regressions within the same release line;
maintenance releases on older lines remain possible. It updates inherited,
renamed, target-specific and direct local path dependencies, including dev/build
requirements. It never increments a package.version. Concurrent manifest edits
are detected before writes; Cargo resolution failures leave edits visible for
repair and do not write a fresh release plan. Review the diff before committing.

```sh
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- validate
cargo run --locked --manifest-path tools/forge-release/Cargo.toml -- publish --dry-run
```

Dry-run creates .crate archives, extracts them, and compiles their normalized
contents with only other reviewed batch archives patched in. This avoids Cargo
1.95's multi-package temporary-registry checksum error without substituting
unpackaged workspace sources or uploading dependencies for a PR check.

For a rate limit or partial failure, honor the registry's retry time, then
manually rerun Publish Forge release plan with the full original merge SHA.
Already-published versions are skipped; yanked versions abort. Retrying checks the
same reviewed plan and CI evidence. No automatic busy retry loop or forced yank
is performed. Only the publish command without --dry-run can upload.

## Validation

Tests use synthetic graphs, temporary Cargo workspaces, a local HTTP registry,
and fake publishers. They cover dependency kinds and cycles, renamed and
platform-specific declarations, unpublished/private/yanked versions, registry
errors, concurrent edits, archive contents, partial publication retries and
exact-commit workflow gates. Forge Release Checks runs the Rust suite on three
operating systems. No test uploads a crate or needs production credentials.

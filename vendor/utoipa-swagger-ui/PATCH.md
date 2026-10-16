# Temporary upstream build fix

Source: crates.io `utoipa-swagger-ui` 10.0.1, MIT OR Apache-2.0.
The upstream HTTP integration sources and licenses are retained unchanged.

The manifest uses `zip` 9.0.3 instead of the withdrawn 8.6 release, and adds
local version metadata and an isolated workspace. The build script propagates
the now-fallible `ZipFile::name()` result required by zip 9. The upstream dependency on
`zip ^8.6` prevents fresh Cargo resolution, including when OpenAPI is disabled.
No application behavior or HTTP integration is changed.

Both the generated service template and Drive consume this package from Forge
Git. Return to crates.io once an upstream release supports a non-withdrawn zip
version; remove this directory and update both consumers together.

## zip source review (2026-10-16)

The maintainer attributes the mass withdrawal to security vulnerabilities, with
further disclosures pending: <https://github.com/zip-rs/zip2/issues/1075#issuecomment-6209381974>.
The three published advisories are GHSA-vv5m-jxxh-4627, GHSA-4v9h-p7m9-rh26,
and GHSA-4684-h2q9-2vcp. Do not restore the withdrawn releases from Git.

zip 9.0.3's registry archive SHA-256 is
`c2d98561b612a84ae525331a7a094429ef8a5772514ed507ff6db5f54badc971`.
Its VCS metadata matches official tag v9.0.3 at commit
`06b1fe7d51842772cbc31bd020e0a925428ecc22` (GitHub verified signature).
All 62 comparable packaged files match that tag byte for byte; Cargo.lock is
package-generated and Cargo.toml is Cargo-normalized. This establishes artifact
consistency, not the absence of source vulnerabilities or compromised credentials.

Consumers enable `vendored` so UI assets come from the checksum-verified Cargo
package instead of an extra build-time curl download. The build script processes
that fixed UI bundle; Drive's untrusted archive extraction separately rejects
links/special files, normalizes paths, bounds sizes and ignores archive permissions.

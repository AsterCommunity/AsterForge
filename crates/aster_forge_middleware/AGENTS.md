# AGENTS.md

This file supplements the repository-level [`../../AGENTS.md`](../../AGENTS.md) and applies only to `aster_forge_actix_middleware`. Follow the root file for the general workflow, Rust conventions, and Forge/product boundary. Use [`../../docs/crates/aster_forge_actix_middleware.md`](../../docs/crates/aster_forge_actix_middleware.md) as the complete API and integration reference instead of copying it here and creating two competing sources of truth.

## Before Making Changes

1. Read this crate's documentation, `Cargo.toml`, `src/lib.rs`, and the middleware module involved in the task.
2. For shared primitives, also read `aster_forge_utils`; for metrics, read the `aster_forge_metrics` and `aster_forge_actix_observability` documentation.
3. Determine whether the change is Actix transport or middleware mechanics, or product authentication, authorization, error-envelope, configuration-key, or audit semantics. The latter remain in product repositories.

## Ownership Boundaries

- This crate owns request IDs, security response headers, CSRF, runtime CORS, trusted-proxy client IP handling, generic rate limiting, and optional HTTP metrics middleware.
- `aster_forge_api` must remain framework-neutral. Do not move Actix types into it.
- The `/metrics` endpoint belongs to `aster_forge_actix_observability`; recorder and backend behavior belongs to `aster_forge_metrics`.
- Authentication, administrator checks, product `ApiResponse` types, localized text, route policy, audit, and business labels do not belong here.

## Change Constraints

- Proxy headers take effect only when the direct peer is trusted. Invalid forwarded values must fall back to the direct peer rather than expanding the trust boundary.
- Middleware errors must remain classifiable so product code can map them into 4xx/5xx responses and product response bodies.
- Metrics route labels must be low-cardinality. Never use an unmatched real path directly as a label.
- Prefer explicit injection points such as resolvers, predicates, and response factories; do not read product global state.
- APIs behind the `metrics` feature must leave the base middleware buildable when the feature is disabled.

## Validation

```bash
cargo test -p aster_forge_actix_middleware
cargo test -p aster_forge_actix_middleware --all-features
cargo clippy -p aster_forge_actix_middleware --all-targets --all-features -- -D warnings
```

Add tests as appropriate for trusted and untrusted proxies, CORS preflight, header-name case handling, rate-limit boundaries, missing app data, low-cardinality route labels, and custom error mapping.

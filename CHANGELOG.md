# Changelog

## [Unreleased]

### Added

- Complete Axum HTTP middleware adapters for CSRF, runtime CORS, trusted-proxy client IPs, and IP/keyed rate limits. Framework-neutral CSRF and keyed rate limiting are available under `aster_forge_middleware::shared` without enabling Actix. Compilable Actix and Axum examples demonstrate product policy and error boundaries.

### Changed

- System configuration persistence now uses `upsert_prevalidated` on bindings, stores, and free functions to make caller validation explicit. Rename existing `upsert` calls accordingly; validate logical values before any secret encoding. `ConfigRegistry::validate_value` is now `validate_value_structure`, while `normalize_value` remains the full validation pipeline.
- Rate-limit retry delays round all fractional seconds upward. Missing direct peers share a localhost quota bucket and cannot establish trust for forwarded headers.
- Runtime CORS preserves all `Vary` values, merges names case-insensitively, and respects `Vary: *`.
- Axum security headers only fill missing values, preserving product policies such as `Referrer-Policy: no-referrer`. Both transports generate a fresh UUID v4 request ID regardless of inbound request headers.

### Fixed

- Failed configuration reloads and connection reconciliations retry with bounded backoff even when no new notification or reconnect occurs. Recovery work is coalesced, and shutdown cancels retry waits and active callbacks.

[Unreleased]: https://github.com/AsterCommunity/AsterForge/compare/1ba5754792ae94bd8888b03d09ada1644b2107c0...HEAD

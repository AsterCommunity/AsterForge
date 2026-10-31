# Changelog

## [Unreleased]

### Added

- Framework-neutral typed REST envelopes in `aster_forge_api::response`, preserving omitted data, explicit null, empty-object responses and optional typed error diagnostics. Product-defined codes are checked against success/error state during construction and deserialization; OpenAPI supports multiple codes/DTOs and nested references.
- Complete Axum HTTP middleware adapters for CSRF, runtime CORS, trusted-proxy client IPs, and IP/keyed rate limits. Framework-neutral CSRF and keyed rate limiting are available under `aster_forge_middleware::shared` without enabling Actix. Compilable Actix and Axum examples demonstrate product policy and error boundaries.

### Changed

- **Task consumers (breaking)** — Separate continuously replenished, capacity-bounded durable consumers from singleton scheduling. Replace the claim-store API with one atomic quota/selection/fenced-claim transaction, remove `fast_continue`, and rename singleton `.worker()` to `.singleton_worker()`. Named lifecycle groups expose per-runner liveness/readiness. Claim, release and handler tasks progress independently, retain bounded polling backoff, and are aborted and joined after drain/cooperative cancellation deadlines; unknown claims and uncooperative work retain leases until expiry. Typed heartbeat cancellation and renewal deadlines wake pending I/O; execution acknowledges reclaimed canceled work without invoking handlers and resolves retry/shutdown races. Products own cancellation permissions, states, and committed-output checkpoints. Add bounded queue-age and lease-takeover metrics. Migrate stores, execution adapters, handlers, components and downstreams together.

- **Aster Metrics v1 (breaking)** — Adopt OTel HTTP/DB/process instruments, seconds-based cumulative CPU, native memory/pool snapshots, versioned resource identity, standard Prometheus translation and `target_info`. HTTP adapters observe response-body completion/cancellation, complete HEAD without false cancellation, omit unmatched routes, and retain integer status attributes in OTLP. Pool sampling requires an application-unique static identity; registration rejects both OTel and translated Prometheus name collisions. Shared extensions move to `aster.*`; update dashboards/rules with the application revision. No legacy series aliases or duplicate instrumentation.
- Authentication key-length failures now return `AuthError::KeyTooShort` instead of `InvalidPolicy` for HMAC construction and algorithm-specific issuance/verification. Products can use the public `JwtAlgorithm::minimum_key_bytes()` to validate configuration at startup; exhaustive error mappings must handle the new variant.
- Trusted-proxy middleware documentation and adapter tests now match the existing right-to-left `X-Forwarded-For` trust walk, stopping at the first untrusted hop rather than accepting a spoofable leftmost value.
- Upgrade system sampling to `sysinfo` 0.40, retaining Rust 1.95 as the minimum. Failed system initialization retries on the next sampling interval while uptime, allocator, and exporter diagnostics continue to update.
- Move the documentation site and Rust API reference to `https://forge.docs.astercosm.com/`; `https://forge.astercosm.com/` is the product landing page. Existing documentation paths are retained.

- Upgrade `parking_lot` to 0.13.0. It retains Rust 1.95 as its minimum and removes automatic eventual fairness; fair unlocking is now explicit through the fair-unlock APIs.
- The Actix service template uses the shared `{code,msg,data?,error?}` envelope for API 404 responses. HTTP status and health probe payloads stay product-owned; generated OpenAPI/SDK artifacts describe the new template contract. Its HTTP client preserves typed-envelope JSON and reports non-JSON HTTP failures with their original status.
- System configuration persistence now uses `upsert_prevalidated` on bindings, stores, and free functions to make caller validation explicit. Rename existing `upsert` calls accordingly; validate logical values before any secret encoding. `ConfigRegistry::validate_value` is now `validate_value_structure`, while `normalize_value` remains the full validation pipeline.
- Rate-limit retry delays round all fractional seconds upward. Missing direct peers share a localhost quota bucket and cannot establish trust for forwarded headers.
- Runtime CORS preserves all `Vary` values, merges names case-insensitively, and respects `Vary: *`.
- Axum security headers only fill missing values, preserving product policies such as `Referrer-Policy: no-referrer`. Both transports generate a fresh UUID v4 request ID regardless of inbound request headers.

### Fixed

- Failed configuration reloads and connection reconciliations retry with bounded backoff even when no new notification or reconnect occurs. Recovery work is coalesced, and shutdown cancels retry waits and active callbacks.

### Security

- **Bounded XML and WebDAV (breaking)** — Add an independent active-namespace quota (128 by default), count namespace declarations in embedded-subtree limits, and avoid pairwise writer attribute checks. PROPFIND/version-tree selectors deduplicate expanded names and bound copied bytes; property composers now require explicit `DavPropertyMaterializationLimits`. PATCH enforces immutable/checked-in version rules and returns the automatic checkout/checkin action for product transactions.
- **Persistence and identity boundaries** — Reject unsafe blob shards and Windows drive-qualified object keys; fence macOS content reads to the engine scope. Audit records validate before admission and return `RecordError::Invalid` without blocking valid records. Configuration updates and default seeding reject collation aliases before modifying the conflicting row and preserve system visibility; database diagnostics redact credential-bearing URLs and overlapping raw/encoded userinfo. Migration helpers quote extension/index identifiers as data.
- **External auth provider destinations (breaking)** — Dedicated Google, GitHub and QQ drivers use official destinations unconditionally. Use generic OIDC/OAuth2 for custom issuers or compatible services. Provider-option diagnostics omit rejected documents; mail diagnostic fields escape control characters. Scope/domain deduplication, mail fallback parsing and saturated cache reservations avoid repeated full scans while preserving order, fallback and TTL semantics.
- **HTTP and local state boundaries** — Both HTTP transports combine repeated forwarding fields before trusted-proxy resolution; an active CORS policy disabling credentials removes inherited credential headers. Unix test state uses verified per-user directories, non-truncating no-follow locks and private atomic files. The Linux memory example uses the same state-file mechanics, replaces restored ledgers with private files before mounting, and bounds documents to 16 MiB with 1 MiB upload chunks; it remains a synthetic adapter.

[Unreleased]: https://github.com/AsterCommunity/AsterForge/compare/1ba5754792ae94bd8888b03d09ada1644b2107c0...HEAD

# Changelog

## [Unreleased]

### Changed

- **Panic reporting (breaking)** — Separate sensitive reports from public human/silent/custom diagnostics; inject product report writers, JSON sinks and timestamp policy. Default timestamps are UTC RFC3339; failed writes never expose raw reports on stderr. Linux private files reject symlinks, unsafe permissions and hard links; other platforms, including macOS where ACLs are not validated, require a product writer. Installation now happens once without chaining prior hooks or replacing later host hooks. Public config literals/trait derives and the install return type change; callbacks must not panic or block.

- **HTTP and crypto features (breaking; ten affected crates advance to alpha.2)** — HTTP consumers explicitly select request encodings, streaming, blocking, HTTP/2, charset, system proxy and TLS backends. Rustls supports AWS-LC, ring or an application-installed provider; async/blocking TLS selection preserves certificate and hostname verification. Bounded reads no longer require streaming or futures executors. Crypto consumers select Argon2 password hashing, SHA-256, HMAC-SHA256 and the existing secret-envelope format independently. Update consumer features instead of relying on workspace unification; authentication and OTLP retain an explicit, replaceable rustls/AWS-LC default.

## [0.1.0-alpha.1] - 2026-11-11

### Added

- First crates.io alpha release of all 33 AsterForge workspace crates. Each crate owns its version independently; this release uses `0.1.0-alpha.1` throughout, explicit internal dependency versions, and package README metadata.

- Product-neutral `aster_forge_macros::wire_enum!` generates string protocol enums with exact known-value mapping, lossless unknown strings, Serde support, and compile-time rejection of duplicate wire values. Consumers retain protocol and execution policy.

- Correlated local task/transaction diagnostics with consumer instance, trigger, capacity, claim sequence and timing, task lease identity, real transaction caller, phase timing and safe outcomes. Spawned work preserves context; idle polling stays at TRACE and raw handler/callback errors are omitted.

- Framework-neutral typed REST envelopes in `aster_forge_api::response`, preserving omitted data, explicit null, empty-object responses and optional typed error diagnostics. Product-defined codes are checked against success/error state during construction and deserialization; OpenAPI supports multiple codes/DTOs and nested references.
- Complete Axum HTTP middleware adapters for CSRF, runtime CORS, trusted-proxy client IPs, and IP/keyed rate limits. Actix/Axum CORS supports typed policy-rejection response factories, preserving product responses and required Vary fields while removing stale CORS grants; default empty 403 responses and request/response error classifications remain unchanged. Framework-neutral CSRF and keyed rate limiting are available under `aster_forge_middleware::shared` without enabling Actix. Compilable Actix and Axum examples demonstrate product policy and error boundaries.

### Changed

- **Outbound HTTP transport (breaking)** — Centralize async and blocking reqwest construction in `aster_forge_http`, with redirect/retry rejection, explicit proxy/TLS/timeouts and request observation through response headers. External-auth runtime configs add an optional product-owned HTTP client; update config literals and inject product transport policy. OTLP and Mailpit use the same factory.

- **Task consumers (breaking)** — Separate continuously replenished, capacity-bounded durable consumers from singleton scheduling. Replace the claim-store API with one atomic quota/selection/fenced-claim transaction, remove `fast_continue`, and rename singleton `.worker()` to `.singleton_worker()`. Named lifecycle groups expose per-runner liveness/readiness. Claim, release and handler tasks progress independently, retain bounded polling backoff, and are aborted and joined after drain/cooperative cancellation deadlines; unknown claims and uncooperative work retain leases until expiry. Typed heartbeat cancellation and renewal deadlines wake pending I/O; execution retains heartbeat renewal through result/control I/O, acknowledges reclaimed canceled work without invoking handlers, and confirms the persisted success/token fence before reporting success. Products must implement `confirm_task_succeeded` and atomically fence retry/failure/release writes against persisted cancellation when migrating execution stores. Products own cancellation permissions, states, and committed-output checkpoints. Add bounded queue-age and lease-takeover metrics. Consumers now implement `wakeup()` with instance-owned broadcast `TaskWakeup` subscriptions instead of competing notification futures; enqueue/config changes reach every subscribed lane and remain pending through in-flight claims. Migrate stores, execution adapters, handlers, components and downstreams together.

- **Aster Metrics v1 (breaking)** — Adopt OTel HTTP/DB/process instruments, seconds-based cumulative CPU, native memory/pool snapshots, versioned resource identity with independent product-defined node mode (`aster.node.mode` / `aster_node_mode`) and process role, standard Prometheus translation and `target_info`. HTTP adapters observe response-body completion/cancellation, complete HEAD without false cancellation, omit unmatched routes, and retain integer status attributes in OTLP. Pool sampling requires an application-unique static identity; registration rejects both OTel and translated Prometheus name collisions. Shared extensions move to `aster.*`; update dashboards/rules with the application revision. No legacy series aliases or duplicate instrumentation.
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

- Upgrade Actix Web to 4.18.2 and locked Actix HTTP to 3.20.3, closing the request decompression and chunked-trailer advisories. Refresh Rust 1.95-compatible dependencies and OIDC 4.1.0, update the service template, and remove its obsolete proc-macro-error2 exception; audit exceptions explicitly distinguish unresolved upstream cryptography/HTTP2 risks from the test-only tar maintenance warning.

- Mail outbox diagnostics escape recipient, template and failure text on every retry/permanent-failure and persistence-error path, including historical rows. Email validation rejects control characters before trimming.

- **Bounded XML and WebDAV (breaking)** — Add an independent active-namespace quota (128 by default), count namespace declarations in embedded-subtree limits, and avoid pairwise writer attribute checks. PROPFIND/version-tree selectors deduplicate expanded names and bound copied bytes; property composers now require explicit `DavPropertyMaterializationLimits`. PATCH enforces immutable/checked-in version rules and returns the automatic checkout/checkin action for product transactions.
- **Persistence and identity boundaries** — Reject unsafe blob shards and Windows drive-qualified object keys; fence macOS content reads to the engine scope. Audit records validate before admission and return `RecordError::Invalid` without blocking valid records. Configuration updates and default seeding reject collation aliases before modifying the conflicting row and preserve system visibility; database diagnostics redact credential-bearing URLs and overlapping raw/encoded userinfo. Migration helpers quote extension/index identifiers as data.
- **External auth provider destinations (breaking)** — Dedicated Google, GitHub and QQ drivers use official destinations unconditionally. Use generic OIDC/OAuth2 for custom issuers or compatible services. Provider-option diagnostics omit rejected documents; mail diagnostic fields escape control characters. Scope/domain deduplication, mail fallback parsing and saturated cache reservations avoid repeated full scans while preserving order, fallback and TTL semantics.
- **HTTP and local state boundaries** — Both HTTP transports combine repeated forwarding fields before trusted-proxy resolution; an active CORS policy disabling credentials removes inherited credential headers. Unix test state uses verified per-user directories, non-truncating no-follow locks and private atomic files. The Linux memory example uses the same state-file mechanics, replaces restored ledgers with private files before mounting, and bounds documents to 16 MiB with 1 MiB upload chunks; it remains a synthetic adapter.

[Unreleased]: https://github.com/AsterCommunity/AsterForge/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/AsterCommunity/AsterForge/releases/tag/v0.1.0-alpha.1

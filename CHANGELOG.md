# Changelog

## [Unreleased]

### Changed

- **Actix rate limiting (breaking)** — Replace the GPL-licensed `actix-governor`
  adapter with Forge's `governor`-backed `RateLimitKeyExtractor`, shared
  `RateLimiterConfig` and Actix middleware. Migrate callers to the new helper
  names and `.wrap(RateLimit::new(&config))`; old `GovernorConfig` helpers are
  removed. The middleware supports body-transforming services, exposes
  `retain_recent()` for product maintenance, and preserves configured outer
  middleware behavior on rejection. Upgrade workspace `process-wrap` to 11,
  `syn` to 3 and Redis to 1.8.3.
- **Middleware crate release intent** — Advance `aster_forge_middleware` from
  `0.1.0-alpha.5` to `0.1.0-alpha.6`; no other in-workspace crate depends on it.

## [2026-11-23 batch] - 2026-11-23

Changes since `da09ee6b6ae57baba3a423438305e927ea51e475`. This batch selects
28 changed crates, including their affected internal consumers. Each advances
by one alpha version; the other seven crates retain their published versions.

### Added

- **Audit recovery files** — Preserve stopped writers' unconfirmed audit records in
  versioned private JSON for database reconciliation. Shutdown helpers retain the
  service failure and report incomplete drain even when preservation succeeds;
  records are never replayed automatically.
- **Reusable schema fixtures** — PostgreSQL/MySQL test containers reuse migrated
  template databases by schema fingerprint and producer version, with locked
  rebuilds, bounded connection probes and interrupted-build cleanup. Products retain
  migration, seed, naming and fingerprint ownership.

### Changed

- **TLS backend selection** — Keep rustls as the default for SMTP, Redis and
  SeaORM consumers while exposing explicit `native-tls` features alongside the
  existing HTTP, external-auth and OTLP choices. The workspace default graph
  now fails CI if `native-tls` is pulled in accidentally.
- **Metrics profile identity** — Use `astercosm-metrics/v1` as the shared
  schema token and document reverse-DNS product namespaces such as
  `com.astercosm.drive`; the product still supplies its own namespace through
  `with_profile`.
- **Configuration validation (breaking)** — Add an unconditional section-validation
  derive and a shared `load_config` pipeline. Forge-owned sections and product
  sections must implement `ValidatableConfig`; root configurations retain an
  explicit cross-section `validate` step.
- **Observability configuration (breaking)** — Provide reusable metrics identity,
  Prometheus/OTLP settings, bounded validation, and validated provider/recorder
  assembly for downstream products. `MetricsIdentity` implements `ValidatableConfig`;
  `MetricError::InvalidConfiguration(String)` retains field and constraint diagnostics
  across loader and direct-provider entrypoints and is no longer a Copy/unit variant.
- **Public API documentation** — Complete API rustdoc and enable the workspace
  `missing_docs` lint; clarify password verification timing contracts.

### Fixed

- **Panic filesystem side effects** — Linux private report writers require existing
  ancestor directories, so installing the default hook cannot create `data/`.
  Explicit bootstrap may create that directory before installing a new writer.
  Filesystem behavior is checked on Linux, macOS and Windows before release gates.

## [2026-11-21 batch] - 2026-11-21

The 31-crate release recorded by `da09ee6b6ae57baba3a423438305e927ea51e475`.

### Added

- **Release automation** — Detect manually versioned, unpublished crates and
  open a release PR containing format-preserving dependency pin updates and an
  explicit release plan. Validate committed lock contents, affected consumers,
  normalized archives and versioned development dependency ordering;
  archive verification compiles each normalized package without relying on
  unarchived sibling test fixtures. Dispatch publication through a crates.io-supported event only after both
  exact-merge CI workflows pass, repeating review and CI gates before requesting
  credentials. Interrupted publication
  can resume without re-uploading existing versions. Downstream package versions
  and security release scope remain maintainer-owned.

### Changed

- **Panic timestamp dependency** — Generate the default UTC millisecond panic
  timestamp with workspace-owned `time`; out-of-range system clocks report an
  unavailable marker without panicking inside the hook. Public APIs, task/runtime
  timestamps, database entities and encodings, WebDAV and the service template
  retain chrono. Track the broader migration in #75 and explicitly accept the
  unmaintained advisory RUSTSEC-2026-0543 until direct/upstream adoption is ready.
- **Dependency refresh** — Run `cargo upgrade --incompatible allow --pinned allow`
  and `cargo update`, including Actix Web 4.20, fuser 0.19, SQLx 0.9.1 and
  libsqlite3-sys 0.40. Linux FUSE accepts the new callback parameters and rejects
  ownership/privilege requests the fixed mount policy cannot represent.
  Remove unused dependency declarations and enable optional logging/HTTP
  dependencies only through their owning features.
- **Affected package versions** — Increment each of the 31 affected publishable
  crates by one alpha release and synchronize exact workspace dependency pins.
  Macros, API-doc macros and validation retain their versions. Versions record
  release intent; publication remains a separate reviewed step.
- **Daily security audit** — Run at 09:17 UTC every day instead of every Monday;
  manual dispatch remains available.

### Security

- **SMTP dependency fixes** — Require lettre >=0.11.24 to address
  RUSTSEC-2026-0560 through RUSTSEC-2026-0565 (STARTTLS response injection,
  dot-stuffing, whitespace EHLO panic, quoted-recipient routing, display-name
  panic and unbounded SMTP reply buffering). Add message/recipient, EHLO, bounded-response, STARTTLS-injection and
  bare-newline/chunk-boundary dot-stuffing regression tests.
- **Audit exceptions** — Remove the resolved libsqlite3-sys and crypto-bigint
  exceptions after dependency updates. Retain the scoped RSA, h2, paste and
  testcontainers tar exceptions with current dependency evidence, and record a
  scoped chrono maintenance exception linked to #75; this is accepted risk,
  not remediation.

### Fixed

- **Concurrent private lock creation on macOS** — Retry one transient ENOENT
  from descriptor-relative O_CREAT|O_NOFOLLOW before applying the same file
  identity/ownership checks, preventing false container-lock failures.

## [2026-11-17 batch] - 2026-11-17

Cumulative package changes since the first bulk release. This batch publishes
17 crate versions; the other 17 retain their already-published versions. Crates
continue to own their versions independently.

### Added

- **Bounded process execution** — Add `aster_forge_process` for direct child execution with bounded pipe capture, optional stdin/environment control, cancellation and deadlines, and owned Unix process-group / Windows Job Object cleanup. Product adapters retain retry, error-mapping, logging, and side-effect policy.

### Fixed

- **Process-tree completion** — Separate direct-child exit from tree cleanup, terminate descendants before waiting on inherited output pipes, and reap children before confirming Unix process groups or Windows Jobs are empty. Windows completion uses the Job's active-process count and closes setup handles on every exit path. Cleanup failures are explicit in `ProcessOutcome`.

### Changed

- **Published crate versions** — Publish `0.1.0-alpha.3` for auth, external-auth,
  middleware, observability, test-support, audit, database and metrics, and
  `0.1.0-alpha.2` for cache, config, events, tasks, utils, WebDAV, runtime, XML and
  mail. The eleven direct updates include JWT 12 error handling, Redis 1.8.2 /
  rand 0.10.4 / Actix Web 4.19 dependency requirements, and the PostgreSQL-only
  test-support cleanup gate. Runtime, XML, mail, metrics, database and audit are
  explicitly republished to align exact internal dependencies and avoid old/new
  utils resolution conflicts; their public APIs are unchanged. Other package
  versions stay unchanged. All 17 versions are available on crates.io.

- **Dependency refresh** — Upgrade compatible workspace dependencies and move the JWT backend to `jsonwebtoken` 12.0.0. The auth adapter now handles the upstream fallible signing-key constructors while preserving the existing token contract and backend isolation.

- **CI selection** — Validate changed crates and their transitive workspace dependents on pushes and PRs, with separate reusable workflows for panic, process, authentication, HTTP/crypto, XML fuzzing and native Cloud Files. Keep a stable aggregate check; run workspace coverage and the complete regression suite weekly or manually. Generated service checks use the current Forge checkout.

- **Workspace dependencies** — Centralize member dependency versions while preserving per-consumer features and default-feature boundaries. Upgrade Actix Web to 4.19.0 / Actix HTTP to 3.21.0 and refresh Rust 1.95-compatible locked dependencies.

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

[Unreleased]: https://github.com/AsterCommunity/AsterForge/compare/b0c0e306e5c8cd299c04da5ff0b946b6f019e06e...HEAD
[2026-11-23 batch]: https://github.com/AsterCommunity/AsterForge/compare/da09ee6b6ae57baba3a423438305e927ea51e475...b0c0e306e5c8cd299c04da5ff0b946b6f019e06e
[2026-11-21 batch]: https://github.com/AsterCommunity/AsterForge/compare/964540389db08b070b24bcfb24cd353576a07932...da09ee6b6ae57baba3a423438305e927ea51e475
[2026-11-17 batch]: https://github.com/AsterCommunity/AsterForge/compare/0030ea4083e8739b176ec0cc0edf2ee1fae5f920...964540389db08b070b24bcfb24cd353576a07932
[0.1.0-alpha.1]: https://github.com/AsterCommunity/AsterForge/commit/0030ea4083e8739b176ec0cc0edf2ee1fae5f920

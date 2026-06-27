# {{project-name}}

This service was generated from the AsterForge `aster-service` template.

## Resources

- AsterForge project: [AsterCommunity/AsterForge](https://github.com/AsterCommunity/AsterForge)
- AsterForge documentation: [forge.astercosm.com](https://forge.astercosm.com/)
- AsterForge Rust API documentation: [forge.astercosm.com/crates/rustdoc](https://forge.astercosm.com/crates/rustdoc/)
- New project integration guide: [forge.astercosm.com/guide/new-project-integration](https://forge.astercosm.com/guide/new-project-integration)

## First Run

```bash
cargo check
cargo run
```

The generated service starts an Actix HTTP server and registers the standard Forge runtime
components:

- HTTP service component.
- Background task shutdown component.
- Mail outbox shutdown component.
- Audit lifecycle component.
- Database shutdown and health component.
- Migration crate with Forge-owned infrastructure tables.
- Optional Prometheus metrics export when the `metrics` feature is enabled.
- Debug allocation tracking by default and jemalloc feature flags for production tuning.
- Debug OpenAPI document and Swagger UI when the `openapi` feature is enabled.

Runtime display names use Cargo package metadata. Rename the generated package in `Cargo.toml`
when the service name should change; the template reads `env!("CARGO_PKG_NAME")` instead of
keeping a separate service-name placeholder.

## Configuration

The generated service looks for static configuration at `data/config.toml` by default. Override the
path with `ASTER_CONFIG=/path/to/config.toml`.

If no configuration file exists, the service writes `data/config.toml` with the defaults selected
during `cargo generate`, then loads that file. Copy or edit `config.example.toml` when the service
needs deployment-specific values.
The default data layout matches AsterDrive and AsterYggdrasil:

- `data/config.toml`
- `data/{{project-name}}.db`
- `data/.tmp`
- `data/{{project-name}}.log`

Startup creates the parent directories for the configured temp directory, SQLite database, and log
file.

Relative filesystem paths and relative `sqlite://` database paths in `data/config.toml` are
resolved from the configuration file directory. For example, `sqlite://service.db?mode=rwc` in
`data/config.toml` points to `data/service.db`.

## Template Parameters

The generator exposes the same boot-time groups used by AsterYggdrasil-style services:

- Package metadata: `package_description`, `forge_git`.
- Server: `server_host`, `server_port`, `server_workers`, `server_temp_dir`.
- Database: `database_url`, `database_pool_size`, `database_retry_count`.
- Cache: `cache_backend`, `cache_endpoint`, `cache_default_ttl`.
- Config sync: `config_sync_backend`, `config_sync_endpoint`, `config_sync_topic`.
- Logging: `logging_level`, `logging_format`, `logging_file`, `logging_enable_rotation`,
  `logging_max_backups`.

## Product Boundaries

Keep these in this product repository:

- Product API routes and DTOs.
- Product permissions and user-facing error mapping.
- Product entities and migrations.
- Product audit action/detail/presentation types.
- Product task kind, payload, result, and execution body.
- Product mail payloads, template rendering, URLs, and audit hooks.

Use Forge for reusable mechanics:

- `AsterRuntime` lifecycle and component graph.
- Database handles, migration schema builders, shared infrastructure stores, and shutdown.
- Background task runtime/shutdown mechanics.
- Mail outbox dispatch and DB-backed state machine.
- Audit lifecycle component and shared audit log store.
- Cache/config/middleware/metrics/panic helpers.

## Migrations

The generated workspace includes a `migration` crate. The first migration creates Forge-owned
infrastructure tables for runtime leases, scheduled tasks, system config, mail outbox, and audit
logs. Product tables should be added as new migration modules in that crate.

## OpenAPI

OpenAPI generation follows the same debug-only pattern used by Aster services:

```bash
cargo run --features openapi
cargo test --features openapi --test generate_openapi
```

When enabled in a debug build, the service exposes:

- `/api-docs/openapi.json`
- `/swagger-ui/`

The OpenAPI generation test writes `generated/openapi.json`. Products with a frontend can point
their frontend type generator at that file or change the test output path to their frontend
workspace.

Add product route annotations with `aster_forge_api_docs_macros::path(...)`, then register those
handlers and schemas in `src/api/openapi.rs`. Release builds do not expand the route annotations
unless the product intentionally changes that policy.

## Metrics and Allocator

Prometheus metrics are available when the `metrics` feature is enabled:

```bash
cargo run --features metrics
```

The service then exposes `/metrics`. The template records low-cardinality HTTP, database, health,
background task, external-operation, allocator, process RSS, CPU, and uptime metrics through Forge
recorder traits.

Allocator behavior follows the Aster service pattern:

- Debug builds without `jemalloc` use `aster_forge_alloc::TrackingAlloc`.
- `--features jemalloc` enables `tikv-jemallocator`.
- `--features jemalloc-stats` enables jemalloc allocator stats.
- `--features jemalloc-profiling` enables jemalloc profiling support.

## Health Checks

The template exposes:

- `/healthz`: lightweight liveness response.
- `/readyz`: database and cache readiness check.
- `/metrics`: Prometheus text export when the `metrics` feature is enabled.

## CI and Container Image

The generated project includes:

- `.github/workflows/rust.yml`: format, check, all-feature clippy, tests, OpenAPI generation, and
  Rust coverage artifact upload.
- `.github/workflows/audit.yml`: scheduled and manual `cargo audit`.
- `.github/workflows/docker-image.yml`: GHCR image publishing for default and `metrics` variants.

Build the container image with:

```bash
docker build -t {{project-name}} .
```

By default the image builds with `CARGO_FEATURES=metrics`, matching the AsterDrive and
AsterYggdrasil container profile. Override it when needed:

```bash
docker build --build-arg CARGO_FEATURES=metrics,openapi -t {{project-name}} .
```

Run the generated compose file:

```bash
docker compose up --build
```

Mount `/data` for persistent runtime files. The image sets:

- `ASTER__SERVER__HOST=0.0.0.0`
- `ASTER__DATABASE__URL=sqlite:///data/{{project-name}}.db?mode=rwc`
- `ASTER__SERVER__TEMP_DIR=/data/.tmp`
- `ASTER__LOGGING__FILE=/data/{{project-name}}.log`

The image healthcheck probes `/readyz`.

## Local Forge Development

The template depends on the AsterForge project through Git:

```toml
aster_forge_runtime = { git = "{{forge_git}}", package = "aster_forge_runtime" }
```

When working on Forge and a generated product on the same machine, temporarily replace those Git
dependencies with local `path = "../AsterForge/crates/..."` entries or a `[patch]` section in the
generated product. Keep the generated product's public shape the same: product code should call
Forge APIs directly unless an adapter adds product error mapping, config injection, audit, metrics,
or permissions.

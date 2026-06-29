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

Startup creates the parent directories for the configured temp directory and SQLite database. It
also creates the parent directory for `logging.file` when file logging is configured.

Relative filesystem paths and relative `sqlite://` database paths in `data/config.toml` are
resolved from the configuration file directory. For example, `sqlite://service.db?mode=rwc` in
`data/config.toml` points to `data/service.db`.

## Template Parameters

The generator only asks for values that are useful at project creation time:

- `package_description`: Cargo package and container image description.
- `server_port`: Local HTTP bind port.

Server host/workers/temp dir, database pool/retry settings, cache, config sync, and logging use
conservative defaults in the generated configuration. Override them in `data/config.toml` or with
`ASTER__...` environment variables when deploying.

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

The service then exposes `/health/metrics` through `aster_forge_actix_observability`. Forge records
low-cardinality HTTP, database, health, background task, external-operation, allocator heap,
process RSS, CPU, and uptime metrics through shared recorder traits.

Allocator behavior follows the Aster service pattern:

- Debug builds without `jemalloc` use `aster_forge_alloc::TrackingAlloc`.
- `--features jemalloc` enables `tikv-jemallocator`.
- `--features jemalloc-stats` enables jemalloc allocator stats.
- `--features jemalloc-profiling` enables jemalloc profiling support.

## Health Checks

The template exposes:

- `/api/v1/*`: versioned product API scope. Unknown API paths return a JSON
  `endpoint_not_found` response instead of the frontend SPA fallback.
- `/health`: lightweight liveness response.
- `/health/ready`: database and cache readiness check.
- `/health/metrics`: Prometheus text export when the `metrics` feature is enabled.

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

The image healthcheck probes `/health/ready`.

## Local Forge Development

The template depends on the AsterForge project through Git:

```toml
aster_forge_runtime = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_runtime" }
```

When working on Forge and a generated product on the same machine, temporarily replace those Git
dependencies with local `path = "../AsterForge/crates/..."` entries or a `[patch]` section in the
generated product. Keep the generated product's public shape the same: product code should call
Forge APIs directly unless an adapter adds product error mapping, config injection, audit, metrics,
or permissions.

# AsterForge

AsterForge is a reusable Rust + React service foundation for Aster projects. It is the base layer you copy before adding product-specific domain code: HTTP server, authentication, runtime configuration, audit logs, background tasks, admin APIs, OpenAPI generation, embedded frontend assets, and deployment defaults.

The repository focuses on common runtime capabilities that are useful across services. Product-specific domains should be added by downstream projects on top of this foundation.

- Chinese README: [README.zh.md](README.zh.md)
- Developer docs: [developer-docs/README.md](developer-docs/README.md)
- Example config: [config.example.toml](config.example.toml)
- Frontend panel: [frontend-panel/](frontend-panel/)

## What You Get

### Backend foundation

- Actix Web service with embedded frontend assets.
- SeaORM entities, repositories, migrations, database retry helpers, transactions, and reader/writer database handles.
- Stable API response envelope with public `AsterErrorCode` values.
- Local auth: first-admin setup, register, login, refresh, logout, current user, and session management.
- External auth provider scaffolding for OIDC/OAuth2-style flows.
- Admin APIs for runtime config, audit logs, external auth providers, and background tasks.
- Runtime config stored in `system_config`, separate from static `config.toml`.
- Memory/noop/Redis cache backends behind a shared cache trait.
- Request ID, security headers, runtime CORS, CSRF helpers, request metrics, and IP rate limit middleware.
- Health and readiness endpoints, plus optional Prometheus metrics.

### Runtime foundation

- Primary/follower startup split through `server.start_mode`.
- Graceful shutdown for HTTP, background tasks, audit flush, and database handles.
- Buffered async audit writes with structured presentation metadata for frontend display.
- Background task records, dispatch, lease/heartbeat handling, retry classification, cleanup, and stable task presentation metadata.
- Primary-only runtime maintenance tasks:
  - background task dispatcher
  - system health check
  - auth session cleanup
  - external auth flow cleanup
  - audit log cleanup
  - task artifact cleanup

Follower mode keeps common runtime initialization but skips primary-only dispatch and cleanup loops.

### Frontend foundation

- React + Vite + TypeScript admin panel under `frontend-panel/`.
- Typed service layer generated from OpenAPI.
- Admin pages for configuration, audit logs, external auth providers, and tasks.
- Stable audit/task presentation formatting so the frontend does not parse raw JSON details or task payloads.
- Unit tests with Vitest and jsdom, formatting/linting with Biome, and Vite production build.

## What Is Deliberately Not Included

AsterForge leaves product-specific modules to downstream services:

- file storage
- upload flows
- teams or shares
- trash or archives
- thumbnails or media processing
- WebDAV or WOPI
- storage policies or remote nodes
- ordinary user task APIs

Background tasks in this template are administrator-facing and runtime-facing only. Do not assume ordinary users can see task records. If a product needs user task APIs, design that product-specific visibility model in the product repository.

AsterForge also does not include a second public API subcode system. Client-visible failures should use named `AsterErrorCode` values.

## Repository Layout

```text
src/                         Rust backend
src/api/                     Routes, DTOs, OpenAPI registration, middleware, response envelope
src/cache/                   Cache trait and memory/noop/Redis implementations
src/config/                  Static config, runtime config definitions, normalizers
src/db/                      Connections, retry helpers, transactions, repositories
src/entities/                SeaORM entity models
src/metrics/                 Prometheus implementation behind the metrics feature
src/runtime/                 App state, startup, shutdown, logging, background task loops
src/services/                Auth, external auth, config, audit, task, health, examples
src/types/                   Shared domain enums and stored DB wrapper types
src/utils/                   Crypto, ID, path, number, email, and RAII helpers
migration/                   SeaORM migration crate
api-docs-macros/             OpenAPI helper macro crate
frontend-panel/              React admin panel
developer-docs/              Developer-facing notes and extension guide
tests/                       Integration tests and OpenAPI export test
```

## Quick Start From Source

Requirements:

- Rust toolchain from [rust-toolchain.toml](rust-toolchain.toml)
- Bun for the frontend panel
- SQLite by default

Build frontend assets, then run the service:

```bash
cd frontend-panel
bun install
bun run build
cd ..

cargo run
```

On first startup AsterForge will:

- create `data/config.toml` if it is missing
- resolve default relative paths under `data/`
- create the default SQLite database
- run migrations
- install default runtime config rows into `system_config`
- start the HTTP service on `127.0.0.1:3000`

Open:

```text
http://127.0.0.1:3000
```

Create the first admin user:

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

For local HTTP cookie testing before the first boot:

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

Keep secure cookies enabled behind HTTPS in real deployments.

## Docker

Run with Compose:

```bash
mkdir -p ./data
docker compose up -d
```

Or build and run locally:

```bash
docker build -t asterforge:local .
docker run --rm -p 3000:3000 -v "$(pwd)/data:/data" asterforge:local
```

The container expects writable runtime state under `/data`.

## Generate A New Project

AsterForge can be used with `cargo-generate` as a starter repository:

```bash
cargo install cargo-generate
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

The template config filters local build/runtime artifacts such as `target/`, `data/`, `tmp/`, frontend `node_modules/`, `dist/`, and generated OpenAPI output.

The generated project intentionally keeps source identifiers such as `aster_forge` until initialization. That keeps this repository buildable as a normal Rust project while still supporting `cargo generate` as a clean copy path. Run `./init.sh --help` for non-interactive options, then use the checklist in [developer-docs/en/README.md](developer-docs/en/README.md) for any remaining product-specific branding.

After initialization, regenerate lockfiles with `cargo generate-lockfile` and `cd frontend-panel && bun install`.

## Important Endpoints

```text
GET  /health
GET  /health/ready
GET  /health/metrics                    # with --features metrics

GET  /api/v1/system/info

POST /api/v1/auth/setup
POST /api/v1/auth/register
POST /api/v1/auth/login
POST /api/v1/auth/refresh
POST /api/v1/auth/logout
GET  /api/v1/auth/me
GET  /api/v1/auth/sessions

GET  /api/v1/external-auth/providers
POST /api/v1/external-auth/{provider}/start
GET  /api/v1/external-auth/{provider}/callback

GET  /api/v1/auth/external-auth/providers
GET  /api/v1/auth/external-auth/{kind}/providers
POST /api/v1/auth/external-auth/{kind}/{provider}/start
GET  /api/v1/auth/external-auth/{kind}/{provider}/callback

GET    /api/v1/admin/config
GET    /api/v1/admin/config/schema
GET    /api/v1/admin/config/{key}
PUT    /api/v1/admin/config/{key}
DELETE /api/v1/admin/config/{key}

GET  /api/v1/admin/audit-logs

GET  /api/v1/admin/tasks
POST /api/v1/admin/tasks/cleanup
POST /api/v1/admin/tasks/{id}/retry

GET    /api/v1/admin/external-auth/provider-kinds
GET    /api/v1/admin/external-auth/providers
POST   /api/v1/admin/external-auth/providers
GET    /api/v1/admin/external-auth/providers/{id}
PATCH  /api/v1/admin/external-auth/providers/{id}
DELETE /api/v1/admin/external-auth/providers/{id}
POST   /api/v1/admin/external-auth/providers/test
POST   /api/v1/admin/external-auth/providers/{id}/test
```

In debug builds with the `openapi` feature, the project can export a static OpenAPI document:

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## Configuration Model

Static config is loaded from `data/config.toml` by default and can be overridden with `ASTER__...` environment variables:

```bash
ASTER__SERVER__HOST=0.0.0.0
ASTER__SERVER__PORT=3000
ASTER__SERVER__START_MODE=primary
ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc'
ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret'
```

See [config.example.toml](config.example.toml) for the full static config shape.

Runtime config lives in `system_config` and is edited through the Admin Config API/UI. Use runtime config for values that should change without editing `config.toml`; use static config for boot-critical settings such as database URL, bind address, and secrets.

## Development Commands

Backend:

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo clippy --tests -- -D warnings
cargo test
cargo run
```

Frontend:

```bash
cd frontend-panel
bun install
bun run check
bun run test
bun run build
bun run dev
```

OpenAPI and generated frontend API types:

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## Features

```text
server               default service build
cli                  reserved CLI feature
metrics              Prometheus metrics and system metrics
openapi              OpenAPI schemas and debug API docs support
full                 server + cli + metrics + openapi
jemalloc             use jemalloc allocator
jemalloc-stats       expose jemalloc stats support
jemalloc-profiling   enable jemalloc profiling support
```

## Template Extension Rules

When building a product from AsterForge:

1. Add product entities in `src/entities/` and migrations in `migration/`.
2. Put DB access in `src/db/repository/`.
3. Put business behavior in `src/services/`.
4. Put HTTP contracts in `src/api/dto/` and routes under `src/api/routes/`.
5. Register new OpenAPI paths and schemas in `src/api/openapi.rs`.
6. Add audit events for admin or security-relevant state changes.
7. Add task kinds only when the task has a clear owner, retry model, and visibility model.
8. Keep foundation modules generic. Product-specific workflows belong in the product repository.

## Reference Implementation

For a larger example of extending the same style of runtime foundation into a product, see AsterDrive. Treat it as an implementation reference for module boundaries, task integration, audit coverage, and operational surfaces, not as a list of modules that should be copied into this template.

## License

MIT. See [LICENSE](LICENSE).

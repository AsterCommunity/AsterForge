# Getting Started

This page starts AsterForge from a clean checkout and verifies the backend, frontend assets, database migrations, and first-admin setup.

## Requirements

You need:

- Rust toolchain from the repository `rust-toolchain.toml`.
- Bun for frontend dependencies, builds, and docs.
- SQLite. The default local configuration uses SQLite and does not require another database service.

Optional:

- Docker and Docker Compose for container deployment checks.
- `cargo-generate` for generating a new project from this repository.

## Build Frontend Assets

The backend embeds static assets from `frontend-panel/dist`. Build them before the first run:

```bash
cd frontend-panel
bun install
bun run build
cd ..
```

If you only change backend code, you do not need to rebuild frontend assets every time. Rebuild when frontend pages, generated API types, or static assets change.

## Start The Service

Run from the repository root:

```bash
cargo run
```

The first startup will:

- Create `data/config.toml` if it does not exist.
- Resolve relative runtime paths under `data/`.
- Create the default SQLite database.
- Run SeaORM migrations.
- Insert default runtime config rows into `system_config`.
- Start the HTTP service on `127.0.0.1:3000`.

Open:

```text
http://127.0.0.1:3000
```

Health checks:

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/health/ready
```

## Create The First Admin

Create the first administrator account:

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

This setup endpoint should only succeed while no administrator exists. After that, admin pages and Admin APIs require an administrator session.

## Local HTTP Cookies

Production deployments should run behind HTTPS and keep secure cookies enabled. For local HTTP-only testing:

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

Use this only for local development. Do not enable it in production.

## Common Checks

Backend:

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo test
```

Frontend:

```bash
cd frontend-panel
bun run check
bun run build
```

Docs:

```bash
cd docs
bun install
bun run docs:dev
bun run docs:build
```

## Export OpenAPI

With the `openapi` feature in debug builds:

```bash
cargo test --features openapi generate_openapi
```

Then regenerate the frontend service layer:

```bash
cd frontend-panel
bun run generate-api
```

When adding an API, register its OpenAPI annotations on the backend, export the schema, and regenerate frontend types. The admin panel should not rely on handwritten API type guesses.

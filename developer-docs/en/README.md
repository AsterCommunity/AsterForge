# AsterForge Developer Guide

AsterForge is a foundation repository. Treat it as shared runtime code first and a product starter second. Generic concerns belong here; product workflows belong in the product repository that needs them.

## Core Principles

- Keep foundation modules domain-neutral.
- Prefer existing service, repository, DTO, and OpenAPI patterns over new local frameworks.
- Do not add ordinary user task APIs unless the product has a real task visibility model.
- Do not add `api::subcode` or another client-visible subcode system. Extend `AsterErrorCode` instead.
- Add audit records for administrator actions, security-sensitive changes, and runtime lifecycle events.
- Add OpenAPI schemas and generated frontend types whenever an API contract changes.
- Keep migrations append-only once a project has shipped.

## Backend Extension Path

Use this shape for new backend functionality:

```text
src/entities/                  SeaORM model
migration/                     schema migration
src/db/repository/             database access
src/services/                  business behavior
src/api/dto/                   request/response DTOs
src/api/routes/                HTTP handlers and route registration
src/api/openapi.rs             OpenAPI paths and schemas
tests/                         integration coverage
frontend-panel/src/services/   frontend service wrapper
frontend-panel/src/pages/      admin UI when needed
```

Do not put query logic in handlers. Handlers should validate HTTP input, call services, and return the common response envelope.

## Runtime Startup

Startup is split under `src/runtime/startup/`:

- `common.rs` prepares runtime directories, metrics, database handles, migrations, runtime config, cache, and audit manager.
- `primary.rs` builds primary runtime state.
- `follower.rs` builds follower runtime state.
- `mod.rs` selects by `config.server.start_mode` and records `server_start`.

`server.start_mode = "primary"` runs dispatcher and maintenance loops. `server.start_mode = "follower"` keeps common service state but skips primary-only runtime background tasks.

## Graceful Shutdown

Shutdown is coordinated from `src/main.rs` and `src/runtime/shutdown.rs`:

1. Wait for SIGINT/SIGTERM.
2. Cancel the shared shutdown token.
3. Stop Actix gracefully.
4. Record `server_shutdown`.
5. Stop background tasks with a grace period.
6. Flush audit logs.
7. Close database handles.

When adding long-running workers, they must observe the shutdown token and leave persisted state resumable.

## Background Tasks

The task system lives in `src/services/task_service/` and `src/runtime/tasks.rs`.

Current template task kinds are only `system_runtime` tasks. Admin APIs can list, retry, and clean up tasks; ordinary user task APIs are intentionally absent.

Key contracts:

- Claiming is token-fenced with `processing_token`.
- Workers renew leases through heartbeat updates.
- Stale workers must not overwrite a newer lease.
- Graceful shutdown releases processing work back to `retry` without spending retry budget or writing business failure details.
- Task presentation uses stable message codes so the frontend does not parse task payloads or result blobs.
- Add task integration tests for claim, retry, cleanup, and shutdown behavior when changing dispatch semantics.

If a product adds a domain task kind, define its payload/result types, registry entry, retry classification, initial steps, presentation, and visibility rules together.

Mail outbox delivery is also a system runtime task. See [Mail Runtime Extension](./mail-runtime.md) for the concrete extension rules.

## Audit Service

Audit code lives in `src/services/audit_service/`.

Use audit for:

- server start and shutdown
- login and security-relevant auth changes
- admin config changes
- admin external auth provider changes
- admin task retry and cleanup
- mail send and mail delivery failure
- product-specific administrator state changes

Audit entries should include structured details and presentation metadata. Frontend code should display `presentation` first and use raw `details` only as a fallback/debug surface.

Mail audit details, presentation, and tests are covered in [Mail Runtime Extension](./mail-runtime.md).

## API And Errors

All API responses use the common envelope in `src/api/response.rs`.

Client-facing failures should expose stable `AsterErrorCode` values. The old internal `E001`-style codes are for server diagnostics only. Do not introduce a second public subcode layer in foundation modules.

When changing API contracts:

1. Update DTOs and route annotations.
2. Register paths and schemas in `src/api/openapi.rs`.
3. Run OpenAPI generation.
4. Regenerate frontend API types.
5. Update frontend service/page code.

Commands:

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## Frontend Extension Path

The frontend lives in `frontend-panel/`.

Use:

- `src/services/` for API wrappers
- `src/types/api.ts` for re-exported generated API types
- `src/lib/presentation.ts` for stable audit/task display formatting
- `src/pages/admin/` for administrator pages
- existing table/form/page shell components before adding new UI primitives

Admin screens should stay dense, predictable, and operational. Avoid marketing-style layouts inside the admin panel.

## Testing

Useful commands:

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo clippy --tests -- -D warnings
cargo test

cd frontend-panel
bun run check
bun run test
bun run build
```

Targeted commands used often:

```bash
cargo test --test test_admin_tasks
cargo test --test test_audit
cargo test --test test_audit mail_outbox_dispatch_records_delivery_audit_logs
cargo test mail_template
cargo test task_service::presentation
cargo test shutdown_release_returns_processing_task_to_retry_without_failure_update
```

## Using cargo-generate

The root `cargo-generate.toml` makes this repository usable as a `cargo generate` source while ignoring build/runtime artifacts:

```bash
cargo install cargo-generate
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

The generated project still contains identifiers such as `aster_forge` and `AsterForge` until initialization. Run `./init.sh --help` for non-interactive options.

Because initialization can update package names inside lockfiles, regenerate them afterwards with `cargo generate-lockfile` and `cd frontend-panel && bun install`.

Recommended follow-up checklist:

- `Cargo.toml`: package name, description, repository, binary naming if changed.
- `Dockerfile`: binary path, image labels, database default path.
- `docker-compose.yml`: image name, service name, database default path.
- `README.md` and `README.zh.md`: project name, description, links.
- `frontend-panel/package.json`: package name.
- `frontend-panel/src/config/app.ts`: frontend display defaults.
- `config.example.toml`: default database path and service-facing examples.
- `src/api/openapi.rs`: API title and description.
- `src/main.rs`: startup log labels if the product should not say AsterForge.

After renaming, run backend and frontend checks before adding domain code.

## Boundary Checklist

Before adding a module to AsterForge, ask:

- Is this useful to most future Aster services?
- Can it be configured without product assumptions?
- Does it avoid importing product-specific business concepts into the shared foundation?
- Does it have tests at the service/repository/API level that match the blast radius?
- Does the frontend get stable presentation data instead of parsing backend internals?

If the answer is no, put it in the product repository.

For a concrete product-scale reference, inspect AsterDrive after understanding this foundation. Use it to study extension patterns, not to decide what belongs in AsterForge.

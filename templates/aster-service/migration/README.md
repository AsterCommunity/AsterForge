# {{project-name}} Migrations

This crate owns the database migration chain for the generated service.

## Foundation Migration

The first migration is `m20260627_000001_forge_foundation_schema`. It creates the
Forge-owned infrastructure tables used by the generated runtime components:

- `runtime_leases`
- `scheduled_tasks`
- `system_config`
- `mail_outbox`
- `audit_logs`

These tables are product-neutral. Their schema builders live in `aster_forge_db` so multiple Aster
services can share the same lease, task, config, mail outbox, and audit storage mechanics.

Do not edit an applied migration in a real deployment. Add a new migration module for later schema
changes.

## Product Tables

Product-specific tables belong in this crate too, but they should be added as new migration modules
after the Forge foundation migration.

Typical flow:

1. Create a new file under `src/`, for example `m20260627_000002_product_schema.rs`.
2. Declare it in `src/lib.rs`.
3. Register it after `m20260627_000001_forge_foundation_schema` in `Migrator::migrations()`.
4. Keep product entities, repositories, permissions, and API semantics in the product crate.

Forge provides shared schema/store mechanics only where Aster services intentionally share the same
behavior.

## Commands

Run all pending migrations:

```bash
cargo run -p migration -- up
```

Check migration status:

```bash
cargo run -p migration -- status
```

Rollback the latest migration:

```bash
cargo run -p migration -- down
```

Rollback a fixed number of migrations:

```bash
cargo run -p migration -- down -n 2
```

Drop all tables and reapply all migrations in local development:

```bash
cargo run -p migration -- fresh
```

Reset all applied migrations:

```bash
cargo run -p migration -- reset
```

## Runtime Startup

The generated service also runs `Migrator::up(...)` during database preparation. That keeps local
development simple: `cargo run` can create the Forge foundation tables automatically for the
configured database URL.

Production deployments may still run this migration crate explicitly in release pipelines before
starting the service.

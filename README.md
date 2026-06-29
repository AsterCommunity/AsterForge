<p align="center">
  <img src="docs/public/favicon.svg" alt="AsterForge" width="112" />
</p>

<h1 align="center">AsterForge</h1>

<p align="center">
  Shared runtime foundation and infrastructure kernel for Aster services.
  <br />
  Product-neutral components, schemas, stores, and lifecycle mechanics for AsterDrive, AsterYggdrasil, and future Aster projects.
</p>

<p align="center">
  <a href="https://forge.astercosm.com/"><img alt="Documentation Site" src="https://img.shields.io/badge/docs-VitePress-0F766E?logo=vitepress&logoColor=white"></a>
  <a href="https://codecov.io/github/AsterCommunity/AsterForge"><img alt="Coverage" src="https://codecov.io/github/AsterCommunity/AsterForge/graph/badge.svg?token=IefDQVj2y6"></a>
  <a href="docs/guide/index.md"><img alt="Chinese Guide" src="https://img.shields.io/badge/guide-中文-E11D48"></a>
  <a href="docs/en/index.md"><img alt="English Overview" src="https://img.shields.io/badge/overview-English-2563EB"></a>
  <a href="docs/crates/aster_forge_actix_middleware.md"><img alt="Crate Docs" src="https://img.shields.io/badge/crates-reference-059669"></a>
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/rust-1.94%2B-B7410E?logo=rust&logoColor=white">
  <img alt="License MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-0F172A">
</p>

## What is AsterForge?

AsterForge is the shared runtime foundation for Aster products. It is no longer just a place for duplicated helper functions; it is the product-neutral infrastructure kernel that Aster services plug into for lifecycle, component registration, health reporting, shutdown ordering, configuration reload, cache backends, database-owned infrastructure tables, mail outbox dispatch, audit log mechanics, scheduled tasks, runtime leases, logging, metrics, panic handling, API helpers, Actix middleware, external-auth connectors, storage key helpers, and validation.

Forge is not a product business framework. Product-specific code, permissions, user-facing API semantics, product entities, task payloads/results, audit action enums, presentation rules, and business repositories should stay in the owning application repositories. Product-neutral runtime mechanics, common database schemas/stores, component graph contracts, retry/claim/lease rules, and cross-process coordination belong in Forge when multiple Aster services need the same behavior.

The target shape for new products is a thin product entrypoint:

```rust
aster_forge_runtime::AsterRuntime::builder()
    .component(http_component(...))
    .component(database_component(...))
    .component(background_task_component(...))
    .component(mail_outbox_component(...))
    .component(audit_component(...))
    .run()
    .await?;
```

Product code still owns resource creation and business semantics; Forge owns the reusable lifecycle and persistence mechanics behind those components.

All crate names use the `aster_forge_*` prefix. The workspace targets Rust `1.94.0+`, edition 2024, and uses dual `MIT OR Apache-2.0` license metadata.

## Crates

| Area | Crates |
| --- | --- |
| Runtime kernel | [`aster_forge_runtime`](docs/crates/aster_forge_runtime.md), [`aster_forge_config`](docs/crates/aster_forge_config.md), [`aster_forge_logging`](docs/crates/aster_forge_logging.md), [`aster_forge_metrics`](docs/crates/aster_forge_metrics.md), [`aster_forge_panic`](docs/crates/aster_forge_panic.md), [`aster_forge_alloc`](docs/crates/aster_forge_alloc.md) |
| Web and API | [`aster_forge_api`](docs/crates/aster_forge_api.md), [`aster_forge_api_docs_macros`](docs/crates/aster_forge_api_docs_macros.md), [`aster_forge_actix_middleware`](docs/crates/aster_forge_actix_middleware.md), [`aster_forge_actix_observability`](docs/crates/aster_forge_actix_observability.md), [`aster_forge_external_auth`](docs/crates/aster_forge_external_auth.md) |
| Data, coordination, and background work | [`aster_forge_db`](docs/crates/aster_forge_db.md), [`aster_forge_cache`](docs/crates/aster_forge_cache.md), [`aster_forge_tasks`](docs/crates/aster_forge_tasks.md), [`aster_forge_mail`](docs/crates/aster_forge_mail.md), [`aster_forge_audit`](docs/crates/aster_forge_audit.md) |
| Storage and domain-neutral helpers | [`aster_forge_storage_core`](docs/crates/aster_forge_storage_core.md), [`aster_forge_file_classification`](docs/crates/aster_forge_file_classification.md) |
| Utilities | [`aster_forge_crypto`](docs/crates/aster_forge_crypto.md), [`aster_forge_utils`](docs/crates/aster_forge_utils.md), [`aster_forge_validation`](docs/crates/aster_forge_validation.md) |

## Integration rules

- Keep product permissions, user-facing errors, business repositories, API semantics, task payloads/results, audit actions/details, and presentation rules in the product repository.
- Use Forge-owned schema/store builders for product-neutral infrastructure tables such as runtime leases, scheduled tasks, mail outbox, and audit logs.
- Register runtime subsystems through Forge components instead of hand-writing shutdown order in product entrypoints.
- Map Forge errors at the product service boundary.
- Write explicit product-side adapters for metrics, runtime config, permission, audit presentation, and policy decisions.
- Use AsterDrive and AsterYggdrasil as references, not as reasons to move business logic into Forge.

See [`docs/guide/new-project-integration.md`](docs/guide/new-project-integration.md) for the target new-product shape and [`docs/guide/integration-principles.md`](docs/guide/integration-principles.md) for the detailed boundary rules.

## Service template

New Aster services can start from the bundled `cargo generate` template:

```bash
cargo generate --git https://github.com/AsterCommunity/AsterForge.git \
  templates/aster-service \
  --name aster_product_service \
  --define server_port=3000
```

The template wires a thin product entrypoint to Forge runtime components, exposes Yggdrasil-style boot parameters for server/database/cache/config-sync/logging, includes a migration crate for Forge-owned infrastructure tables, and uses Cargo metadata such as `env!("CARGO_PKG_NAME")` for process, health, panic, and placeholder mail display names. Product repositories still own their business routes, product migrations, config registry, audit enums/details, task payloads/results, and mail template rendering.

## Documentation

- [Documentation site](https://forge.astercosm.com/)
- [Chinese guide](docs/guide/index.md)
- [New project integration guide](docs/guide/new-project-integration.md)
- [English overview](docs/en/index.md)
- [Crate reference pages](docs/crates/aster_forge_actix_middleware.md)
- [Reference projects](docs/guide/reference-projects.md)

The Chinese crate pages are currently the authoritative integration reference. English pages provide entry points while the crate-by-crate documentation is being mirrored.

## Development

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all
```

Documentation site:

```bash
cd docs
bun install
bun run docs:dev
```

## Project structure

```text
crates/                 Rust workspace crates
docs/                   VitePress documentation site and crate reference pages
developer-docs/         Compatibility entry points for developer documentation
scripts/                Repository maintenance scripts
templates/              cargo-generate templates for new Aster services
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option, as declared in the workspace package metadata.

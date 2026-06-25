<p align="center">
  <img src="docs/public/favicon.svg" alt="AsterForge" width="112" />
</p>

<h1 align="center">AsterForge</h1>

<p align="center">
  Shared Rust infrastructure crates for Aster services.
  <br />
  Product-neutral mechanisms extracted from AsterDrive, AsterYggdrasil, and future Aster projects.
</p>

<p align="center">
  <a href="https://forge.astercosm.com/"><img alt="Documentation Site" src="https://img.shields.io/badge/docs-VitePress-0F766E?logo=vitepress&logoColor=white"></a>
  <a href="https://codecov.io/github/AsterCommunity/AsterForge"><img alt="Coverage" src="https://codecov.io/github/AsterCommunity/AsterForge/graph/badge.svg?token=IefDQVj2y6"></a>
  <a href="docs/guide/index.md"><img alt="Chinese Guide" src="https://img.shields.io/badge/guide-中文-E11D48"></a>
  <a href="docs/en/index.md"><img alt="English Overview" src="https://img.shields.io/badge/overview-English-2563EB"></a>
  <a href="docs/crates/aster_forge_actix_middleware.md"><img alt="Crate Docs" src="https://img.shields.io/badge/crates-reference-059669"></a>
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/rust-1.94%2B-B7410E?logo=rust&logoColor=white">
  <img alt="License MIT" src="https://img.shields.io/badge/license-MIT-0F172A">
</p>

## What is AsterForge?

AsterForge is the shared Rust crate workspace and runtime foundation for Aster projects. It collects low-domain infrastructure such as API helpers, Actix middleware, cache backends, database utilities, runtime configuration primitives, external-auth building blocks, logging, metrics, task mechanics, storage helpers, and validation.

Forge is not a product business framework. Product-specific code, SeaORM entities, migrations, permissions, business repositories, storage policies, task payloads, and user-facing API semantics should stay in the owning application repositories. Shared lifecycle mechanics, component registration, and runtime reporting belong in Forge when multiple Aster services need the same behavior.

All crate names use the `aster_forge_*` prefix. The workspace targets Rust `1.94.0+`, edition 2024, and uses MIT license metadata.

## Crates

| Area | Crates |
| --- | --- |
| API and web | [`aster_forge_api`](docs/crates/aster_forge_api.md), [`aster_forge_api_docs_macros`](docs/crates/aster_forge_api_docs_macros.md), [`aster_forge_actix_middleware`](docs/crates/aster_forge_actix_middleware.md), [`aster_forge_external_auth`](docs/crates/aster_forge_external_auth.md) |
| Runtime | [`aster_forge_alloc`](docs/crates/aster_forge_alloc.md), [`aster_forge_config`](docs/crates/aster_forge_config.md), [`aster_forge_logging`](docs/crates/aster_forge_logging.md), [`aster_forge_metrics`](docs/crates/aster_forge_metrics.md), [`aster_forge_panic`](docs/crates/aster_forge_panic.md) |
| Data and tasks | [`aster_forge_cache`](docs/crates/aster_forge_cache.md), [`aster_forge_db`](docs/crates/aster_forge_db.md), [`aster_forge_storage_core`](docs/crates/aster_forge_storage_core.md), [`aster_forge_tasks`](docs/crates/aster_forge_tasks.md) |
| Utilities | [`aster_forge_crypto`](docs/crates/aster_forge_crypto.md), [`aster_forge_file_classification`](docs/crates/aster_forge_file_classification.md), [`aster_forge_utils`](docs/crates/aster_forge_utils.md), [`aster_forge_validation`](docs/crates/aster_forge_validation.md) |

## Integration rules

- Keep product models, permissions, repositories, migrations, and user-facing errors in the product repository.
- Map Forge errors at the product service boundary.
- Write explicit product-side adapters for persistence, metrics, runtime config, and policy decisions.
- Start with small helper crates, then adopt lifecycle and runtime crates once integration tests cover the boundary.
- Use AsterDrive and AsterYggdrasil as references, not as reasons to move business logic into Forge.

See [`docs/guide/integration-principles.md`](docs/guide/integration-principles.md) for the detailed rules.

## Documentation

- [Documentation site](https://forge.astercosm.com/)
- [Chinese guide](docs/guide/index.md)
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
```

## License

MIT, as declared in the workspace package metadata.

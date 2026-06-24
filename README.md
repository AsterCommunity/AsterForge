# AsterForge

AsterForge collects shared Rust crates used by Aster projects.

Crate names use the `aster_forge_*` prefix. This repository is for low-domain
infrastructure such as OpenAPI helper macros, cache backends, and database
utilities. Product-specific code, SeaORM entities, migrations, and business
repositories should stay in their owning application repositories.

Current crates:

- `aster_forge_api_docs_macros`
- `aster_forge_api`
- `aster_forge_cache`
- `aster_forge_crypto`
- `aster_forge_db`
- `aster_forge_file_classification`
- `aster_forge_logging`
- `aster_forge_storage_core`
- `aster_forge_utils`
- `aster_forge_validation`

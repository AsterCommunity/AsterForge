# AGENTS.md

This file is for agents working in the AsterForge repository. Understand the project boundaries before changing code. Forge is no longer a dumping ground for duplicated functions; it is the shared runtime foundation for Aster products. That still does not make it a place for product business logic, because pulling that logic back out later is a miserable job.

## Project Positioning

AsterForge is the shared Rust crate workspace and product-neutral runtime kernel for Aster projects. It consolidates reusable infrastructure mechanics, component lifecycle management, shared schemas and stores, background-task runtime mechanics, configuration synchronization, cache backends, mail outbox handling, audit-log mechanics, and health and shutdown reporting.

It is evolving into a common framework foundation for Aster products, but it is not a repository for relocating the business layers of AsterDrive or AsterYggdrasil. Product repositories continue to own business semantics; Forge owns reusable mechanics.

Forge should own:

- Lifecycle infrastructure such as `AsterRuntime`, the runtime component registry, health checks, startup and shutdown phases, and signal handling.
- Product-neutral API helpers, pagination, cursors, sorting, and OpenAPI macros.
- Infrastructure mechanics such as caching, database connections, transactions, retries, metrics, logging, and panic hooks.
- Product-neutral database schemas, index builders, and stores for runtime leases, the scheduled-task catalog, mail outbox, and audit logs.
- Generic background-task mechanics such as leases, heartbeats, dispatch, runtime workers, step state, and scheduled-task due-run claims.
- Product-neutral mail mechanics such as senders, outbox dispatch, retry decisions, and template catalogs.
- Product-neutral audit mechanics such as the audit runtime component and audit-log write, query, count, and delete operations.
- Runtime configuration infrastructure such as the configuration registry, structured value conversion, runtime snapshots, and reload notifications.
- Reusable utilities for validation, cryptography, storage keys, and S3 endpoint normalization.

Forge should not own:

- Product business entities, product API DTOs, product permission models, or user, team, and organization rules.
- Historical product migrations. New migrations may call Forge schema builders, but migration ownership must remain in the product repository.
- Product repository SQL. Forge may own product-neutral store and query mechanics such as audit cursor queries, mail-outbox claims, and scheduled-task claims.
- Product audit-action enums, detail schemas, presentation, permissions, or business-statistics definitions.
- Product business state machines.
- Product API error text, status codes, localization, or frontend presentation policy.
- AsterDrive- or AsterYggdrasil-specific task kinds, payloads and results, storage policies, or external-auth account-binding rules.
- Thin wrappers with no semantic value that exist only to make code look uniform.

## Required Reading Before Making Changes

Before implementing anything, read the documentation first, then the code, and only then inspect downstream replacement points. Do not reverse that order. Do not be lazy, kitty.

1. Read the entry-point documentation:
   - `README.md`
   - `docs/guide/index.md`
   - `docs/guide/new-project-integration.md`
   - `docs/guide/integration-principles.md`
   - `docs/guide/reference-projects.md`

2. Read the relevant crate documentation:
   - `docs/crates/aster_forge_api.md`
   - `docs/crates/aster_forge_config.md`
   - `docs/crates/aster_forge_tasks.md`
   - Or the `docs/crates/*.md` pages directly involved in the task.

3. Read the corresponding crate code:
   - `crates/<crate>/Cargo.toml`
   - `crates/<crate>/src/lib.rs`
   - `crates/<crate>/src/**/*.rs`
   - `crates/<crate>/tests/**/*.rs`

4. Only then inspect reference projects or downstream integration points:
   - Prefer AsterYggdrasil for lightweight integrations with clear boundaries.
   - Use AsterDrive for complete but more complex integrations.

Reference projects may confirm an integration pattern. They are not justification for moving business logic into Forge.

## Integration and Replacement Workflow

For tasks such as "integrate Forge," "replace the existing function," or "extract a shared module," map the replacement relationship before changing code. Do not ask only whether functions are duplicated. Also determine whether Forge should own a complete mechanism consisting of a component, schema builder, store, runner, registry, and query layer.

Before replacing anything, confirm at least the following:

- Does the existing function or module implement shared mechanics or product semantics?
- Should the work extract only a function, or a complete mechanism such as a component, schema or index builder, store, runner, registry, hook, and test model?
- Does Forge already provide an equivalent API?
- At which layer should Forge errors be mapped into product errors?
- Does the product need to retain an adapter, trait implementation, metrics, audit behavior, or permission check?
- Which existing behaviors need test coverage to prevent semantic drift during replacement?

Before editing, write down these four columns:

```text
Old function/module -> Forge API/component/schema/store -> Product-owned responsibility -> Required behavior tests
```

If an old function is a meaningless thin wrapper that only calls Forge without mapping errors, injecting configuration, recording metrics, or expressing product semantics, remove the wrapper and call Forge directly.

If an old function carries a product-boundary responsibility such as error mapping, configuration injection, audit, metrics, or permission checks, retain the product-side adapter and keep those responsibilities out of Forge.

## Code-Boundary Rules

- Shared mechanics and the product-neutral runtime foundation belong in Forge; product semantics remain in product repositories.
- Forge error types describe infrastructure or mechanism failures only. Product API layers decide status codes, messages, and error envelopes.
- Write trait adapters explicitly in product code. Do not rely on hidden global state or make product crates depend on Forge implementation details.
- Do not introduce global singletons, implicit registries, or untestable static product state merely to remove a few lines of code.
- Do not make `aster_forge_api` depend on Actix, Axum, or concrete product entities.
- `aster_forge_db` may own product-neutral infrastructure tables and stores such as runtime leases, scheduled tasks, mail outbox, and audit logs. It must not own product business entities, historical product migrations, or business repository queries.
- Do not let `aster_forge_tasks` define product task kinds, payloads and results, administration APIs, or concrete task implementations.
- Do not let `aster_forge_config` define product configuration keys, categories, i18n text, administration APIs, or business normalizers.
- Do not copy Drive or Yggdrasil business enums, permission rules, or audit fields into Forge.

## Crate Adoption Order

Integrate Forge in foundation order instead of treating it as a loose utility collection:

- Entry-point foundation: `aster_forge_runtime`, `aster_forge_logging`, `aster_forge_metrics`, `aster_forge_panic`, and `aster_forge_alloc`.
- Data and coordination: `aster_forge_db`, `aster_forge_cache`, and `aster_forge_config`.
- Background mechanics: `aster_forge_tasks`, `aster_forge_mail`, and `aster_forge_audit`.
- Web and API: `aster_forge_api`, `aster_forge_api_docs_macros`, `aster_forge_actix_middleware`, and `aster_forge_external_auth`.
- Utility and storage foundation: `aster_forge_validation`, `aster_forge_utils`, `aster_forge_crypto`, `aster_forge_file_classification`, and `aster_forge_storage_core`.

Be more careful when integrating high-impact modules because they affect startup, shutdown, error handling, concurrency, test isolation, and data consistency. Prefer the final intended shape; do not retain compatibility facades that have no boundary value.

## Documentation Synchronization

Adding or changing a public API normally requires documentation updates. Each crate document should retain the following structure:

- Purpose and ownership boundary.
- Appropriate use cases.
- Cargo features and integration method.
- Minimal integration example.
- New-project or runtime-component integration shape.
- Error boundary.
- Testing requirements.
- Reference projects.

When code and documentation disagree, determine whether the code is wrong or the documentation is stale. Do not update only one side by reflex.

## Rust Conventions

- The workspace uses Rust 1.94+ and edition 2024.
- Prefer declaring dependencies in the root `Cargo.toml` under `[workspace.dependencies]`, then reference workspace dependencies from individual crates as needed.
- Name new crates with the `aster_forge_*` prefix.
- Public APIs must have explicit boundaries and should avoid exposing implementation details.
- Do not use `unwrap`, `expect`, `panic`, `todo`, or `unimplemented` by default. Most crates deny these Clippy lints in non-test builds.
- Use unsafe only when genuinely necessary and include an accurate `SAFETY:` explanation. `aster_forge_alloc` applies stricter unsafe requirements.
- Prefer `thiserror` for error types and expose classifications that product layers can map. Do not hard-code product-facing text in Forge.
- Test code may be more direct, but it must not hide real boundary problems.

## Testing and Validation

Common commands:

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all
```

Choose validation according to the change scope:

- Documentation-only changes: verify links, paths, and crate names.
- Pure-function helpers: run the relevant crate tests and, when needed, a workspace check.
- Public API or feature changes: compile the relevant crate with its default and target feature sets.
- Task, configuration, cache, database, or external-auth changes: run at least the relevant crate tests and `cargo check --workspace`; use `cargo test --workspace` for higher-risk changes.
- Foundation changes involving runtime components, schemas and stores, tasks, mail, audit, configuration, or cache: run the relevant crate tests and Clippy. If Yggdrasil is integrated, run its corresponding tests through a local patch as well.

High-impact integrations must test at least:

- Success paths.
- Failure paths.
- Error-mapping boundaries.
- Retry, degradation, or cancellation behavior.
- Concurrency, lease, token-fence, or shutdown behavior.

## Code Review Fixes

When the user provides review comments from Greptile, CodeRabbit, Gemini, or similar tools:

1. Classify each comment as a real issue or a false positive.
2. Fix real issues first. Do not change correct code merely to satisfy a bot.
3. Run compilation or tests after each batch of fixes.
4. In the final response, state which comments were fixed, which were false positives, and which validation commands were run.

## Working Style

- Read existing patterns before changing code. If the pattern is unclear, keep investigating rather than guessing.
- Prefer `rg` and `rg --files` for searches.
- Keep changes focused and avoid opportunistic refactors.
- Do not revert the user's existing changes.
- Do not edit `target/`, `docs/node_modules/`, or other generated or dependency directories.
- Do not proactively write long usage documents unless explicitly requested, but keep crate documentation current when public APIs change.

If a feature appears extractable but extracting it would prevent the product repository from expressing its own business boundary, leave it in the product.

If a feature is clearly a shared runtime, component, schema, store, runner, or query mechanism used by multiple products, do not extract only a token helper function. Move the complete shared kernel and leave only explicit business boundaries in product code.

# AsterForge Guide

AsterForge is the shared runtime foundation and infrastructure kernel for Aster services. It collects product-neutral Rust crates for lifecycle management, runtime components, shared database stores, background task mechanics, mail outbox dispatch, audit log mechanics, config sync, cache backends, middleware, and validation.

Forge does not own product business semantics. Product repositories still own API routes, permissions, business entities, migrations, audit action/detail schemas, presentation, task payload/result types, and product-specific repositories. Forge owns the reusable mechanics that should not diverge between Aster products.

New services should aim for a thin entrypoint:

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

The Chinese crate pages are the complete reference for now:

- [Integration principles](/guide/integration-principles)
- [Reference projects](/guide/reference-projects)
- [Crate docs](/crates/aster_forge_actix_middleware)

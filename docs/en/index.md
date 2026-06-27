---
layout: home

hero:
  name: AsterForge
  text: Runtime foundation for Aster services
  tagline: Product-neutral Rust crates for lifecycle, runtime components, shared database stores, tasks, mail outbox, audit logs, config sync, middleware, and reusable infrastructure mechanics.
  actions:
    - theme: brand
      text: Guide
      link: /en/guide/
    - theme: alt
      text: Chinese crate docs
      link: /crates/aster_forge_actix_middleware
---

AsterForge is the shared runtime foundation for Aster products. It is not only a collection of helper functions; it owns product-neutral infrastructure mechanics that should behave the same across AsterDrive, AsterYggdrasil, and future services.

The target shape for product entrypoints is:

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

Product repositories still own product state, API routes, permissions, business repositories, product entities, migrations, audit action/detail schemas, and task payload/result types. Forge owns reusable lifecycle mechanics, schema builders, stores, runners, registries, hooks, and cross-process coordination.

The Chinese crate pages are currently the authoritative integration reference. English pages provide entry points and project context while the crate-by-crate documentation is being mirrored.

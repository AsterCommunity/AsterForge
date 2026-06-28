---
layout: home

hero:
  name: AsterForge
  text: Aster 产品共用的运行时地基
  tagline: 把 AsterDrive、AsterYggdrasil 和后续服务中的生命周期、组件注册、数据库基础表、任务、邮件、审计、配置同步和中间件收敛到清晰、可测试、可组合的 Rust crates。
  actions:
    - theme: brand
      text: 开始接入
      link: /guide/
    - theme: alt
      text: 查看模块
      link: /crates/aster_forge_actix_middleware

features:
  - title: Runtime component 优先
    details: 新产品入口应该是创建资源、注册 component、运行 AsterRuntime，而不是手写 task shutdown、mail drain、audit flush 和 db close。
  - title: 公共状态机下沉
    details: runtime lease、scheduled task、mail outbox、audit log、config sync 等产品无关状态机由 Forge 承接，产品侧只保留业务边界。
  - title: 边界和 feature 显式
    details: Redis、SeaORM 表、runtime worker、mail drain、OpenAPI 等能力按 feature 启用，产品错误、权限、展示和业务 repository 留在产品仓库。
---

## 文档范围

AsterForge 是 Aster 项目的共享 Rust crate workspace 和产品无关运行时内核。这里的文档面向接入方开发者，不是产品用户手册，也不是把 AsterDrive 或 AsterYggdrasil 业务层搬进 Forge 的说明。

常规接入目标是：

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

产品仓库仍然拥有 `AppState`、API route、业务 service、权限、产品实体、migration、audit action/detail 和 task payload/result。Forge 拥有可复用的生命周期、schema builder、store、runner、registry、hook 和跨进程协调机制。

当前覆盖的 crate 按字母顺序排列：

- [`aster_forge_actix_middleware`](./crates/aster_forge_actix_middleware.md)
- [`aster_forge_actix_observability`](./crates/aster_forge_actix_observability.md)
- [`aster_forge_alloc`](./crates/aster_forge_alloc.md)
- [`aster_forge_api`](./crates/aster_forge_api.md)
- [`aster_forge_api_docs_macros`](./crates/aster_forge_api_docs_macros.md)
- [`aster_forge_audit`](./crates/aster_forge_audit.md)
- [`aster_forge_cache`](./crates/aster_forge_cache.md)
- [`aster_forge_config`](./crates/aster_forge_config.md)
- [`aster_forge_crypto`](./crates/aster_forge_crypto.md)
- [`aster_forge_db`](./crates/aster_forge_db.md)
- [`aster_forge_external_auth`](./crates/aster_forge_external_auth.md)
- [`aster_forge_file_classification`](./crates/aster_forge_file_classification.md)
- [`aster_forge_logging`](./crates/aster_forge_logging.md)
- [`aster_forge_mail`](./crates/aster_forge_mail.md)
- [`aster_forge_metrics`](./crates/aster_forge_metrics.md)
- [`aster_forge_panic`](./crates/aster_forge_panic.md)
- [`aster_forge_runtime`](./crates/aster_forge_runtime.md)
- [`aster_forge_storage_core`](./crates/aster_forge_storage_core.md)
- [`aster_forge_tasks`](./crates/aster_forge_tasks.md)
- [`aster_forge_utils`](./crates/aster_forge_utils.md)
- [`aster_forge_validation`](./crates/aster_forge_validation.md)

## 参考项目

- [AsterDrive](https://github.com/AsterCommunity/AsterDrive)：文件、存储、外部认证、后台任务和运行时服务比较完整，适合作为较复杂接入参考。
- [AsterYggdrasil](https://github.com/AsterCommunity/AsterYggdrasil)：认证站和任务系统较轻，适合看 Forge crate 如何在边界清晰的服务里落地。

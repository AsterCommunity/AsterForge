<p align="center">
  <img src="docs/public/favicon.svg" alt="AsterForge" width="112" />
</p>

<h1 align="center">AsterForge</h1>

<p align="center">
  Aster 服务的共享运行时基座与基础设施内核。
  <br />
  为 AsterDrive、AsterYggdrasil 及未来 Aster 项目提供产品无关的组件、schema、存储与生命周期机制。
</p>

<p align="center">
  <a href="https://forge.astercosm.com/"><img alt="文档站点" src="https://img.shields.io/badge/docs-VitePress-0F766E?logo=vitepress&logoColor=white"></a>
  <a href="https://codecov.io/github/AsterCommunity/AsterForge"><img alt="Coverage" src="https://codecov.io/github/AsterCommunity/AsterForge/graph/badge.svg?token=IefDQVj2y6"></a>
  <a href="https://forge.astercosm.com/guide/index"><img alt="中文指南" src="https://img.shields.io/badge/guide-中文-E11D48"></a>
  <a href="https://forge.astercosm.com/en/index"><img alt="English Overview" src="https://img.shields.io/badge/overview-English-2563EB"></a>
  <a href="https://forge.astercosm.com/crates/aster_forge_actix_middleware"><img alt="Crate 文档" src="https://img.shields.io/badge/crates-reference-059669"></a>
  <img alt="Rust 1.94+" src="https://img.shields.io/badge/rust-1.94%2B-B7410E?logo=rust&logoColor=white">
  <img alt="License MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-0F172A">
</p>

<p align="center">
  <a href="README.md">English</a> | 中文
</p>

## AsterForge 是什么？

AsterForge 是 Aster 产品的共享运行时基座。它最初只是存放公共辅助函数的地方，如今已成为 Aster 服务接入的产品无关基础设施内核，涵盖生命周期管理、组件注册、健康上报、关闭顺序、配置重载、缓存后端、数据库基础设施表、邮件 outbox 分发、审计日志机制、定时任务、运行时租约、日志、指标、panic 处理、API 辅助、Actix 中间件、外部认证连接器、存储 key 辅助和校验能力。

Forge 不是产品业务框架。产品专属代码——权限、面向用户的 API 语义、产品实体、任务 payload/结果、审计 action 枚举、展示规则和业务 repository——留在各自的应用仓库中。Forge 只负责多个 Aster 服务共同需要的部分：产品无关的运行时机制、通用数据库 schema/存储、组件图契约、重试/认领/租约规则，以及跨进程协调。

新产品的目标形态是一个薄入口：

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

产品代码仍然负责资源创建和业务语义，Forge 负责这些组件背后可复用的生命周期与持久化机制。

所有 crate 名称统一使用 `aster_forge_*` 前缀。workspace 面向 Rust `1.94.0+` 和 edition 2024，采用 `MIT OR Apache-2.0` 双许可证。

## Crates

| 领域 | Crates |
| --- | --- |
| 运行时内核 | [`aster_forge_runtime`](https://forge.astercosm.com/crates/aster_forge_runtime)、[`aster_forge_config`](https://forge.astercosm.com/crates/aster_forge_config)、[`aster_forge_logging`](https://forge.astercosm.com/crates/aster_forge_logging)、[`aster_forge_metrics`](https://forge.astercosm.com/crates/aster_forge_metrics)、[`aster_forge_panic`](https://forge.astercosm.com/crates/aster_forge_panic)、[`aster_forge_alloc`](https://forge.astercosm.com/crates/aster_forge_alloc) |
| Web 与 API | [`aster_forge_api`](https://forge.astercosm.com/crates/aster_forge_api)、[`aster_forge_api_docs_macros`](https://forge.astercosm.com/crates/aster_forge_api_docs_macros)、[`aster_forge_actix_middleware`](https://forge.astercosm.com/crates/aster_forge_actix_middleware)、[`aster_forge_actix_observability`](https://forge.astercosm.com/crates/aster_forge_actix_observability)、[`aster_forge_external_auth`](https://forge.astercosm.com/crates/aster_forge_external_auth) |
| 数据、协调与后台任务 | [`aster_forge_db`](https://forge.astercosm.com/crates/aster_forge_db)、[`aster_forge_cache`](https://forge.astercosm.com/crates/aster_forge_cache)、[`aster_forge_tasks`](https://forge.astercosm.com/crates/aster_forge_tasks)、[`aster_forge_mail`](https://forge.astercosm.com/crates/aster_forge_mail)、[`aster_forge_audit`](https://forge.astercosm.com/crates/aster_forge_audit) |
| 存储与领域无关辅助 | [`aster_forge_storage_core`](https://forge.astercosm.com/crates/aster_forge_storage_core)、[`aster_forge_file_classification`](https://forge.astercosm.com/crates/aster_forge_file_classification) |
| 工具类 | [`aster_forge_crypto`](https://forge.astercosm.com/crates/aster_forge_crypto)、[`aster_forge_utils`](https://forge.astercosm.com/crates/aster_forge_utils)、[`aster_forge_validation`](https://forge.astercosm.com/crates/aster_forge_validation) |

## 集成规则

- 产品权限、面向用户的错误、业务 repository、API 语义、任务 payload/结果、审计 action/详情和展示规则，留在产品仓库中。
- 运行时租约、定时任务、邮件 outbox、审计日志等产品无关的基础设施表，统一使用 Forge 提供的 schema/存储构建器。
- 通过 Forge 组件注册运行时子系统，不要在产品入口手写关闭顺序。
- 在产品服务边界处映射 Forge 错误。
- 为指标、运行时配置、权限、审计展示和策略决策编写显式的产品侧适配器。
- 把 AsterDrive 和 AsterYggdrasil 当作参考，而不是把业务逻辑搬进 Forge 的理由。

目标新产品形态见 [`新项目接入指南`](https://forge.astercosm.com/guide/new-project-integration)，详细边界规则见 [`接入原则`](https://forge.astercosm.com/guide/integration-principles)。

## 服务模板

从内置的 `cargo generate` 模板创建新的 Aster 服务：

```bash
cargo generate --git https://github.com/AsterCommunity/AsterForge.git \
  templates/aster-service \
  --name aster_product_service \
  --define server_port=3000
```

该模板将薄产品入口接入 Forge 运行时组件。生成提示只询问包描述和 HTTP 端口。Forge 依赖指向官方 AsterForge Git 仓库，服务器、数据库、缓存、配置同步和日志均采用保守默认值。数据库 URL 和配置同步 topic 按项目名推导，文件日志默认关闭。模板自带 Forge 基础设施表的 migration crate，并使用 `env!("CARGO_PKG_NAME")` 等 Cargo 元数据作为进程、健康检查、panic 和占位邮件的显示名。

构建与 CI 沿用 AsterDrive 的模式：锁定的工具链与分离的 dev profile、隔离的 debug/test 前端回退、发布期前端强制校验、纳入版本控制的 OpenAPI/TypeScript 产物及漂移检查、覆盖率摘要、依赖触发的安全审计，以及 PostgreSQL/MySQL 迁移冒烟任务。产品仓库仍拥有各自的业务路由、产品迁移、配置注册表、审计枚举/详情、任务 payload/结果和邮件模板渲染。

## 文档

- [文档站点](https://forge.astercosm.com/)
- [中文指南](https://forge.astercosm.com/guide/index)
- [新项目集成指南](https://forge.astercosm.com/guide/new-project-integration)
- [English 概览](https://forge.astercosm.com/en/index)
- [Crate 参考页](https://forge.astercosm.com/crates/aster_forge_actix_middleware)
- [参考项目](https://forge.astercosm.com/guide/reference-projects)

目前中文 crate 页面是权威的集成参考；英文页面在逐 crate 文档镜像完成前作为入口使用。

## 开发

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all
```

文档站点：

```bash
cd docs
bun install
bun run docs:dev
```

## 项目结构

```text
crates/                 Rust workspace crates
docs/                   VitePress 文档站点与 crate 参考页
developer-docs/         开发者文档的兼容性入口
scripts/                仓库维护脚本
templates/              新 Aster 服务的 cargo-generate 模板
```

## 许可证

根据 workspace 包元数据声明，可在以下两种许可证中任选其一：

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT license（[LICENSE-MIT](LICENSE-MIT)）

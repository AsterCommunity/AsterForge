---
layout: home

hero:
  name: AsterForge
  text: 共享 Rust crates 的开发与接入文档
  tagline: 把 AsterDrive、AsterYggdrasil 和后续服务中的重复基础设施收敛到清晰、可测试、可组合的 crate。
  actions:
    - theme: brand
      text: 开始接入
      link: /guide/
    - theme: alt
      text: 查看模块
      link: /crates/aster_forge_actix_middleware

features:
  - title: 模块边界明确
    details: Forge 只承载跨项目复用的底层机制，业务实体、权限规则、存储策略和产品流程继续留在产品仓库。
  - title: 接入路径可追踪
    details: 每个 crate 文档都列出适用场景、功能开关、最小接入方式、测试要求和 Drive/Yggdrasil 参考位置。
  - title: 面向严格项目
    details: 文档默认以 no unwrap/expect/panic 的服务代码为目标，要求调用方把 Forge 错误映射到产品错误边界。
---

## 文档范围

AsterForge 是 Aster 项目的共享 crate 仓库。这里的文档面向接入方开发者，不是产品用户手册。

当前覆盖的 crate 按字母顺序排列：

- [`aster_forge_actix_middleware`](./crates/aster_forge_actix_middleware.md)
- [`aster_forge_alloc`](./crates/aster_forge_alloc.md)
- [`aster_forge_api`](./crates/aster_forge_api.md)
- [`aster_forge_api_docs_macros`](./crates/aster_forge_api_docs_macros.md)
- [`aster_forge_cache`](./crates/aster_forge_cache.md)
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

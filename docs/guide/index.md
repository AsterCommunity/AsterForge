# Forge 接入总览

AsterForge 是 Aster 产品共用的运行时地基和基础设施内核。它不再只是把多个项目里的重复工具函数抽出来，而是把生命周期、component 注册、健康检查、关闭顺序、配置同步、缓存、数据库基础设施表、后台任务、mail outbox、audit log、日志、指标、panic hook、API helper 和中间件这些产品无关机制沉淀成可复用、可测试、边界清楚的 Rust crates。

它的目标形态更接近 Aster 产品公共 framework foundation：新产品应该把 `main.rs` 写成“创建资源、注册 component、运行 runtime”的薄入口。但 Forge 不接管产品业务层，用户、团队、权限、产品实体、业务状态机、API 语义、审计 action/detail 展示和任务 payload/result 仍然留在产品仓库。

接入时先判断代码属于哪一层：

- **产品语义**：用户、团队、权限、业务状态机、数据库实体、配置存储方式，留在产品仓库。
- **共享机制**：runtime component、health/startup/shutdown、分页、缓存、数据库连接、基础设施 schema/store、指标接口、任务租约、mail outbox、audit log 查询、重试分类、路径和 URL 规范化，放在 Forge。
- **适配边界**：把 Forge 的通用错误、trait、配置结构映射到产品错误、产品指标和产品 repository。

## 推荐接入顺序

新项目优先按最终运行时形态设计，不要从零散工具函数开始凭感觉接：

1. 先定 `main.rs`：使用 `AsterRuntime::builder().component(...).run().await?`，入口只负责资源初始化和 component 注册。
2. 再定基础设施 component：logging、metrics、panic hook、database、cache、config、task、mail、audit、HTTP server。
3. 然后定数据库边界：产品 migration 调用 Forge schema/index builder，产品实体和历史 migration ownership 留在产品仓库。
4. 最后替换零散 helper：validation、API helper、URL/path/id/number 工具、storage key、external auth connector 等。

旧项目接入时可以分批推进，但每一批都要先问清楚：这是单个函数复用，还是应该把 component、schema builder、store、runner、registry、query 机械层一起收进 Forge。先看[新项目接入指南](./new-project-integration.md)，把 `main.rs`、runtime component、migration、audit、mail、task 和 config/cache 的目标形态定住，再逐个 crate 落地。

## 文档结构

每个 crate 页面都按同一套结构写：

- **用途边界**：这个 crate 解决什么，不解决什么。
- **功能开关**：Cargo feature 应该如何启用。
- **最小接入**：产品仓库里通常应该怎么调用。
- **错误边界**：Forge 错误应该在哪里映射成产品错误。
- **测试要求**：接入时必须覆盖哪些行为。
- **参考项目**：Drive/Yggdrasil 里可以对照的代码路径。

## 当前设计取舍

Forge 默认不做下面这些事：

- 不持有产品数据库实体。
- 不定义产品级权限模型。
- 不重新导出产品仓库的类型。
- 不替产品仓库决定用户可见错误文案。
- 不隐藏需要调用方显式处理的外部系统失败。

如果一个功能看起来“可以抽”，但抽出来以后会让产品仓库无法表达自己的业务边界，那就先别抽。反过来，如果它已经是多个产品共享的 runtime/component/schema/store/runner/query 机械层，就不要只抽一个小函数糊弄过去，要把公共内核收完整。

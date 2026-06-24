# AGENTS.md

本文件给在 AsterForge 仓库工作的 agent 使用。先看清楚项目边界再动手，别一上来就把业务逻辑塞进共享 crate，后面拆起来会很烦。

## 项目定位

AsterForge 是 Aster 项目的共享 Rust crate workspace，用来沉淀跨项目复用的基础设施机制。

它不是应用框架，也不是 AsterDrive / AsterYggdrasil 的业务层搬运仓库。

Forge 应该承载：

- API helper、分页、cursor、排序等产品无关结构。
- 缓存、数据库连接、事务、重试、指标、日志、panic hook 等基础设施机制。
- 后台任务 lease、heartbeat、dispatch、runtime、step 状态等通用机械流程。
- 配置 registry、结构化值转换、runtime snapshot、reload notification 等运行时配置内核。
- validation、crypto、storage key、S3 endpoint normalization 等可复用工具。

Forge 不应该承载：

- 产品数据库实体、SeaORM migration、repository SQL。
- 用户、团队、权限、审计、业务状态机。
- 产品 API 错误文案、状态码、本地化、前端展示策略。
- AsterDrive 或 AsterYggdrasil 的具体任务 kind、payload/result、存储策略、外部认证账号绑定规则。
- “为了看起来统一”而没有语义价值的薄封装。

## 开始工作前必须阅读

任何实现前都要先读文档，再读代码，再判断下游替换点。顺序不要反，猫猫别偷懒。

1. 先读入口文档：
   - `README.md`
   - `docs/guide/index.md`
   - `docs/guide/integration-principles.md`
   - `docs/guide/reference-projects.md`

2. 再读相关 crate 文档：
   - `docs/crates/aster_forge_api.md`
   - `docs/crates/aster_forge_config.md`
   - `docs/crates/aster_forge_tasks.md`
   - 或本次任务实际涉及的 `docs/crates/*.md`

3. 然后读对应 crate 代码：
   - `crates/<crate>/Cargo.toml`
   - `crates/<crate>/src/lib.rs`
   - `crates/<crate>/src/**/*.rs`
   - `crates/<crate>/tests/**/*.rs`

4. 最后才去看参考项目或下游接入点：
   - AsterYggdrasil：优先看边界清晰的轻量接入。
   - AsterDrive：适合看功能完整但更复杂的接入。

参考项目只能用来确认接入方式，不能作为把业务逻辑抽进 Forge 的理由。

## 接入与替换工作流

当任务是“接入 Forge”“替换现有函数”“抽公共模块”时，必须先整理替换关系，再改代码。

替换前至少确认：

- 现有函数解决的是共享机制，还是产品语义。
- Forge 中是否已有等价 API。
- Forge API 的错误类型应该在哪一层映射成产品错误。
- 产品侧是否需要保留 adapter、trait impl、metrics、audit、permission check。
- 测试应该覆盖哪些旧行为，避免替换后语义变了。

建议在动手前列出这四列：

```text
旧函数/旧模块 -> Forge API -> 产品侧保留职责 -> 必测行为
```

如果旧函数只是无意义薄封装，例如只调用 Forge API、不映射错误、不注入配置、不记录指标、不表达产品语义，就删掉薄封装，直接使用 Forge API。

如果旧函数承担产品边界职责，例如错误映射、配置注入、审计、指标、权限判断，就保留产品侧 adapter，不要把这些职责移进 Forge。

## 代码边界规则

- 共享机制放 Forge，产品语义留产品仓库。
- Forge 错误类型只表达基础设施或机制失败；产品 API 层自己决定状态码、文案和错误 envelope。
- trait 适配显式写在产品侧，不要靠隐藏全局状态或产品 crate 反向依赖 Forge 内部实现。
- 不要为了减少几行代码引入全局 singleton、隐式 registry 或产品不可测试的静态状态。
- 不要让 `aster_forge_api` 依赖 Actix、Axum 或具体产品实体。
- 不要让 `aster_forge_db` 持有产品 migration、entity 或 repository query。
- 不要让 `aster_forge_tasks` 定义产品 task kind、payload/result、管理 API 或具体任务实现。
- 不要让 `aster_forge_config` 定义产品配置 key、category、i18n 文案、管理 API 或业务 normalizer。
- 不要把 Drive/Yggdrasil 的业务枚举、权限规则、审计字段复制到 Forge。

## Crate 使用导向

优先按风险从低到高接入：

- 小工具：`aster_forge_validation`、`aster_forge_utils`、`aster_forge_api`、`aster_forge_crypto`、`aster_forge_file_classification`、`aster_forge_storage_core`。
- 生命周期基础设施：`aster_forge_logging`、`aster_forge_metrics`、`aster_forge_panic`、`aster_forge_alloc`。
- 高影响运行时模块：`aster_forge_cache`、`aster_forge_db`、`aster_forge_config`、`aster_forge_external_auth`、`aster_forge_tasks`。

高影响模块接入时必须更谨慎，因为它们会影响启动、关闭、错误处理、并发、测试隔离和数据一致性。

## 文档同步规则

新增或改变 public API 时，通常也要同步文档。每个 crate 文档应保持这套结构：

- 用途边界。
- 适用场景。
- Cargo feature / 接入方式。
- 最小接入示例。
- 错误边界。
- 测试要求。
- 参考项目。

如果代码行为和文档冲突，先判断是代码错还是文档过期。不要只改一边。

## Rust 代码规范

- Workspace 使用 Rust 1.94+、edition 2024。
- 依赖尽量放到 root `Cargo.toml` 的 `[workspace.dependencies]`，crate 内按需引用 workspace 依赖。
- 新 crate 命名使用 `aster_forge_*`。
- public API 要有清晰边界，避免暴露内部实现细节。
- 默认不使用 `unwrap`、`expect`、`panic`、`todo`、`unimplemented`。多数 crate 已在非 test 编译下 deny 这些 clippy lint。
- unsafe 只能在确实必要时使用，并写清楚 `SAFETY:` 说明；`aster_forge_alloc` 对 unsafe 要求更严格。
- 错误类型优先用 `thiserror`，并提供产品侧可映射的分类，不要把产品文案写死在 Forge。
- 测试代码可以更直接，但不能掩盖真实边界问题。

## 测试与验证

常用命令：

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all
```

根据改动范围选择验证：

- 只改文档：检查链接、路径和 crate 名称是否正确。
- 只改纯函数 helper：跑相关 crate 测试，必要时跑 workspace check。
- 改 public API 或 feature：跑相关 crate 的默认 feature 和目标 feature 编译。
- 改任务、配置、缓存、数据库、外部认证：至少跑相关 crate 测试和 `cargo check --workspace`，风险高时跑 `cargo test --workspace`。

接入高影响模块时，测试至少覆盖：

- 成功路径。
- 失败路径。
- 错误映射边界。
- 重试、降级或 cancellation。
- 并发、lease、token fence 或 shutdown 行为。

## Code Review Fixes

如果用户粘贴 Greptile、CodeRabbit、Gemini 等 review comments：

1. 逐条判断是真问题还是误报。
2. 先修真实问题，不要为了满足 bot 修改正确代码。
3. 每一批修复后做编译或测试验证。
4. 最终回复里说明哪些评论已修、哪些是误报以及验证命令。

## 工作方式

- 改代码前先读现有模式；模式不清楚就继续查，不要猜。
- 搜索优先用 `rg` / `rg --files`。
- 保持改动聚焦，不做顺手重构。
- 不要回滚用户已有改动。
- 不要改 `target/`、`docs/node_modules/` 或其他生成/依赖目录。
- 除非明确要求，不主动写长篇使用说明；但 public API 变化要维护 crate 文档。

如果一个功能看起来“可以抽”，但抽出来会让产品仓库失去自己的业务边界，那就先别抽。

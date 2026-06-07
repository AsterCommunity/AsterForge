# AsterForge 开发者指南

AsterForge 是地基仓库。先把它当共享运行时代码，再把它当产品 starter。通用能力可以进这里；具体产品工作流应该留在需要它的产品仓库。

## 核心原则

- 地基模块保持业务中立。
- 优先沿用现有 service、repository、DTO、OpenAPI pattern，不要随手发明一套本地框架。
- 不要新增普通用户任务 API，除非具体产品已经有真实的任务可见性模型。
- 不要新增 `api::subcode` 或第二套客户端可见 subcode。需要新公开错误原因时扩展 `AsterErrorCode`。
- 管理员操作、安全敏感变更和运行时生命周期事件要接 audit。
- API contract 改动必须同步 OpenAPI schema 和前端生成类型。
- 项目上线后 migration 只追加，不改已经应用过的迁移。

## 后端扩展路径

新增后端能力时按这个形状走：

```text
src/entities/                  SeaORM model
migration/                     schema migration
src/db/repository/             数据库访问
src/services/                  业务行为
src/api/dto/                   request/response DTO
src/api/routes/                HTTP handler 和路由注册
src/api/openapi.rs             OpenAPI path 和 schema
tests/                         集成测试
frontend-panel/src/services/   前端 service wrapper
frontend-panel/src/pages/      需要时增加管理页面
```

不要把查询逻辑塞进 handler。Handler 应该只做 HTTP 输入处理、调用 service、返回统一 response envelope。

## Runtime Startup

启动拆在 `src/runtime/startup/`：

- `common.rs` 准备运行时目录、metrics、数据库句柄、migration、runtime config、cache、audit manager。
- `primary.rs` 构建 primary runtime state。
- `follower.rs` 构建 follower runtime state。
- `mod.rs` 按 `config.server.start_mode` 分发，并记录 `server_start`。

`server.start_mode = "primary"` 会跑 dispatcher 和 maintenance loops。`server.start_mode = "follower"` 保留公共服务状态，但跳过 primary-only 的后台任务。

## 优雅退出

关闭流程由 `src/main.rs` 和 `src/runtime/shutdown.rs` 协调：

1. 等待 SIGINT/SIGTERM。
2. 取消共享 shutdown token。
3. 优雅停止 Actix。
4. 记录 `server_shutdown`。
5. 在宽限期内停止后台任务。
6. flush audit logs。
7. 关闭数据库句柄。

新增长跑 worker 时，必须监听 shutdown token，并保证持久化状态可恢复。

## 后台任务

任务系统在 `src/services/task_service/` 和 `src/runtime/tasks.rs`。

当前模板 task kind 只有 `system_runtime`。Admin API 可以 list、retry、cleanup；普通用户任务 API 是故意没有的。

关键契约：

- Claim 带 `processing_token` fence。
- Worker 通过 heartbeat 续租。
- 过期 worker 不能覆盖新 lease。
- 优雅退出会把 processing task 放回 `retry`，不消耗 retry budget，也不写业务失败细节。
- Task presentation 使用稳定 message code，前端不解析 task payload 或 result blob。
- 修改 dispatch 语义时，要补 claim、retry、cleanup、shutdown 行为测试。

如果产品要加业务 task kind，payload/result 类型、registry、retry 分类、初始 steps、presentation、可见性规则要一起设计。

邮件 outbox 投递也是系统运行时任务，具体扩展规则见 [邮件运行时扩展](./mail-runtime.md)。

## Audit Service

Audit 代码在 `src/services/audit_service/`。

这些场景要写 audit：

- server start 和 shutdown
- login 以及安全相关 auth 变更
- admin config 变更
- admin external auth provider 变更
- admin task retry 和 cleanup
- mail send 和 mail delivery failure
- 产品自己的管理员状态变更

Audit entry 应包含结构化 details 和 presentation metadata。前端优先展示 `presentation`，raw `details` 只作为 fallback/debug 信息。

邮件 audit 的 details、presentation 和测试要求见 [邮件运行时扩展](./mail-runtime.md)。

## API 与错误

所有 API 响应使用 `src/api/response.rs` 里的统一 envelope。

客户端可见失败应该暴露稳定的 `AsterErrorCode`。旧的 `E001` 这类内部码只用于服务端诊断。地基模块里不要新增第二套公开 subcode。

API contract 改动流程：

1. 更新 DTO 和路由注解。
2. 在 `src/api/openapi.rs` 注册 path 和 schema。
3. 生成 OpenAPI。
4. 重新生成前端 API 类型。
5. 更新前端 service/page。

命令：

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## 前端扩展路径

前端在 `frontend-panel/`。

约定：

- `src/services/` 放 API wrapper。
- `src/types/api.ts` 统一 re-export 生成类型。
- `src/lib/presentation.ts` 放稳定 audit/task 展示格式化。
- `src/pages/admin/` 放管理员页面。
- 先用已有 table/form/page shell 组件，再考虑新增 UI primitive。

Admin screen 应该保持高信息密度、可扫描、可预测。不要在管理面板里做 marketing-style 布局。

## 测试

常用命令：

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo clippy --tests -- -D warnings
cargo test

cd frontend-panel
bun run check
bun run test
bun run build
```

常用目标测试：

```bash
cargo test --test test_admin_tasks
cargo test --test test_audit
cargo test --test test_audit mail_outbox_dispatch_records_delivery_audit_logs
cargo test mail_template
cargo test task_service::presentation
cargo test shutdown_release_returns_processing_task_to_retry_without_failure_update
```

## 使用 cargo-generate

根目录的 `cargo-generate.toml` 让这个仓库可以作为 `cargo generate` 来源，同时过滤构建和运行产物：

```bash
cargo install cargo-generate
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

生成后的项目仍然会包含 `aster_forge` 和 `AsterForge` 这类标识，直到执行初始化脚本。`./init.sh --help` 可以查看非交互参数。

因为初始化会替换 lockfile 里的 package name，执行后建议用 `cargo generate-lockfile` 和 `cd frontend-panel && bun install` 重新生成 lockfile。

建议后续检查清单：

- `Cargo.toml`：package name、description、repository，需要时改 binary 命名。
- `Dockerfile`：binary path、image labels、默认数据库路径。
- `docker-compose.yml`：image name、service name、默认数据库路径。
- `README.md` 和 `README.zh.md`：项目名、描述、链接。
- `frontend-panel/package.json`：package name。
- `frontend-panel/src/config/app.ts`：前端展示默认值。
- `config.example.toml`：默认数据库路径和示例。
- `src/api/openapi.rs`：API title 和 description。
- `src/main.rs`：如果产品不应该再输出 AsterForge，改 startup log label。

重命名后先跑后端和前端校验，再开始加业务代码。

## 边界检查清单

往 AsterForge 加模块前先问：

- 这个能力是否对大多数未来 Aster 服务都有用？
- 它能不能在没有产品假设的情况下配置？
- 它有没有避免把产品业务概念塞进共享地基？
- 它的测试是否覆盖了对应的 service/repository/API 风险？
- 前端拿到的是稳定 presentation，而不是被迫解析后端内部结构？

如果答案是否定的，就放进产品仓库。

如果需要产品级扩展示例，可以在理解这层地基之后再看 AsterDrive。看它是为了学习扩展模式，不是为了决定什么模块应该进 AsterForge。

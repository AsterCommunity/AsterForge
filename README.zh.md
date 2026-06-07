# AsterForge

AsterForge 是 Aster 系列项目复用的 Rust + React 服务地基。它不是一个具体产品，而是开新服务前应该复制的底座：HTTP 服务、认证、运行时配置、邮件投递、审计日志、后台任务、管理员 API、OpenAPI 生成、内嵌前端资源和部署默认值。

这个仓库聚焦跨服务通用的运行时能力。具体业务域应该由下游项目基于这层地基继续添加。

- English README: [README.md](README.md)
- 公开文档: [docs/index.md](docs/index.md)
- 开发者文档: [developer-docs/README.md](developer-docs/README.md)
- 配置示例: [config.example.toml](config.example.toml)
- 前端面板: [frontend-panel/](frontend-panel/)

## 包含什么

### 后端地基

- Actix Web 服务，支持内嵌前端资源。
- SeaORM entities、repositories、migrations、数据库重试、事务和读写句柄。
- 稳定 API 响应 envelope，以及公开的 `AsterErrorCode` 错误码。
- 本地认证：首个管理员初始化、注册、登录、刷新、登出、当前用户和会话管理。
- 外部认证 provider 脚手架，用于 OIDC/OAuth2 一类登录流程。
- 管理员 API：运行时配置、审计日志、外部认证 provider、后台任务。
- `system_config` 运行时配置，和静态 `config.toml` 分离。
- 邮件投递：SMTP 运行时配置、模板变量、持久化 outbox、测试邮件和邮件审计。
- memory/noop/Redis cache backend，统一挂在 cache trait 后面。
- request id、安全响应头、运行时 CORS、CSRF helper、请求 metrics、IP 限流 middleware。
- 健康检查、readiness，以及可选 Prometheus metrics。

### 运行时地基

- 通过 `server.start_mode` 区分 primary/follower 启动模式。
- HTTP、后台任务、审计 flush、数据库连接的优雅退出。
- 异步缓冲审计写入，并提供稳定的 audit presentation 给前端展示。
- 后台任务记录、dispatch、lease/heartbeat、重试分类、清理和稳定 task presentation。
- primary-only 周期任务：
  - background task dispatcher
  - system health check
  - auth session cleanup
  - external auth flow cleanup
  - mail outbox dispatch
  - audit log cleanup
  - task artifact cleanup

Follower 模式会保留公共运行时初始化，但跳过 primary-only 的 dispatch、邮件 outbox 投递和 cleanup loop。

### 前端地基

- `frontend-panel/` 下的 React + Vite + TypeScript 管理面板。
- 基于 OpenAPI 生成的类型化 service 层。
- 管理员页面：配置、审计日志、外部认证 provider、后台任务。邮件 SMTP、模板和测试邮件属于运行时配置入口。
- audit/task presentation 格式化，前端不需要解析 raw details 或 task payload。
- Vitest + jsdom 单测、Biome 检查、Vite 生产构建。

## 明确不包含什么

AsterForge 把具体业务模块留给下游服务：

- 文件存储
- 上传流程
- 团队或分享
- 回收站或压缩包
- 缩略图或媒体处理
- WebDAV 或 WOPI
- 存储策略或远程节点
- 普通用户任务 API

这个模板里的后台任务只面向管理员和系统运行时。不要假设普通用户可以看到任务记录。如果具体产品需要用户任务 API，请在该产品仓库里单独设计可见性模型。

AsterForge 也不引入第二套公开 API subcode。客户端可见错误应该使用明确命名的 `AsterErrorCode`。

## 仓库结构

```text
src/                         Rust 后端
src/api/                     路由、DTO、OpenAPI 注册、middleware、响应 envelope
src/cache/                   Cache trait 和 memory/noop/Redis 实现
src/config/                  静态配置、运行时配置定义、normalizer
src/db/                      数据库连接、重试、事务、repository
src/entities/                SeaORM entity model
src/metrics/                 metrics feature 下的 Prometheus 实现
src/runtime/                 App state、启动、关闭、日志、后台任务 loop
src/services/                auth、external auth、config、mail、audit、task、health、examples
src/types/                   共享 domain enum 和 DB wrapper type
src/utils/                   crypto、ID、path、number、email、RAII helper
migration/                   SeaORM migration crate
api-docs-macros/             OpenAPI helper macro crate
frontend-panel/              React 管理面板
developer-docs/              开发者文档和扩展说明
tests/                       集成测试和 OpenAPI 导出测试
```

## 从源码快速启动

需要：

- [rust-toolchain.toml](rust-toolchain.toml) 指定的 Rust toolchain
- Bun
- 默认使用 SQLite

先构建前端资源，再启动服务：

```bash
cd frontend-panel
bun install
bun run build
cd ..

cargo run
```

首次启动时，AsterForge 会：

- 如果缺失则创建 `data/config.toml`
- 把默认相对路径解析到 `data/` 下
- 创建默认 SQLite 数据库
- 执行 migrations
- 把默认运行时配置写入 `system_config`
- 在 `127.0.0.1:3000` 启动 HTTP 服务

打开：

```text
http://127.0.0.1:3000
```

创建首个管理员：

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

如果只是本地 HTTP cookie 测试，可以在首次启动前这样跑：

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

真实部署请放到 HTTPS 后面，并保持 secure cookie 开启。

## Docker

使用 Compose：

```bash
mkdir -p ./data
docker compose up -d
```

本地构建并运行：

```bash
docker build -t asterforge:local .
docker run --rm -p 3000:3000 -v "$(pwd)/data:/data" asterforge:local
```

容器期望 `/data` 是可写运行时目录。

## 生成新项目

AsterForge 可以作为 `cargo-generate` starter repository 使用：

```bash
cargo install cargo-generate
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

模板配置会过滤本地构建和运行产物，比如 `target/`、`data/`、`tmp/`、前端 `node_modules/`、`dist/` 和生成的 OpenAPI 输出。

生成后的项目会暂时保留 `aster_forge` 这类源码标识，直到执行初始化脚本。这样做是为了让 AsterForge 本仓库仍然是一个可以直接编译的普通 Rust 项目，而不是只能被模板引擎处理的源码。`./init.sh --help` 可以查看非交互参数；剩余的产品品牌化细节按 [developer-docs/zh-CN/README.md](developer-docs/zh-CN/README.md) 里的清单处理。

初始化后建议用 `cargo generate-lockfile` 和 `cd frontend-panel && bun install` 重新生成 lockfile。

## 重要接口

```text
GET  /health
GET  /health/ready
GET  /health/metrics                    # 需要 --features metrics

GET  /api/v1/system/info

POST /api/v1/auth/setup
POST /api/v1/auth/register
POST /api/v1/auth/login
POST /api/v1/auth/refresh
POST /api/v1/auth/logout
GET  /api/v1/auth/me
GET  /api/v1/auth/sessions

GET  /api/v1/external-auth/providers
POST /api/v1/external-auth/{provider}/start
GET  /api/v1/external-auth/{provider}/callback

GET  /api/v1/auth/external-auth/providers
GET  /api/v1/auth/external-auth/{kind}/providers
POST /api/v1/auth/external-auth/{kind}/{provider}/start
GET  /api/v1/auth/external-auth/{kind}/{provider}/callback

GET    /api/v1/admin/config
GET    /api/v1/admin/config/schema
GET    /api/v1/admin/config/template-variables
GET    /api/v1/admin/config/{key}
PUT    /api/v1/admin/config/{key}
DELETE /api/v1/admin/config/{key}
POST   /api/v1/admin/config/mail/action

GET  /api/v1/admin/audit-logs

GET  /api/v1/admin/tasks
POST /api/v1/admin/tasks/cleanup
POST /api/v1/admin/tasks/{id}/retry

GET    /api/v1/admin/external-auth/provider-kinds
GET    /api/v1/admin/external-auth/providers
POST   /api/v1/admin/external-auth/providers
GET    /api/v1/admin/external-auth/providers/{id}
PATCH  /api/v1/admin/external-auth/providers/{id}
DELETE /api/v1/admin/external-auth/providers/{id}
POST   /api/v1/admin/external-auth/providers/test
POST   /api/v1/admin/external-auth/providers/{id}/test
```

debug 构建加 `openapi` feature 时，可以导出静态 OpenAPI：

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## 配置模型

静态配置默认从 `data/config.toml` 读取，可以用 `ASTER__...` 环境变量覆盖：

```bash
ASTER__SERVER__HOST=0.0.0.0
ASTER__SERVER__PORT=3000
ASTER__SERVER__START_MODE=primary
ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc'
ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret'
```

完整静态配置见 [config.example.toml](config.example.toml)。

运行时配置存在 `system_config`，通过 Admin Config API/UI 修改。需要不改 `config.toml` 就能热调整的值放运行时配置；数据库 URL、监听地址、密钥这类启动关键值放静态配置。

邮件投递也是运行时配置：SMTP 主机、端口、加密、用户名密码、发件人、邮件模板和 `mail_outbox_dispatch_interval_secs` 都通过 Admin Config API/UI 管理。管理员测试邮件走 `POST /api/v1/admin/config/mail/action`，outbox 由 primary 节点的 `mail-outbox-dispatch` 周期任务投递。详细说明见 [docs/guide/mail.md](docs/guide/mail.md)。

## 开发命令

后端：

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo clippy --tests -- -D warnings
cargo test
cargo run
```

前端：

```bash
cd frontend-panel
bun install
bun run check
bun run test
bun run build
bun run dev
```

OpenAPI 和前端生成类型：

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

## Feature

```text
server               默认服务构建
cli                  预留 CLI feature
metrics              Prometheus metrics 和 system metrics
openapi              OpenAPI schema 和 debug API docs 支持
full                 server + cli + metrics + openapi
jemalloc             使用 jemalloc allocator
jemalloc-stats       jemalloc stats 支持
jemalloc-profiling   jemalloc profiling 支持
```

## 模板扩展规则

从 AsterForge 开产品时：

1. 业务 entity 放进 `src/entities/`，schema 变更加到 `migration/`。
2. DB 访问放进 `src/db/repository/`。
3. 业务行为放进 `src/services/`。
4. HTTP contract 放进 `src/api/dto/`，路由放到 `src/api/routes/`。
5. 新路径和 schema 要注册到 `src/api/openapi.rs`。
6. 管理员操作和安全相关状态变更要接 audit。
7. 新 task kind 必须先明确 owner、retry 模型和可见性模型。
8. 新邮件模板、邮件 payload 或 outbox 语义要同步 runtime config、OpenAPI、audit presentation 和前端生成类型。
9. 地基模块保持通用，产品工作流留在产品仓库。

## 参考实现

如果想看更完整的产品扩展示例，可以参考 AsterDrive。它适合用来理解 module boundary、task integration、audit 覆盖和运维界面怎么落地，但不要把它当成应该复制进模板的模块清单。

## License

MIT。见 [LICENSE](LICENSE)。

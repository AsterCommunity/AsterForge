# 快速开始

这一页用于从零启动 AsterForge，并确认后端、前端资源、数据库迁移和首个管理员初始化都能正常工作。

## 环境要求

需要准备：

- Rust toolchain，版本以仓库根目录的 `rust-toolchain.toml` 为准。
- Bun，用于安装前端依赖、构建管理面板和运行 docs。
- SQLite。默认配置会使用 SQLite，本地开发不需要额外数据库服务。

可选准备：

- Docker 和 Docker Compose，用于验证容器部署。
- `cargo-generate`，用于从这个仓库生成新项目。

## 构建前端资源

后端会内嵌 `frontend-panel/dist` 下的静态资源。第一次运行前先构建前端：

```bash
cd frontend-panel
bun install
bun run build
cd ..
```

如果你只改后端代码，后续不一定每次都要重新构建前端。改了前端页面、API 类型或静态资源后再执行一次即可。

## 启动服务

从仓库根目录运行：

```bash
cargo run
```

首次启动会完成这些动作：

- 如果 `data/config.toml` 不存在，会根据默认值创建。
- 相对路径会按 `data/` 运行时目录解析。
- 默认 SQLite 数据库会被创建。
- SeaORM migrations 会被执行。
- 默认运行时配置会写入 `system_config`。
- HTTP 服务会监听 `127.0.0.1:3000`。

打开：

```text
http://127.0.0.1:3000
```

健康检查：

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/health/ready
```

## 创建首个管理员

首次部署需要创建第一个管理员账号：

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

创建完成后，后续用户注册和管理员操作都应按项目自己的策略控制。

## 本地 HTTP Cookie

真实部署应该放在 HTTPS 后面，并保持 secure cookie 开启。本地只用 HTTP 调试时，可以临时启用 insecure bootstrap cookie：

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

这个开关只应该用于本地开发。生产环境不要把它设为 `true`。

## 常用检查命令

后端：

```bash
cargo fmt
cargo check --bins
cargo check --features openapi
cargo test
```

前端：

```bash
cd frontend-panel
bun run check
bun run build
```

文档：

```bash
cd docs
bun install
bun run docs:dev
bun run docs:build
```

## 导出 OpenAPI

debug 构建开启 `openapi` feature 时，可以生成静态 OpenAPI 文档：

```bash
cargo test --features openapi generate_openapi
```

然后刷新前端生成的 service 层：

```bash
cd frontend-panel
bun run generate-api
```

新增 API 时，后端 route 需要注册 OpenAPI 注解，前端再基于导出的 schema 生成类型。这样 Admin UI 不需要手写不可靠的接口类型。

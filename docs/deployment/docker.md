# Docker 部署

AsterForge 提供 Dockerfile、Compose 配置和 GitHub Actions 镜像发布工作流。默认运行状态保存在 `/data`，容器镜像本身不应该保存运行时数据。

## Docker Compose

本地最小启动：

```bash
mkdir -p ./data
docker compose up -d
```

查看日志：

```bash
docker compose logs -f
```

停止：

```bash
docker compose down
```

`./data` 会挂载到容器内 `/data`。数据库、配置文件和运行时状态都应该保存在这个目录。

## 本地构建

```bash
docker build -t asterforge:local .
docker run --rm -p 3000:3000 -v "$(pwd)/data:/data" asterforge:local
```

如果要覆盖配置：

```bash
docker run --rm \
  -p 3000:3000 \
  -v "$(pwd)/data:/data" \
  -e ASTER__SERVER__HOST=0.0.0.0 \
  -e ASTER__SERVER__PORT=3000 \
  -e ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc' \
  -e ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret' \
  asterforge:local
```

生产环境不要使用示例 secret。JWT 和 MFA secret 应该来自 secret manager 或受保护的部署变量。

## 反向代理

生产部署建议放到 HTTPS 反向代理后面。代理至少要处理：

- TLS termination。
- `X-Forwarded-For`、`X-Forwarded-Proto` 等转发头。
- 请求体大小限制。
- 超时。
- 访问日志。

AsterForge 侧要配置 trusted proxies，避免错误信任客户端伪造的转发头。

## 健康检查

容器编排系统可以使用：

```text
GET /health
GET /health/ready
```

`/health` 表示进程存活，`/health/ready` 表示服务是否准备好接收请求。数据库不可用、迁移失败或 runtime 未就绪时，readiness 不应通过。

## 镜像发布

`.github/workflows/docker-image.yml` 默认发布到 GHCR：

```text
ghcr.io/astercommunity/asterforge
```

tag push 会构建 amd64 和 arm64 镜像，并创建 multi-arch manifest。默认不会发布 Docker Hub。

如果确实需要同时发布 Docker Hub，手动运行 workflow，并勾选 `publish_dockerhub`。同时需要配置：

```text
DOCKERHUB_USERNAME
DOCKERHUB_TOKEN
```

这个 opt-in 设计是为了避免模板项目在没有明确配置时把镜像推到错误的 Docker Hub namespace。

## Primary 和 Follower

单实例部署使用默认值：

```bash
ASTER__SERVER__START_MODE=primary
```

多节点部署时，只应该让需要执行全局维护任务的节点使用 primary。Follower 节点可以接收 HTTP 请求，但会跳过 dispatcher 和 cleanup loop。

## 数据备份

默认 SQLite 部署至少备份：

```text
data/config.toml
data/asterforge.db
data/asterforge.db-shm
data/asterforge.db-wal
```

如果使用 PostgreSQL 或 MySQL，按数据库自身策略备份，同时仍要保留 `data/config.toml` 和其他运行时文件。

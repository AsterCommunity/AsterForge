# 配置模型

AsterForge 把配置分成两层：静态配置和运行时配置。这个拆分很重要，因为不是所有配置都适合在线修改。

## 静态配置

静态配置默认读取 `data/config.toml`。如果文件不存在，服务首次启动会创建默认配置。

示例：

```toml
[server]
host = "127.0.0.1"
port = 3000
workers = 0
temp_dir = ".tmp"
start_mode = "primary"

[database]
url = "sqlite://asterforge.db?mode=rwc"
pool_size = 10
retry_count = 3

[auth]
jwt_secret = "replace-with-a-long-random-secret"
mfa_secret_key = "replace-with-another-long-random-secret"
bootstrap_insecure_cookies = false
```

完整示例见仓库根目录的 `config.example.toml`。

静态配置适合放这些内容：

- HTTP 监听地址和端口。
- 数据库连接 URL、连接池大小和重试次数。
- JWT、MFA 等启动期密钥。
- 节点启动模式。
- cache backend 和 Redis 地址。
- 日志输出格式和文件路径。
- rate limit 和 trusted proxy 初始设置。

## 环境变量覆盖

静态配置可以用 `ASTER__...` 环境变量覆盖。层级用双下划线连接：

```bash
ASTER__SERVER__HOST=0.0.0.0
ASTER__SERVER__PORT=3000
ASTER__SERVER__START_MODE=primary
ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc'
ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret'
```

容器部署时，建议把密钥、数据库 URL 和监听地址放在环境变量或 secret 管理系统里，不要写死在镜像内。

## 路径解析

默认运行时目录是 `data/`。当配置文件位于 `data/config.toml` 时，相对路径会按 `data/` 解析。

例如：

```toml
[database]
url = "sqlite://asterforge.db?mode=rwc"

[server]
temp_dir = ".tmp"
```

会落在类似：

```text
data/asterforge.db
data/.tmp
```

这种策略让本地开发和容器挂载都比较直接。生产部署时，保证 `data/` 或容器内 `/data` 可写。

## 运行时配置

运行时配置存储在数据库的 `system_config` 表中，通过 Admin Config API 或管理面板修改。

运行时配置适合放这些内容：

- 站点展示名称、公开 URL 等 branding/site 设置。
- CORS、注册策略、本地邮箱策略等可以在线调整的策略。
- SMTP、邮件发件人、邮件模板和 outbox 投递间隔。
- 审计保留天数和维护任务参数。
- 不需要重启进程即可生效的业务开关。

不适合放运行时配置的内容：

- 数据库 URL。
- 监听地址和端口。
- JWT secret、MFA secret 这类安全密钥。
- 决定启动拓扑的节点模式。

这些值变更后通常需要重启服务，或者必须在服务启动前就确定。

## Admin Config API

常用端点：

```text
GET    /api/v1/admin/config
GET    /api/v1/admin/config/schema
GET    /api/v1/admin/config/{key}
PUT    /api/v1/admin/config/{key}
DELETE /api/v1/admin/config/{key}
```

`/schema` 会返回运行时配置 schema，前端可以据此渲染表单、校验输入和展示说明。

新增运行时配置项时，应同时补：

- 配置定义和默认值。
- schema 信息。
- normalizer 或 validator。
- Admin UI 展示。
- 相关操作的 audit log。

## 邮件配置

邮件投递配置也是运行时配置，不放进 `config.toml`。常用 key 包括：

```text
mail_smtp_host
mail_smtp_port
mail_security
mail_smtp_username
mail_smtp_password
mail_from_address
mail_from_name
mail_outbox_dispatch_interval_secs
```

邮件模板使用 `mail_template_*_subject` 和 `mail_template_*_html` 这类 key。可用模板变量可以从这个端点读取：

```text
GET /api/v1/admin/config/template-variables
```

管理员可以通过 config action 发送测试邮件：

```text
POST /api/v1/admin/config/mail/action
```

请求体：

```json
{
  "action": "send_test_email",
  "target_email": "ops@example.com"
}
```

详细说明见 [邮件投递](./mail.md)。

## 配置变更审计

管理员修改运行时配置时应写审计。审计 details 里保留 key、旧值摘要、新值摘要和变更来源，presentation 层负责给前端稳定显示文案。

不要让前端直接拼 raw details 字符串。配置项一多，前端硬解析会很快变成维护负担。

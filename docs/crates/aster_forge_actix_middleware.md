# aster_forge_actix_middleware

`aster_forge_actix_middleware` 收纳 Actix Web 绑定的共享中间件。它的边界很窄：只放与 HTTP 框架相关、但不依赖产品业务实体的 middleware。

## 适用场景

- 给每个请求生成或传递 request id。
- 注入基础安全响应头。
- 让 Drive、Yggdrasil 等 Actix 服务复用同一套 HTTP 基础行为。

不适合放在这里的内容：

- 认证、权限、管理员校验。
- 产品审计上下文。
- 依赖产品配置表或用户实体的 middleware。

## Cargo 接入

```toml
[dependencies]
aster_forge_actix_middleware = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## Request ID

模块：`aster_forge_actix_middleware::request_id`

主要类型：

- `RequestIdMiddleware`
- `RequestId`

接入方式：

```rust
use aster_forge_actix_middleware::request_id::RequestIdMiddleware;

app.wrap(RequestIdMiddleware)
```

中间件会优先使用已有请求头里的 request id；缺失时生成 UUID。handler 可以从 request extensions 中读取 `RequestId`，用于日志字段、错误响应或审计链路。

接入注意点：

- 产品侧决定是否把 request id 暴露给前端。
- 产品侧决定日志字段名，例如 `request_id`、`trace_id`。
- 不要在业务 service 里重新生成 request id，否则请求链路会断。

## Security headers

模块：`aster_forge_actix_middleware::security_headers`

主要 API：

- `default_headers()`
- `X_FRAME_OPTIONS_VALUE`
- `REFERRER_POLICY_VALUE`
- `X_CONTENT_TYPE_OPTIONS_VALUE`

接入方式：

```rust
use aster_forge_actix_middleware::security_headers::default_headers;

app.wrap(default_headers())
```

默认头用于普通后端管理界面和 API 服务：

- `X-Frame-Options: SAMEORIGIN`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `X-Content-Type-Options: nosniff`

如果产品有 WOPI、iframe preview 或跨站嵌入需求，不要硬改 Forge 默认值。应该在产品侧选择是否使用默认 middleware，或者在产品侧单独实现更具体的 header 策略。

## 测试要求

接入产品仓库后至少覆盖：

- 无 request id 请求会生成 request id。
- 已有 request id 会被保留。
- 默认安全头出现在响应里。
- 特殊路由如果不能使用默认安全头，需要有单独测试说明原因。

## 参考项目

- AsterYggdrasil：Actix app 初始化和 request id 日志链路。
- AsterDrive：WebDAV、WOPI、预览等路由如果需要特殊 header，优先在产品侧覆盖，不反推 Forge 默认值。

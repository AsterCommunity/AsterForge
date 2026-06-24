# aster_forge_actix_middleware

`aster_forge_actix_middleware` 收纳 Actix Web 绑定的共享中间件。它的边界很窄：只放与 HTTP 框架相关、但不依赖产品业务实体的 middleware。

## 适用场景

- 给每个请求生成或传递 request id。
- 注入基础安全响应头。
- 提供 CSRF token 和 request source 校验 helper。
- 记录 Actix HTTP 请求指标。
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

## Metrics

模块：`aster_forge_actix_middleware::metrics`

主要类型：

- `MetricsMiddleware`
- `MetricsService`

接入方式：

```rust
use aster_forge_actix_middleware::metrics::MetricsMiddleware;
use aster_forge_metrics::SharedMetricsRecorder;

app.app_data(web::Data::new(metrics as SharedMetricsRecorder))
    .wrap(MetricsMiddleware)
```

中间件会从 Actix app data 读取 `SharedMetricsRecorder`。没有注册 recorder 时使用 `NoopMetrics`；recorder disabled 时直接跳过记录。启用后会记录 method、route label、status code 和 request duration。

route label 优先使用 Actix matched pattern。未匹配路由会被归入低基数标签：

- `/api/...` -> `unmatched_api`
- `/health...` -> `unmatched_health`
- 其他 -> `unmatched`

产品侧仍然负责真实 recorder、Prometheus exporter、metric namespace 和 label 约束。

## CSRF

模块：`aster_forge_actix_middleware::csrf`

主要 API：

- `build_csrf_token()`
- `CsrfTokenNames::new(cookie_name, header_name)`
- `ensure_double_submit_token(req)`
- `ensure_double_submit_token_with_names(req, names)`
- `ensure_service_double_submit_token(req)`
- `ensure_service_double_submit_token_with_names(req, names)`
- `ensure_request_source_allowed(req, public_site_origins, mode)`
- `ensure_service_request_source_allowed(req, public_site_origins, mode)`
- `ensure_headers_allowed(origin, referer, sec_fetch_site, request_origin, public_site_origins, mode)`
- `is_unsafe_method(method)`

主要类型：

- `CsrfTokenNames`
- `RequestSourceMode`
- `CsrfError`
- `CsrfErrorKind`

Forge 只做产品无关的 CSRF 机制：

- URL-safe 32-byte random token。
- cookie 与 header 的 double-submit 校验。默认兼容名是 `aster_csrf` 和 `X-CSRF-Token`，但产品可以传入 `CsrfTokenNames` 使用自己的 cookie/header 名。
- `Origin` / `Referer` / `Sec-Fetch-Site` 来源校验。
- 请求 source header 长度上限和 origin 规范化。

产品侧负责把 `CsrfErrorKind` 映射到自己的错误码、HTTP response 和审计字段。例如 Drive 可以映射到 `ApiErrorCode::AuthCsrfCookieMissing`，Yggdrasil 可以映射到自己的 `AsterError::auth_csrf_missing()`。

推荐接入方式：

```rust
use std::sync::OnceLock;

use actix_web::{HttpRequest, dev::ServiceRequest};
use aster_forge_actix_middleware::csrf::{
    CsrfError, CsrfTokenNames, RequestSourceMode,
    ensure_double_submit_token_with_names,
    ensure_service_request_source_allowed,
};

static CSRF_NAMES: OnceLock<CsrfTokenNames> = OnceLock::new();

fn init_csrf_names() -> Result<(), CsrfError> {
    let names = CsrfTokenNames::new("aster_yggdrasil_csrf", "X-Aster-Yggdrasil-CSRF")?;
    let _ = CSRF_NAMES.set(names);
    Ok(())
}

fn csrf_names() -> &'static CsrfTokenNames {
    CSRF_NAMES.get_or_init(CsrfTokenNames::default)
}

fn ensure_token(req: &HttpRequest) -> Result<(), CsrfError> {
    ensure_double_submit_token_with_names(req, csrf_names())
}

fn csrf_header_for_cors() -> &'static str {
    csrf_names().header_name_str()
}
```

接入注意点：

- `public_site_origins` 由产品 runtime config 提供。
- CSRF helper 不知道产品登录态；middleware 应该只在 cookie-authenticated unsafe method 上调用。
- `RequestSourceMode::Required` 适合强制要求可信 `Origin`/`Referer` 的写操作。
- `OptionalWhenPresent` 适合兼容旧客户端，但仍会拒绝明确不可信的来源。
- 同一个浏览器 origin 上部署多个 Aster 服务时，不要共享默认 CSRF cookie/header 名；每个产品应该在启动时初始化自己的 `CsrfTokenNames`。
- CSRF token names 不适合运行时热切。改名会让浏览器已有 cookie、前端发送的 header 和后端校验出现短时间不一致，应该通过静态配置或环境变量设置，并在重启后生效。
- 自定义 header 名必须同步加入产品的 CORS preflight allow-list。Forge 提供 `CsrfTokenNames::header_name_str()`，就是为了让产品在构造 `Access-Control-Allow-Headers` 时复用同一份名字。

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
- CSRF token 生成、cookie/header mismatch、来源 header 校验。
- metrics enabled 时成功和错误响应都会记录。
- metrics disabled 或缺失 recorder 时不影响请求。
- 默认安全头出现在响应里。
- 特殊路由如果不能使用默认安全头，需要有单独测试说明原因。

## 参考项目

- AsterYggdrasil：Actix app 初始化和 request id 日志链路。
- AsterDrive：WebDAV、WOPI、预览等路由如果需要特殊 header，优先在产品侧覆盖，不反推 Forge 默认值。

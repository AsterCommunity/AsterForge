# aster_forge_actix_middleware

`aster_forge_actix_middleware` 收纳 Actix Web 绑定的共享中间件。它的边界很窄：只放与 HTTP 框架相关、但不依赖产品业务实体的 middleware。

## 适用场景

- 给每个请求生成或传递 request id。
- 注入基础安全响应头。
- 提供 CSRF token 和 request source 校验 helper。
- 提供 runtime CORS middleware 的 Actix 机械层。
- 提供可信代理真实 IP 提取和通用 keyed rate limiter。
- 可选记录 Actix HTTP 请求指标。
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

默认 feature 不启用 HTTP metrics middleware，适合只需要 CSRF、CORS、rate limit、request id 和 security headers 的产品。

如果产品要使用 `aster_forge_actix_middleware::metrics::MetricsMiddleware`，需要显式开启：

```toml
aster_forge_actix_middleware = { git = "https://github.com/AsterCommunity/AsterForge", features = ["metrics"] }
```

## Rate Limit

模块：`aster_forge_actix_middleware::rate_limit`

主要类型和函数：

- `TrustedProxyIpKeyExtractor`
- `NormalizedStringRateLimiter`
- `RateLimitRejection`
- `build_ip_governor_config(seconds_per_request, burst_size, trusted_proxies)`
- `build_ip_governor_config_with_rejection_response(...)`
- `retry_after_seconds(not_until)`

Forge 负责产品无关的 rate-limit 机械行为：

- 解析可信代理 CIDR / 单 IP 列表。
- 仅当 direct peer 是可信代理时，使用 `X-Forwarded-For` 最左侧地址作为客户端 IP。
- 为 `actix-governor` 提供可复用 IP key extractor。
- 从非零 `(seconds_per_request, burst_size)` 构造 governor quota。
- 允许产品注入自己的 `429` response factory，同时继续复用可信代理和 client IP 提取。
- 提供按字符串 key 限流的 `NormalizedStringRateLimiter`，默认 trim 并 lowercase key。
- 把 governor rejection 转成可复用的 `retry_after_seconds`。

产品侧仍然负责：

- 配置结构、默认值、热更新策略。
- `429 Too Many Requests` 的 response body、错误码和本地化文案。
- 决定哪些路由使用 IP 限流，哪些协议端点使用 username/email/provider id 等业务 key 限流。
- 审计、指标标签和安全事件记录。

典型 Actix API 接入：

```rust
use actix_governor::GovernorConfig;
use actix_web::http::StatusCode;
use aster_forge_actix_middleware::rate_limit::{
    TrustedProxyIpKeyExtractor, build_ip_governor_config_with_rejection_response,
};
use governor::middleware::NoOpMiddleware;
use std::num::{NonZeroU32, NonZeroU64};

fn build_config(
    seconds_per_request: NonZeroU64,
    burst_size: NonZeroU32,
    trusted_proxies: &[String],
) -> GovernorConfig<TrustedProxyIpKeyExtractor, NoOpMiddleware> {
    build_ip_governor_config_with_rejection_response(
        seconds_per_request,
        burst_size,
        trusted_proxies,
        |retry_after, mut response| {
            response
                .status(StatusCode::TOO_MANY_REQUESTS)
                .insert_header(("Retry-After", retry_after.to_string()))
                .json(serde_json::json!({
                    "code": "rate_limited",
                    "retry_after": retry_after,
                }))
        },
    )
}
```

不要为修改 `429` body 再复制一份 `KeyExtractor`。产品只注入 response factory；trusted proxy、
`X-Forwarded-For` 和 governor quota 的机械逻辑继续由 Forge 持有。

典型协议端点接入：

```rust
use aster_forge_actix_middleware::rate_limit::NormalizedStringRateLimiter;
use std::num::{NonZeroU32, NonZeroU64};

let limiter = NormalizedStringRateLimiter::new(
    true,
    NonZeroU64::new(60).unwrap(),
    NonZeroU32::new(1).unwrap(),
);

if let Some(rejection) = limiter.check("User@Example.com") {
    let retry_after = rejection.retry_after_seconds();
    // Product code maps this into its own protocol error body.
}
```

不要把产品 `ApiResponse`、Yggdrasil 协议错误体、Drive 错误码或 config key 放进 Forge。Forge 的职责是共享限流机械件，产品侧负责面向客户端的语义。

## Client IP

模块：`aster_forge_actix_middleware::client_ip`

主要函数：

- `real_ip_from_headers(headers, peer, trusted_proxies)`
- `real_ip_from_trusted_headers(headers, peer, trusted)`

这个模块只做 Actix `HeaderMap` 适配：从请求头里读取 `X-Forwarded-For`，然后把可信代理判断交给
`aster_forge_utils::net`。适合 service 或 audit 代码已经拿到 `HttpRequest` / `HeaderMap`，但不想重复写
header 解析逻辑的场景。可信 peer 的左侧 forwarded 值支持裸 IPv4/IPv6，也支持代理常见的
`IPv4:port` 与 `[IPv6]:port` 形式；非法值回退到 direct peer。

```rust
use aster_forge_actix_middleware::client_ip::real_ip_from_headers;

let peer = req.peer_addr().map(|socket| socket.ip());
let client_ip = peer.map(|peer| {
    real_ip_from_headers(
        req.headers(),
        peer,
        &state.config().network_trust.trusted_proxies,
    )
});
```

产品侧仍然负责：

- 从配置里读取 trusted proxy 列表；
- 决定 peer address 缺失时返回 `None`、localhost 还是产品错误；
- 把 client IP 写入审计、协议缓存或日志字段。

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

真实 recorder 和 backend 由 `aster_forge_metrics` 负责；Actix `/metrics` endpoint 由
`aster_forge_actix_observability` 负责。产品侧只需要把 shared recorder 放进 app data，并保持业务
label 低基数。

## Runtime CORS

模块：`aster_forge_actix_middleware::cors`

主要类型：

- `RuntimeCors`
- `RuntimeCorsConfig`
- `RuntimeCorsPolicy`
- `CorsAllowedOrigins`
- `CorsMiddlewareError`
- `CorsMiddlewareErrorKind`

Forge 负责产品无关的 Actix CORS 机械行为：

- 读取 `Origin`。
- 判断 same-origin、preflight 和普通跨源请求。
- 校验 `Access-Control-Request-Method` 与 `Access-Control-Request-Headers`。
- 应用 `Access-Control-Allow-Origin`、`Access-Control-Allow-Credentials`、`Access-Control-Allow-Methods`、`Access-Control-Allow-Headers`、`Access-Control-Max-Age`、`Access-Control-Expose-Headers`。
- 维护 `Vary`。
- 对不允许的跨源请求返回 `403`。

产品侧通过 `RuntimeCorsConfig` 注入：

- runtime policy resolver，例如从 `AppState` 读取当前 `RuntimeConfig`。
- exempt path predicate，例如静态前端资源、favicon、service worker。
- allowed methods、allowed request headers、exposed response headers。
- `CorsMiddlewareError` 到产品错误类型的映射。

`CorsMiddlewareErrorKind` 区分两类边界：

- `InvalidRequest`：客户端传入了非法 `Origin` 或 preflight header，通常映射为产品的 `400` validation error。
- `InvalidResponse`：下游响应或 middleware 生成的 header 无法序列化，通常映射为产品的 `500` internal error。

这样产品可以保留稳定错误码，而不需要根据错误字符串猜测来源。

典型接入：

```rust
use actix_web::{Error, dev::ServiceRequest, web};
use aster_forge_actix_middleware::cors::{
    CorsAllowedOrigins, CorsMiddlewareError, CorsMiddlewareErrorKind, RuntimeCors,
    RuntimeCorsConfig, RuntimeCorsPolicy,
};

fn runtime_cors() -> RuntimeCors {
    RuntimeCors::new(
        RuntimeCorsConfig::new(
            |req: &ServiceRequest| {
                let state = req
                    .app_data::<web::Data<AppState>>()
                    .ok_or_else(|| AsterError::internal_error("AppState not found"))?;
                Ok(RuntimeCorsPolicy {
                    enabled: state.runtime_config().cors_enabled(),
                    allowed_origins: CorsAllowedOrigins::List(state.runtime_config().cors_origins()),
                    allow_credentials: state.runtime_config().cors_credentials(),
                    max_age_secs: state.runtime_config().cors_max_age_secs(),
                })
            },
            |path| path == "/" || path.starts_with("/assets/"),
            |error: CorsMiddlewareError| -> Error {
                match error.kind() {
                    CorsMiddlewareErrorKind::InvalidRequest => {
                        AsterError::validation_error(error.message()).into()
                    }
                    CorsMiddlewareErrorKind::InvalidResponse => {
                        AsterError::internal_error(error.message()).into()
                    }
                }
            },
        )
        .allowed_methods(["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"])
        .allowed_headers(["authorization", "content-type", "x-csrf-token", "x-request-id"])
        .exposed_headers(["content-length", "etag", "x-request-id"]),
    )
}
```

不要把产品 auth、admin 权限、用户实体、config key 或错误码写进 Forge。不同产品可以用同一套 CORS 机械层，但通过不同的 policy resolver 和 header 列表表达自己的业务需求。

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
- CORS preflight allow/deny、普通跨源 allow/deny、same-origin bypass、`Vary` 头。
- CSRF token 生成、cookie/header mismatch、来源 header 校验。
- metrics enabled 时成功和错误响应都会记录。
- metrics disabled 或缺失 recorder 时不影响请求。
- 默认安全头出现在响应里。
- 特殊路由如果不能使用默认安全头，需要有单独测试说明原因。

## 参考项目

- AsterYggdrasil：Actix app 初始化和 request id 日志链路。
- AsterDrive：WebDAV、WOPI、预览等路由如果需要特殊 header，优先在产品侧覆盖，不反推 Forge 默认值。

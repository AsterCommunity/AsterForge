# aster_forge_middleware

`aster_forge_middleware` 收纳共享 HTTP 中间件，并通过 `actix` / `axum` feature 提供框架适配。

## 适用场景

- 给每个请求生成并在服务内部传播 request id。
- 注入基础安全响应头。
- 提供 CSRF token 和 request source 校验 helper。
- 为 Actix 和 Axum 提供同一套动态 CORS 校验与响应头机制。
- 提供可信代理真实 IP 提取和通用 keyed rate limiter。
- 可选记录 Actix/Axum HTTP 请求指标。
- 让 Drive、Yggdrasil、Gate 等产品复用 HTTP 基础机制，产品自行选择路由与策略。

不适合放在这里的内容：

- 认证、权限、管理员校验。
- 产品审计上下文。
- 依赖产品配置表或用户实体的 middleware。

## Cargo 接入

```toml
[dependencies]
aster_forge_middleware = { git = "https://github.com/AsterCommunity/AsterForge" }
```

默认启用 `actix`，不启用 `metrics`。Axum 产品必须关闭默认 feature，避免引入 Actix。`shared` 核心不要求任何 transport feature。

如果产品要使用 `aster_forge_middleware::actix::metrics::MetricsMiddleware`，需要显式开启：

```toml
aster_forge_middleware = { git = "https://github.com/AsterCommunity/AsterForge", features = ["metrics"] }
```

## 能力与模块边界

| 能力 | 共享核心 | Actix 适配 | Axum 适配 |
| --- | --- | --- | --- |
| CSRF | `shared::csrf`：token、常量时间比较、来源校验、错误分类 | `actix::csrf`：request/cookie helpers | `axum::csrf`：helpers、`CsrfConfig` + `csrf` |
| Runtime CORS | `shared::cors`：policy、错误类型、同一份校验/响应头引擎 | `RuntimeCors` + `RuntimeCorsConfig` | `RuntimeCorsConfig` + `runtime_cors` |
| Client IP | `aster_forge_utils::net`：IP/CIDR 与 forwarded 解析 | `actix::client_ip` | `axum::client_ip`：HTTP headers、ConnectInfo |
| Rate limit | `shared::rate_limit`：governor quota、normalized string keys、retry metadata | `actix::rate_limit`：governor config/extractor | `axum::rate_limit`：IP/string middleware |
| Request ID | 两端统一生成新的 UUID v4 | `RequestIdMiddleware` | `request_id` |
| Security headers | `shared::security_headers`：默认值 | `default_headers()` | `security_headers` |
| HTTP metrics | `aster_forge_metrics`：recorder | `MetricsMiddleware` | `metrics` |

已有 Actix helper/type import 保持有效。CSRF 校验、CORS policy 和 keyed limiter 来自 `shared`，纯机械件的新接入优先使用 `shared::*`。Actix 的 `CsrfTokenNames` 只适配 HTTP 0.2 header 类型，保留 `header_name()` 的原有返回类型；共享核心与 Axum 使用 HTTP 1 header 类型。两种 adapter 共用同一份名称校验规则。

## Axum 接入

```toml
aster_forge_middleware = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["axum"] }
# 需要 HTTP metrics 时再加入 "metrics"；Axum-only 依赖树不包含 actix-web/actix-governor。
```

最小接入：

```rust
use axum::{Router, middleware, routing::get};
use aster_forge_middleware::axum::{request_id, security_headers};

let app: Router = Router::new()
    .route("/health", get(|| async { "ok" }))
    .layer(middleware::from_fn(security_headers))
    .layer(middleware::from_fn(request_id));
```

完整、可编译的 [Axum 中间件示例](https://github.com/AsterCommunity/AsterForge/blob/master/crates/aster_forge_middleware/examples/axum_middleware.rs) 同时安装 CSRF、动态 CORS、IP 限流、安全头和 request ID：

```bash
cargo run -p aster_forge_middleware --example axum_middleware --no-default-features --features axum
# 可选 recorder 安装顺序也包含在示例中：
cargo check -p aster_forge_middleware --example axum_middleware --no-default-features --features axum,metrics
```

示例中的 origin 和 quota 是产品示例配置，不是 Forge 默认策略。真实产品通过捕获的 `Arc<AppState>` 或每次请求的 extension 读取运行时配置，不要把 `AppState`、登录态或业务错误码塞进 Forge。

安装与错误边界：

- `from_fn_with_state(config, handler)` 的 `State` 是该中间件的配置，独立于产品 Router 的 `State<AppState>`。resolver 可以捕获 `Arc<AppState>`，按请求读取最新 snapshot。
- Axum 最后添加的 `.layer(...)` 最先处理请求。示例的外到内顺序是：可选 `Extension(recorder)` → metrics → request ID → security headers → CORS → IP limit → 选定路由的 CSRF → handler。CORS preflight 在 CSRF 之前处理；短路响应仍经过外层 request ID、安全头和 metrics。
- 需要从 extensions 读取产品状态或 recorder 时，把 `Extension(...)` 放在相应 middleware 外层。metrics 从 `SharedMetricsRecorder` extension 读取 recorder；缺失或 disabled 时透传。使用 `Router::layer` 可以同时统计 fallback 请求；matched pattern 优先，未知路径使用低基数标签。
- `CsrfConfig::new(names, protect, source, map_error)` 的 predicate 表达产品的 cookie-authenticated 路由范围；安全方法总是跳过。source resolver 返回 `RequestSourcePolicy`，包含可信 scheme/host、规范化的 public origins 和 source mode。校验先检查来源，再做 double-submit。
- `RuntimeCorsConfig::new(policy, exempt_path, map_error)` 每次跨源请求重新解析 policy。`enabled = false` 或 `CorsAllowedOrigins::None` 在解析 Origin 前透传；`List([])` 是显式空白名单，拒绝跨源请求。`Any` + credentials 会反射具体 origin，不发送 `*`。
- `RuntimeCorsConfig::request_origin(...)` 默认返回 `None`，关闭 same-origin bypass。只有产品提供经过信任校验的规范化 origin 后才启用 bypass。Host、Forwarded、X-Forwarded-Proto 本身不能证明来源可信。Actix 保留已有 `connection_info()` 行为，产品必须在入口控制或清理代理头。
- CSRF source、CORS policy/request-origin 和 keyed-limit key resolver 失败时可以返回 `Err(Box::new(product_response))`，该响应原样保留。CSRF/CORS 校验错误分别交给 `map_error`；CORS 正常策略拒绝与 Actix 一样返回 403。`InvalidRequest` / `InvalidResponse` 分别供产品映射到 4xx / 5xx。
- IP 限流通过 `ConnectInfo<SocketAddr>` 获取 direct peer，服务要使用 `into_make_service_with_connect_info::<SocketAddr>()`。其他可信 transport 可以显式设置 `peer_resolver`。缺失 peer 时 client IP helper 返回 `None`，IP limiter 使用共享 localhost 桶且忽略所有 forwarded headers；UDS 产品应关闭 IP limit 或改用字符串 key。
- `IpRateLimitConfig::new(enabled, seconds, burst, trusted_proxies, rejection)` 和 `KeyedRateLimitConfig::new(limiter, key, rejection)` 由产品提供 rejection response。使用 `RateLimitRejection::retry_after_seconds()` 写入 429 和 Retry-After；所有不足整秒的等待向上取整。配置克隆共享限流状态，disabled 时也跳过 peer/key resolver。
- keyed/IP limiter 的 `retain_recent()` 可由产品 runtime 的维护任务调用，回收过期 key；Forge 不创建隐藏后台任务。
- `/metrics` 路由仍属于 `aster_forge_observability::axum`，不由本 crate 注册。

## Actix 接入示例

[完整 Actix 示例](https://github.com/AsterCommunity/AsterForge/blob/master/crates/aster_forge_middleware/examples/actix_middleware.rs) 参考 AD 的 `runtime/components.rs`、CORS resolver 和 rate-limit response adapter：通过 `web::Data<AppState>` 按请求读取 policy，产品映射校验错误与 429，选定 API scope 保护 unsafe 写操作。

```bash
cargo run -p aster_forge_middleware --example actix_middleware
cargo check -p aster_forge_middleware --example actix_middleware --features metrics
```

Actix 最后安装的 `.wrap(...)` 最先处理请求。示例从外到内为代理来源边界 → 可选 metrics → request ID → security headers → CORS → Governor → API scope CSRF → handler。两份示例都监听 `127.0.0.1:3000`，分别运行；示例 `/api/demo` 的 POST 使用 `example_csrf` cookie 与 `X-Example-CSRF` header，`/health` 不安装 CSRF。

该示例直接监听 TCP、不信任任何代理，因此在 `connection_info()` 首次读取之前移除影响 scheme/host 的代理头。真实产品应从 direct peer 判定可信代理后处理这些头，并保留自己的 cookie/bearer 认证边界。AD 的业务错误码、认证、权限和 runtime component 继续归 AD；示例只展示 middleware 组合。

## Rate Limit

模块：`shared::rate_limit`（框架无关）与 `actix::rate_limit`（Actix governor 适配）；Axum 见上文。

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
- 无 peer 地址的部署（Unix domain socket）所有客户端回落共享 `127.0.0.1` 这一个限流桶，一人突发全员被拒；UDS 形态的产品应禁用 IP 限流或改用 `NormalizedStringRateLimiter` 业务 key 限流。
- 为 `actix-governor` 提供可复用 IP key extractor。
- 从非零 `(seconds_per_request, burst_size)` 构造 governor quota。
- 允许产品注入自己的 `429` response factory，同时继续复用可信代理和 client IP 提取。
- 提供按字符串 key 限流的 `NormalizedStringRateLimiter`，默认 trim 并 lowercase key。
- 把 governor rejection 转成可复用的 `retry_after_seconds`；所有不足整秒的等待向上取整，最小为 1 秒，避免 `Retry-After: 0` 诱导客户端立即重试。

产品侧仍然负责：

- 配置结构、默认值、热更新策略。
- `429 Too Many Requests` 的 response body、错误码和本地化文案。
- 决定哪些路由使用 IP 限流，哪些协议端点使用 username/email/provider id 等业务 key 限流。
- 审计、指标标签和安全事件记录。

典型 Actix API 接入：

```rust
use actix_governor::GovernorConfig;
use actix_web::http::StatusCode;
use aster_forge_middleware::actix::rate_limit::{
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
use aster_forge_middleware::shared::rate_limit::NormalizedStringRateLimiter;
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

模块：`aster_forge_middleware::actix::client_ip`

主要函数：

- `real_ip_from_headers(headers, peer, trusted_proxies)`
- `real_ip_from_trusted_headers(headers, peer, trusted)`

这个模块只做 Actix `HeaderMap` 适配：从请求头里读取 `X-Forwarded-For`，然后把可信代理判断交给
`aster_forge_utils::net`。适合 service 或 audit 代码已经拿到 `HttpRequest` / `HeaderMap`，但不想重复写
header 解析逻辑的场景。可信 peer 的左侧 forwarded 值支持裸 IPv4/IPv6，也支持代理常见的
`IPv4:port` 与 `[IPv6]:port` 形式；非法值回退到 direct peer。

```rust
use aster_forge_middleware::actix::client_ip::real_ip_from_headers;

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

模块：`aster_forge_middleware::actix::metrics`

主要类型：

- `MetricsMiddleware`
- `MetricsService`

接入方式：

```rust
use aster_forge_middleware::actix::metrics::MetricsMiddleware;
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
`aster_forge_observability` 负责。产品侧只需要把 shared recorder 放进 app data，并保持业务
label 低基数。

## Runtime CORS

模块：`aster_forge_middleware::actix::cors`

主要类型：

- `RuntimeCors`
- `RuntimeCorsConfig`
- `RuntimeCorsPolicy`
- `CorsAllowedOrigins`
- `CorsMiddlewareError`
- `CorsMiddlewareErrorKind`

Forge 负责产品无关的 Actix CORS 机械行为：

- 读取 `Origin`。
- 策略未启用或没有有效允许来源时，在解析 `Origin` 前原样透传请求。
- 判断 same-origin、preflight 和普通跨源请求。
- 校验 `Access-Control-Request-Method` 与 `Access-Control-Request-Headers`；header 名匹配不区分大小写（配置里的 `Content-Type` 能匹配 preflight 请求的 `content-type`）。
- 应用 `Access-Control-Allow-Origin`、`Access-Control-Allow-Credentials`、`Access-Control-Allow-Methods`、`Access-Control-Allow-Headers`、`Access-Control-Max-Age`、`Access-Control-Expose-Headers`。
- 维护 `Vary`。
- 对不允许的跨源请求返回 `403`。

产品侧通过 `RuntimeCorsConfig` 注入：

- runtime policy resolver，例如从 `AppState` 读取当前 `RuntimeConfig`。
- exempt path predicate，例如静态前端资源、favicon、service worker。
- allowed methods、allowed request headers、exposed response headers。
- 可选的额外 origin scheme 解析规则；这只扩展语法，最终仍需精确 origin 白名单匹配。
- `CorsMiddlewareError` 到产品错误类型的映射。

`CorsMiddlewareErrorKind` 区分两类边界：

- `InvalidRequest`：客户端传入了非法 `Origin` 或 preflight header，通常映射为产品的 `400` validation error。
- `InvalidResponse`：下游响应或 middleware 生成的 header 无法序列化，通常映射为产品的 `500` internal error。

这样产品可以保留稳定错误码，而不需要根据错误字符串猜测来源。

典型接入：

```rust
use actix_web::{Error, dev::ServiceRequest, web};
use aster_forge_middleware::actix::cors::{
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
        .exposed_headers(["content-length", "etag", "x-request-id"])
        .additional_origin_schemes(["chrome-extension"]),
    )
}
```

`additional_origin_schemes` 不按 scheme 放行请求。即使配置了 `chrome-extension`，策略中仍然
必须列出完整的 `chrome-extension://<extension-id>`，其他扩展 ID 会继续被拒绝。通用
HTTP(S) origin、public site URL 和 CSRF 来源解析不受这个 CORS 专用扩展点影响。

不要把产品 auth、admin 权限、用户实体、config key 或错误码写进 Forge。不同产品可以用同一套 CORS 机械层，但通过不同的 policy resolver 和 header 列表表达自己的业务需求。

## CSRF

模块：`shared::csrf`（框架无关）与 `actix::csrf`（Actix request adapter）；Axum 见上文。

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
- cookie 与 header 的 double-submit 校验（常量时间比较，避免 token 值的时序侧信道）。默认兼容名是 `aster_csrf` 和 `X-CSRF-Token`，但产品可以传入 `CsrfTokenNames` 使用自己的 cookie/header 名。
- `Origin` / `Referer` / `Sec-Fetch-Site` 来源校验。
- 请求 source header 长度上限和 origin 规范化。

产品侧负责把 `CsrfErrorKind` 映射到自己的错误码、HTTP response 和审计字段。例如 Drive 可以映射到 `ApiErrorCode::AuthCsrfCookieMissing`，Yggdrasil 可以映射到自己的 `AsterError::auth_csrf_missing()`。

推荐接入方式：

```rust
use std::sync::OnceLock;

use actix_web::{HttpRequest, dev::ServiceRequest};
use aster_forge_middleware::actix::csrf::{
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

模块：`aster_forge_middleware::actix::request_id`

主要类型：

- `RequestIdMiddleware`
- `RequestId`

接入方式：

```rust
use aster_forge_middleware::actix::request_id::RequestIdMiddleware;

app.wrap(RequestIdMiddleware)
```

两端中间件都为每个请求生成新的 UUID v4，不接受客户端传入的 `X-Request-ID`。这个边界避免把未经信任、未限制格式的请求值当成服务内部标识。handler 从 request extensions 中读取 `RequestId`，响应的 `X-Request-ID` 返回同一值（覆盖下游同名头），用于日志与错误链路。

接入注意点：

- 产品侧决定是否把 request id 暴露给前端。
- 产品侧决定日志字段名，例如 `request_id`、`trace_id`。
- 不要在业务 service 里重新生成 request id，否则请求链路会断。

## Security headers

模块：`aster_forge_middleware::actix::security_headers`

主要 API：

- `default_headers()`
- `X_FRAME_OPTIONS_VALUE`
- `REFERRER_POLICY_VALUE`
- `X_CONTENT_TYPE_OPTIONS_VALUE`

接入方式：

```rust
use aster_forge_middleware::actix::security_headers::default_headers;

app.wrap(default_headers())
```

两端都只补缺失的响应头，保留产品已设置的 `no-referrer`、`DENY` 等策略。默认头用于普通后端管理界面和 API 服务：

- `X-Frame-Options: SAMEORIGIN`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `X-Content-Type-Options: nosniff`

如果产品有 WOPI、iframe preview 或跨站嵌入需求，不要硬改 Forge 默认值。应该在产品侧选择是否使用默认 middleware，或者在产品侧单独实现更具体的 header 策略。

## 测试要求

接入产品仓库后至少覆盖：

- 无 request id 请求会生成 request id。
- 客户端传入的 request id 会被新的 UUID v4 替换，extension 与响应头一致。
- CORS preflight allow/deny、普通跨源 allow/deny、same-origin bypass、`Vary` 头。
- CSRF safe/unsafe 方法、cookie/header 缺失/空/不匹配、自定义名称、来源 header 优先级和长度边界。
- metrics enabled 时成功和错误响应都会记录。
- metrics disabled 或缺失 recorder 时不影响请求。
- 默认安全头出现在成功和失败响应里，保留产品已设置的更严格策略。
- 特殊路由如果不能使用默认安全头，需要有单独测试说明原因。

仓库测试全部使用 lib unit tests（各模块 `#[cfg(test)] mod tests`），Axum 测试通过真实 `Router` + `tower::ServiceExt::oneshot` 验证。双 transport feature 还会运行跨框架 request ID/security/CORS 契约测试。限流并发验证共享 burst，额度恢复使用 governor fake clock，不依赖 sleep。

```bash
bash scripts/test-middleware-features.sh
cargo clippy -p aster_forge_middleware --all-targets --all-features -- -D warnings
```

本地脚本覆盖无默认 feature、Actix、Axum、双 transport，以及各自带 metrics 的矩阵，并检查 Axum-only 正常依赖树无 Actix。示例也纳入编译；远端继续使用现有 workspace feature-matrix CI 执行 lib tests，不另建 CI job。

## 参考项目

- AsterYggdrasil：Actix app 初始化和 request id 日志链路。
- AsterDrive：WebDAV、WOPI、预览等路由如果需要特殊 header，优先在产品侧覆盖，不反推 Forge 默认值。

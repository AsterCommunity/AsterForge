# aster_forge_http

`aster_forge_http` 提供产品无关的 outbound HTTP 传输辅助。它通过 `HttpClient::builder()` 统一装配 reqwest 连接池，并负责用 `reqwest`
严格限制响应体大小，以及把内存中的 `http::Request<Vec<u8>>` 执行为有界的
`http::Response<Vec<u8>>`。

## 所有权边界

Forge 负责：

- 同时检查声明的 `Content-Length` 和实际流式累计大小。
- 在没有长度头、chunked 传输或长度头不可信时继续执行实际大小限制。
- 保留 response status、version、headers 和有界 body。
- 提供不包含响应正文的 transport 错误。

产品或协议 crate 继续负责：

- endpoint、timeout、proxy、TLS 和 User-Agent 策略；factory 固定禁止自动 redirect。
- 各调用点使用的具体字节上限。
- 非 2xx 状态处理和产品错误映射。
- response body 的协议解析。

这个 crate 不提供全局 client、endpoint registry 或产品错误类型。

## Cargo features 与 TLS 后端

默认 features 为空；基础能力包含 bytes body、请求签名入口、observer 和有界响应读取，不隐式启用 HTTPS。
workspace 的 reqwest 也关闭默认 features；下游 `default-features = false` 不再被 Forge 的依赖声明抵消。

| Feature | 能力 |
| --- | --- |
| `json` / `form` / `query` | 分别提供请求 builder 的 `.json()` / `.form()` / `.query()` |
| `multipart` | `.multipart()` 与 reqwest multipart 类型 |
| `stream` | reqwest streaming body/response API；有界读体自身无需此 feature |
| `blocking` | `build_blocking()` |
| `http2` / `charset` / `system-proxy` | HTTP/2、响应 charset 解码、系统代理发现 |
| `rustls-aws-lc-rs` | rustls + AWS-LC 默认密码提供者，使用平台信任验证 |
| `rustls-ring` | rustls + ring；产品入口必须先显式安装 ring provider |
| `rustls` | rustls 无内置 provider；供产品显式安装第三方或自选 provider |
| `native-tls` / `native-tls-vendored` | 平台 native TLS；后者在使用 OpenSSL 的平台编译 vendored OpenSSL |

ACME 类 JSON/bytes 消费方只需：

```toml
aster_forge_http = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["json", "rustls-aws-lc-rs"] }
```

这些 feature 拆分在 计划在尚未发布的 `0.1.0-alpha.2` 中交付；使用新版本或本次实现的固定 Git revision，不能把新 feature 名加在旧 alpha.1 依赖上。

ring 的应用启动代码（应用显式依赖 `rustls = { version = "0.23", default-features = false, features = ["std", "ring"] }`）：

```rust
rustls::crypto::ring::default_provider()
    .install_default()
    .map_err(|_| ProductError::conflicting_crypto_provider())?;
```

Forge 不自动安装全局 provider。多个 TLS/provider feature 可以因 Cargo 合并同时存在；产品需要固定 TLS 实现时使用
`HttpClient::builder().tls_backend(TlsBackend::Rustls)` 或 `TlsBackend::Native`，async/blocking 一致。
未显式选择时沿用 reqwest 对已编译后端的默认选择；provider 冲突由产品入口处理。
未编译 TLS 时不暴露证书及 TLS selector API；关闭 TLS feature 不等于关闭证书验证，HTTPS 需要显式选择一个受验证的后端。

外部认证默认保持 rustls/AWS-LC；切换时关闭其默认 feature 后显式选择 provider 和 TLS feature。
metrics 默认的 `rustls-aws-lc-rs` 只在启用 OTLP 时转发，不单独引入 HTTP；Mailpit 的本地 HTTP fixture 只请求 JSON。

## 读取有界响应体

```rust
let body = aster_forge_http::read_reqwest_body_limited(
    response,
    "WOPI discovery response",
    1024 * 1024,
    ProductError::upstream,
)
.await?;
```

`map_error` 让调用方保留自己的错误边界。错误消息只描述上下文和大小限制，不包含
响应正文。

## 执行 buffered HTTP 请求

```rust
let response = aster_forge_http::execute_reqwest_buffered_limited(
    &client,
    request,
    1024 * 1024,
)
.await?;
```

该 helper 适合要求 `http::Request<Vec<u8>>` / `http::Response<Vec<u8>>` adapter 的协议库，
例如 `openidconnect::AsyncHttpClient`。调用方传入由 `HttpClient::builder()` 装配的
`HttpClient`：factory 禁止自动 redirect，timeout 等其余策略由产品显式配置。

## 测试要求

- body 恰好等于限制时成功，超过一个字节时失败。
- `Content-Length` 超限在读取前失败。
- chunked 或缺失长度头的实际 body 超限时失败。
- buffered adapter 保留 status、version、headers 和 body。
- 错误的 `Display` / `Debug` 不包含响应正文。


## 统一装配与执行

```rust
let client = aster_forge_http::HttpClient::builder()
    .user_agent("ExampleProduct/1.0")
    .connect_timeout(std::time::Duration::from_secs(5))
    .timeout(std::time::Duration::from_secs(15))
    .build()?;
let request = client.get("https://api.example.com/resource").build()?;
// 协议签名可修改 request；execute 不重写 URL、header 或 body。
let response = client.execute(request).await?;
```

默认禁止自动重定向和自动重试，设置非空通用 UA、5 秒连接、30 秒读取/总超时，
90 秒 idle pool、每 host 8 个 idle connection。产品必须注入自己的 UA；启用 `system-proxy` 时代理默认继承系统，
可显式禁用或配置。TLS 保留证书和 hostname 验证，可添加信任根，不提供 insecure bypass。
DNS 地址固定强制禁用代理，不受 builder 调用顺序影响。
流式协议可显式取消总期限/读取期限，但必须自己提供取消、进度和确认响应边界。
`HttpClient` clone 共享连接池；没有 raw client accessor 或 Deref。

`HttpObserver` 只接收 method、status、outcome、duration，不接收 URL、header、响应正文。
观测边界是开始执行到响应头，不是响应体 EOF；future 被 drop 记录 Cancelled。
文件传输的完整生命周期指标仍由产品负责，observer 不把短期响应头成功冒充传输完成。
`HttpRequestBuilder::build` 保留签名入口，`send` 和 `execute` 都经过同一观测边界。

`blocking` feature 为同步 SDK 提供 `build_blocking()`，必须在 async runtime 外构造/销毁。
blocking transport 与 async 共用 UA/redirect/proxy/TLS/pool 装配规则，但 SDK 自己拥有
同步 I/O、读体限制和观测；不安装 async observer 或 async idle-read timer。
当前消费者是 OTLP HTTP/protobuf exporter，认证 provider 使用可注入的 async HttpClient。
测试 fixture 和 Mailpit 支持也使用 factory。

运行 `python3 scripts/check-http-construction.py --self-test` 检查新增构造旁路。
底层 raw reqwest builder 只允许存在于 `aster_forge_http/src/builder.rs` 的 async/blocking 两处。

独立消费检查：`python3 scripts/test-http-crypto-features.py`。fixture 在 workspace 外解析，检查最小依赖、单 feature、组合、禁用 API、AWS-LC/ring/native TLS 隔离；crate 测试验证 TLS trust/hostname、请求保持、取消与有界读取。

依赖数量参考（2026-11-11，reqwest 0.13.5，`aarch64-apple-darwin`）：相同 rustls + JSON 消费配置，从 alpha.1 的 115 个普通依赖包降至 91 个。用 workspace 外 fixture 的 `cargo tree --target aarch64-apple-darwin --edges normal --prefix none --format '{p}'` 去重；不计 consumer、dev/build 依赖，不用整个 lockfile 的跨平台条目数冒充实际依赖面。数字受平台和依赖版本影响，不是二进制体积或吞吐量保证。

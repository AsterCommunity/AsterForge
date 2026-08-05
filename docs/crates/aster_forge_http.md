# aster_forge_http

`aster_forge_http` 提供产品无关的 outbound HTTP 传输辅助。目前它负责用 `reqwest`
严格限制响应体大小，以及把内存中的 `http::Request<Vec<u8>>` 执行为有界的
`http::Response<Vec<u8>>`。

## 所有权边界

Forge 负责：

- 同时检查声明的 `Content-Length` 和实际流式累计大小。
- 在没有长度头、chunked 传输或长度头不可信时继续执行实际大小限制。
- 保留 response status、version、headers 和有界 body。
- 提供不包含响应正文的 transport 错误。

产品或协议 crate 继续负责：

- endpoint、redirect、timeout、proxy、TLS 和 User-Agent 策略。
- 各调用点使用的具体字节上限。
- 非 2xx 状态处理和产品错误映射。
- response body 的协议解析。

这个 crate 不提供全局 client、endpoint registry 或产品错误类型。

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
例如 `openidconnect::AsyncHttpClient`。redirect 和 timeout 行为来自调用方传入的
`reqwest::Client`。

## 测试要求

- body 恰好等于限制时成功，超过一个字节时失败。
- `Content-Length` 超限在读取前失败。
- chunked 或缺失长度头的实际 body 超限时失败。
- buffered adapter 保留 status、version、headers 和 body。
- 错误的 `Display` / `Debug` 不包含响应正文。

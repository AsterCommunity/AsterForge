//! API 中间件：通用安全响应头。

use actix_web::middleware::DefaultHeaders;

pub const X_FRAME_OPTIONS_VALUE: &str = "SAMEORIGIN";
pub const REFERRER_POLICY_VALUE: &str = "strict-origin-when-cross-origin";
pub const X_CONTENT_TYPE_OPTIONS_VALUE: &str = "nosniff";

/// 为所有响应补基础安全头。
///
/// 不在这里设置 HSTS：
/// - 当前服务端不终止 HTTPS；
/// - 真正的 HSTS 应该由前置 HTTPS 反向代理负责。
pub fn default_headers() -> DefaultHeaders {
    DefaultHeaders::new()
        .add(("X-Frame-Options", X_FRAME_OPTIONS_VALUE))
        .add(("Referrer-Policy", REFERRER_POLICY_VALUE))
        .add(("X-Content-Type-Options", X_CONTENT_TYPE_OPTIONS_VALUE))
}

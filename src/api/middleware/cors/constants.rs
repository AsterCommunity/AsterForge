//! CORS 中间件子模块：`constants`。

pub(super) const ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

pub(super) const ALLOWED_HEADERS: &[&str] = &[
    "authorization",
    "accept",
    "content-type",
    "range",
    "timeout",
    "x-csrf-token",
    "x-request-id",
];

pub(super) const EXPOSE_HEADERS: &[&str] = &["content-length", "etag", "x-request-id"];

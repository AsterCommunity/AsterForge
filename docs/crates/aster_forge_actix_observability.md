# aster_forge_actix_observability

`aster_forge_actix_observability` 提供 Actix Web 专用的观测 endpoint glue。它不负责记录指标，也不持有 metrics backend；这些职责属于 `aster_forge_metrics`。这个 crate 只解决一个问题：产品 route 模块不需要散落 backend-specific `#[cfg]`，也不需要重复写 `/metrics` 导出 handler。

## 适用场景

- 给 Actix Web 服务挂载 Prometheus `/metrics` endpoint。
- 把 route-level observability glue 从产品仓库和 middleware crate 中拆出来。
- 让产品路由无条件调用 helper，由 Cargo feature 决定 endpoint 是否存在。

不适合放在这里的内容：

- HTTP request metrics middleware；它属于 `aster_forge_actix_middleware`。
- metrics recorder trait、Prometheus registry、产品自定义 metric 注册；它们属于 `aster_forge_metrics`。
- 产品 dashboard、告警规则、业务指标命名策略。

## Cargo 接入

```toml
[features]
metrics = [
    "aster_forge_actix_observability/prometheus",
    "aster_forge_metrics/allocator-metrics",
    "aster_forge_metrics/backend-prometheus",
    "aster_forge_metrics/runtime-health",
]

[dependencies]
aster_forge_actix_observability = { git = "https://github.com/AsterCommunity/AsterForge" }
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Route 接入

产品 route 模块推荐无条件调用：

```rust
pub fn routes() -> actix_web::Scope {
    let scope = actix_web::web::scope("/health")
        .route("", actix_web::web::get().to(health))
        .route("/ready", actix_web::web::get().to(ready));

    aster_forge_actix_observability::configure_prometheus_route(scope)
}
```

当 `aster_forge_actix_observability/prometheus` feature 未启用时，`configure_prometheus_route` 返回原始 scope，不注册 `/metrics`。当 feature 启用时，它注册 `/metrics`，并从 `aster_forge_metrics::prometheus` 导出 Prometheus text exposition body。

## 启动顺序

`/metrics` endpoint 只负责导出，不负责初始化 backend。产品入口仍然应该通过 `aster_forge_metrics::init_configured_or_noop()` 初始化 recorder：

```rust
pub fn create_metrics_recorder() -> aster_forge_metrics::SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        return aster_forge_metrics::init_configured_or_noop();
    }

    aster_forge_metrics::NoopMetrics::arc()
}
```

如果 endpoint 被访问但 Prometheus registry 尚未初始化，它会返回 `503 Service Unavailable`，避免误导 scraper 认为服务已经暴露有效 metrics。

## 测试要求

- 未启用 `prometheus` feature 时，helper 不注册 `/metrics`。
- 启用 `prometheus` feature 且 registry 未初始化时，`/metrics` 返回 `503`。
- 启用 `prometheus` feature 且 registry 已初始化时，`/metrics` 返回 Prometheus text body。

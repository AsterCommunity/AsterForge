# aster_forge_observability

这个 crate 把一个显式传入的 `MetricsProvider` 暴露成 Prometheus HTTP 抓取端点。它提供 Actix 和 Axum adapter；指标定义、记录、聚合与导出预算属于 [aster_forge_metrics](./aster_forge_metrics.md)。

适合需要提供 `/metrics` 或 `/health/metrics` 的 HTTP 服务。只向 OTLP collector 推送 metrics 的进程不需要这个 crate。它也不提供 tracing、Tempo 接入、日志采集或 health/readiness 检查。

## 所有权与调用链

```text
产品静态配置与认证
  -> 产品决定是否挂载 route
  -> Forge Actix Resource / Axum Router
  -> 注入的 MetricsProvider.export_prometheus(5s)
  -> blocking pool gather + encode
  -> Prometheus 抓取
```

| Forge 负责 | 产品负责 |
| --- | --- |
| adapter、稳定导出响应、脱敏错误、no-store | 路径、端口、TLS、网络访问规则 |
| 显式实例注入和异步 handler | 是否启用、配置来源、认证和权限 |
| 调用 provider 的有界导出接口 | 创建和关闭 provider，启动采样任务 |
| Prometheus text 格式 | Prometheus scrape 配置、Grafana dashboard 与告警 |

库不读取产品 JWT、cookie、Basic Auth、环境配置或全局 registry。挂载 metrics 不会自动挂载或改变 health/readiness。

## Features 与显式启用

| Feature | 默认 | 提供的 API |
| --- | --- | --- |
| `actix` | 是 | Actix adapter 模块 |
| `axum` | 否 | Axum adapter 模块 |
| `prometheus` | 否 | exposition API，并启用 metrics crate 的 `backend-prometheus` |

三个 feature 独立。只有 `actix` 或 `axum` 而没有 `prometheus` 时，没有 metrics route 构造函数。编译 feature 不创建 reader、不注册路由；运行时仍须创建带 Prometheus reader 的 provider，并由产品显式挂载路由。

Actix 项目：

```toml
[dependencies]
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge", features = ["backend-prometheus"] }
aster_forge_observability = { git = "https://github.com/AsterCommunity/AsterForge", features = ["prometheus"] }
```

Axum 项目不需要引入默认 Actix 依赖：

```toml
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge", features = ["backend-prometheus"] }
aster_forge_observability = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["axum", "prometheus"] }
```

## Actix 最小接入

```rust
use actix_web::{App, web};
use aster_forge_metrics::{InfrastructureMetricsRecorder, MetricResult, MetricsProvider};

async fn example() -> MetricResult<()> {
    let provider = MetricsProvider::builder().with_prometheus()?.build()?;
    // 实际应用应保存 recorder，并注入 HTTP middleware、DB 和业务 producer。
    let recorder = InfrastructureMetricsRecorder::new(provider.clone())?;
    let _app = App::new()
        .app_data(web::Data::new(provider.clone()))
        .service(aster_forge_observability::actix::prometheus_resource("/metrics"));
    drop(recorder);
    provider.shutdown(std::time::Duration::from_secs(5)).await
}
```

`prometheus_resource(path)` 只创建一个 GET resource，`path` 是传入字符串。放进 `/health` scope 并传入 `/metrics` 时，最终路径是 `/health/metrics`。`web::Data<MetricsProvider>` 必须在 request 可见的 App/scope 层注入，类型不能替换成 recorder、provider 的另一层 Arc 或其他 wrapper。

生产应用按静态 `enabled` 决定是否调用 `service`。用 `resource.wrap(...)` 或 `wrap(from_fn(...))` 只包装该 metrics resource，并在导出 handler 前完成认证。这样未授权请求不会占用 gather 额度，且不会改变 liveness/readiness 的访问规则。密码校验限流、hash 参数预算与 401 响应由产品认证 adapter 负责。

## Axum 最小接入

```rust
use aster_forge_metrics::{MetricResult, MetricsProvider};

fn metrics_routes(provider: MetricsProvider) -> axum::Router {
    axum::Router::new().nest(
        "/health",
        aster_forge_observability::axum::prometheus_router(provider),
    )
}

fn create() -> MetricResult<axum::Router> {
    let provider = MetricsProvider::builder().with_prometheus()?.build()?;
    Ok(metrics_routes(provider))
}
```

`prometheus_router(provider)` 返回自带 `Extension<MetricsProvider>` 的 Router，内部固定路径 `/metrics`；上例最终路径是 `/health/metrics`。要只保护 exposition，在 nest/merge 到产品 router **之前**为这个 metrics router 添加认证 layer。产品仍要保留一个 provider clone，用于最终 flush/shutdown。

## HTTP 契约与错误边界

| 条件 | HTTP 行为 |
| --- | --- |
| 正常导出 | 200；`text/plain; version=0.0.4; charset=utf-8` |
| Actix 缺少 `web::Data<MetricsProvider>` | 503；`metrics provider is unavailable` |
| Noop、OTLP-only、provider 关闭、忙、超时、编码失败或结果超限 | 503；`metrics export is unavailable` |

adapter 产生的导出响应均有 `Cache-Control: no-store`。产品认证 middleware 的响应由产品负责；建议同样禁止缓存。方法不匹配、路径不存在等由框架路由器响应，不属于导出 handler 的上述合同。

HTTP 层不公开 provider 的内部错误、标签、endpoint、凭据或远端响应体。需要区分 `Busy`、`Timeout` 等故障时，在产品运维边界直接处理 provider API 的 `MetricError`；不要把敏感配置拼进 HTTP 错误。

## 负载与运行时生命周期

- handler 只 gather 内存中的指标，不执行产品 SQL、目录遍历、远端请求、健康检查或系统采样。
- gather/encode 放到 blocking pool；每个 provider 最多两个并行 gather，adapter 最多等待五秒。
- 已超时的 blocking job 仍持有额度，直到真正完成，防止持续请求无限堆积后台工作。
- 文本受 `MetricsLimits.max_exposition_bytes` 限制，默认 16 MiB。超过预算返回 503，不返回截断的成功响应。
- 不同 provider 的额度和系列相互隔离；clone 共享同一个实例。不要为每次请求创建 provider。

启动时先确定部署身份与静态配置，构建 provider 和 recorder，再注入 HTTP/DB/worker；系统及业务采样任务单独启动。关闭时先停止接收新请求，等待 producer 和采样任务退出，最后调用 `provider.shutdown(timeout).await`。超时与最终关闭结果的区别见 metrics crate 文档。

## 迁移、验证与参考

旧 `configure_prometheus_route(scope)` 已删除，改为显式 resource/Router 和实例注入。没有保留可绕过产品认证的全局导出入口。

```bash
cargo check -p aster_forge_observability --no-default-features --all-targets
cargo test -p aster_forge_observability --features prometheus --lib
cargo test -p aster_forge_observability --no-default-features --features axum,prometheus --lib
```

库测试覆盖正常导出、实例注入、关闭后的 503、格式和 no-store。产品测试还必须覆盖：feature 缺失、静态开关关闭时 route 不存在、认证先于导出故障、错误凭据、认证并发预算、角色边界以及 health/readiness 不受影响。

AsterDrive 的 `src/api/routes/health` 与 metrics Basic Auth middleware 是产品集成参考；路径和认证策略保留在 Drive。新项目先参照上面的显式注入形态，再按自身部署策略选择认证，不必复制 Drive 的业务配置树。

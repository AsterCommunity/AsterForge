# aster_forge_observability

Forge 提供可注入的 Actix/Axum Prometheus exposition adapter。指标内核和 reader 属于 `aster_forge_metrics`；路由路径、是否启用、认证、静态配置和产品错误策略归产品。

## Cargo 与能力

默认只有 `actix` adapter；`axum` 和 `prometheus` 独立可选。未编译 `prometheus` 时没有 exposition API，不会注册端点；OTLP-only 产品不需要这个 feature。

```toml
aster_forge_observability = { git = "https://github.com/AsterCommunity/AsterForge", features = ["prometheus"] }
```

## 最小接入

```rust
#[cfg(feature = "metrics")]
fn endpoint() -> actix_web::Resource {
    aster_forge_observability::actix::prometheus_resource("/metrics")
}
```

在 App 上注入 `web::Data<aster_forge_metrics::MetricsProvider>`。产品根据自己的静态配置决定是否调用 `service(endpoint())`，并可在 resource 上调用 `wrap` / `from_fn`，先鉴权再进入导出 handler。库不读取产品配置、JWT、cookie 或 Basic Auth。

Axum 使用 `prometheus_router(provider)`，返回带实例 Extension 的 Router；产品可以 nest 到自己的路径并在其外层加认证 layer。没有全局 registry 或匿名 probe scope 的自动挂载入口。

## HTTP 与运行时合同

- 成功：200，`text/plain; version=0.0.4; charset=utf-8`。
- 缺少实例注入、provider 无 pull reader、关闭、超限或导出失败：503，稳定脱敏文本。
- 所有导出响应包含 `Cache-Control: no-store`。
- handler 不初始化 metrics，也不做业务 SQL、目录遍历、健康探测或系统采样。
- gather 放进 blocking pool，最多两个并行工作，默认最多等待五秒；已超时工作仍占额度，编码结果受 provider 大小预算限制。

产品必须在实例生命周期内持有 provider，生产者退出后再关闭。认证需要只包此 resource，避免同时改变 health/readiness 的访问规则。AsterDrive 的静态 Basic Auth 是产品 adapter，不下沉到 metrics crate。

## 迁移与测试

旧 `configure_prometheus_route(scope)` 删除，替换为显式 resource/Router 和实例注入。没有兼容全局导出路径可绕过产品认证。

```bash
cargo check -p aster_forge_observability --no-default-features --all-targets
cargo test -p aster_forge_observability --features prometheus --lib
cargo check -p aster_forge_observability --no-default-features --features axum,prometheus --all-targets
```

测试应验证 feature absence、认证先行、独立实例、正常导出、503、no-store 和导出资源上限。产品仍需覆盖自己的角色、静态配置和认证矩阵。

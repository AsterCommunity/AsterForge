# aster_forge_metrics

`aster_forge_metrics` 提供共享 metrics trait、noop 实现和可注册的 metric descriptor catalog。它的核心目的是让子系统自己声明指标，由产品侧 recorder 决定如何落到 Prometheus、日志或 no-op。

## 适用场景

- 统一服务内 metrics recorder trait。
- 让 recorder 同时实现数据库查询、HTTP、后台任务和外部系统等通用 metrics hook。
- 提供 `NoopMetrics` 给测试和不启用 metrics 的部署。
- 让子系统通过 `MetricsSubsystem` 注册自己的 `MetricDescriptor`。

不适合放在这里的内容：

- Prometheus exporter 绑定。
- 产品 metric namespace。
- Grafana dashboard。
- 告警规则。

## Cargo 接入

```toml
[dependencies]
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## Recorder trait

核心类型：

- `DbMetricBackend`
- `DbQueryKind`
- `DbQueryMetric`
- `DbMetricsRecorder`
- `SharedDbMetricsRecorder`
- `NoopDbMetrics`
- `MetricsRecorder`
- `SharedMetricsRecorder`
- `NoopMetrics`
- `init_metrics_or_noop`

`MetricsRecorder` 包含数据库、HTTP、后台任务、外部系统等通用记录入口。产品可以实现自己的 recorder，把这些调用映射到实际后端。

数据库 query metrics 使用 `DbQueryMetric`，只包含 backend、低基数 query kind、status 和 elapsed duration。Forge 不把 SeaORM callback 类型或 SQL 全文暴露给 recorder；`aster_forge_db` 负责把 SeaORM metric callback 转成这个产品无关的结构。

测试中推荐：

```rust
let metrics = aster_forge_metrics::NoopMetrics::arc();
```

启动时如果产品侧有真实 metrics backend，可以把 feature 边界留在产品仓库，只把“失败后降级
noop”的机械逻辑交给 Forge：

```rust
fn create_metrics_recorder() -> aster_forge_metrics::SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        return aster_forge_metrics::init_metrics_or_noop(
            crate::metrics::init_metrics,
            || crate::metrics::PrometheusMetricsRecorder,
        );
    }

    aster_forge_metrics::NoopMetrics::arc()
}
```

这里 Forge 不持有 Prometheus registry，也不定义产品 metric namespace；产品只把“初始化函数”和
“recorder 构造器”传进来。

## 指标注册

核心类型：

- `MetricKind`
- `MetricDescriptor`
- `MetricCatalog`
- `MetricsSubsystem`
- `register_subsystems`

推荐模式：

1. Forge crate 或产品子系统实现 `MetricsSubsystem`。
2. 在启动时注册所有 subsystem。
3. 产品 recorder 根据 catalog 初始化真实指标。

这种注册式设计比在一个 metrics 文件里手写所有指标更稳，因为子系统新增指标时能跟代码放在一起。

## 接入边界

Forge 可以定义通用指标描述：

- DB query duration。
- background task transition。
- external operation duration。

产品侧应该决定：

- metric name 前缀。
- label cardinality 限制。
- 是否导出系统 metrics updater task。
- dashboard 和 alert。

## 测试要求

- 重复注册同一 subsystem/name 会报错。
- 产品 recorder 能接受所有已注册 descriptor。
- metrics disabled 时 `NoopMetrics` 不影响业务流程。
- 高基数字段不要作为 label，需要 code review 明确拦住。

## 参考项目

- AsterDrive：完整监控和 Grafana dashboard 适合作为产品侧 exporter 参考。
- AsterYggdrasil：后台任务 transition、pending task 和 DB metrics 的轻量接入。

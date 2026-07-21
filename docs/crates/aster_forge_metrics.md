# aster_forge_metrics

`aster_forge_metrics` 提供共享 metrics trait、noop 实现、可注册的 metric descriptor catalog，以及可选的 Prometheus 后端。它的核心目的是让 Aster 产品不再各自维护一份 Prometheus registry/exporter，同时仍然允许产品注册自己的业务指标。

## 适用场景

- 统一服务内 metrics recorder trait。
- 让 recorder 同时实现数据库查询、HTTP、后台任务和外部系统等通用 metrics hook。
- 提供 `NoopMetrics` 给测试和不启用 metrics 的部署。
- 让子系统通过 `MetricsSubsystem` 注册自己的 `MetricDescriptor`。
- 在 `backend-prometheus` feature 下提供统一的 Prometheus registry、exporter、系统指标 updater 和产品自定义指标注册 API。

不适合放在这里的内容：

- Grafana dashboard。
- 告警规则。
- 高基数业务 label 或产品私有 dashboard 语义。

## Cargo 接入

```toml
[features]
metrics = [
    "aster_forge_metrics/allocator-metrics",
    "aster_forge_metrics/backend-prometheus",
    "aster_forge_metrics/runtime-health",
]

[dependencies]
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `allocator-metrics`：把 `aster_forge_alloc::stats()` 暴露为 Prometheus heap metrics。
- `backend-prometheus`：启用 Forge 提供的 Prometheus backend。产品不需要直接依赖 `prometheus` 或 `sysinfo`。
- `runtime-health`：让 `PrometheusMetricsRecorder` 实现 `aster_forge_runtime::HealthMetricsRecorder`，用于 health report 记录。

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

`MetricsRecorder` 包含数据库、HTTP、运行时配置、后台任务、外部系统等通用记录入口。产品可以实现自己的 recorder，把这些调用映射到实际后端。

`record_http_request` 的 `method` / `route` 直接成为 Prometheus label 值，**必须传低基数路由模板**（如 `/api/v1/files/{id}`），禁止传原始请求路径——含 UUID/ID 的原始路径会让每个不同值分配一条永不释放的时间序列（用户输入驱动的内存炸弹）。Forge 自己的中间件调用点已经传模板；产品自实现调用点必须遵守同一约束。

`PrometheusMetricsRecorder::enabled()` 与注册表初始化状态一致：未经 `init_metrics()` 构造的裸 recorder 返回 `false`（此时 `record_*` 全部早退），调用方可以据此跳过回调安装。

数据库 query metrics 使用 `DbQueryMetric`，只包含 backend、低基数 query kind、status 和 elapsed duration。Forge 不把 SeaORM callback 类型或 SQL 全文暴露给 recorder；`aster_forge_db` 负责把 SeaORM metric callback 转成这个产品无关的结构。

运行时配置 metrics 使用两组 hook：

- `record_config_reload(source, decision, status, changed_keys, duration_seconds)`：记录跨进程 reload 通知处理结果。
- `record_config_mutation(source, operation, status, changed_keys)`：记录本地配置写入、删除或 action 触发的变更发布结果。

推荐 label 口径：

- `source`：`api`、`cli`、`startup`、`pubsub` 这类低基数字符串。
- `decision`：`reloaded`、`ignored_namespace`、`ignored_origin`。
- `operation`：`upsert`、`delete`，或者产品 action 映射出的低基数操作名。
- `status`：`ok`、`error`。

不要把配置 key、用户 ID、错误文本、topic、endpoint 或 runtime ID 放进 metrics label。key 数量用 `changed_keys` 记录；具体 key 可以写日志或审计。

测试中推荐：

```rust
let metrics = aster_forge_metrics::NoopMetrics::arc();
```

启动时推荐直接使用 Forge 的 Prometheus backend：

```rust
fn create_metrics_recorder() -> aster_forge_metrics::SharedMetricsRecorder {
    #[cfg(feature = "metrics")]
    {
        return aster_forge_metrics::init_configured_or_noop();
    }

    aster_forge_metrics::NoopMetrics::arc()
}
```

HTTP `/metrics` route 不应该在产品仓库重复实现。Actix Web 产品推荐使用
`aster_forge_actix_observability::configure_prometheus_route(scope)`：

```rust
pub fn routes() -> actix_web::Scope {
    let scope = actix_web::web::scope("/health")
        .route("", actix_web::web::get().to(health));

    aster_forge_actix_observability::configure_prometheus_route(scope)
}
```

产品侧不应该直接 import `prometheus::*`。如果需要业务指标，用下面的产品指标注册 API。

## Backend 选择原则

metrics backend 是进程级资源，应该由产品入口统一选择，不应该由各个子系统分别选择。普通 Aster 产品推荐：

1. 在产品 `metrics` feature 中启用一个 `aster_forge_metrics/backend-*` feature。
2. 如果产品已经使用 `aster_forge_alloc::TrackingAlloc` 或 jemalloc stats，启用 `aster_forge_metrics/allocator-metrics`。
3. 在 runtime 初始化时调用 `aster_forge_metrics::init_configured_or_noop()`。
4. 在业务代码中只调用 recorder trait 或产品 metrics helper，不写 backend-specific `#[cfg]`。

目前只提供 `backend-prometheus`。以后如果增加 OTLP 或其他 backend，Forge 会继续保持“同一构建只能启用一个 backend”的约束，避免多个 exporter 同时初始化造成不明确行为。

## Allocator Metrics

启用 `allocator-metrics` 后，Prometheus backend 会在导出和 system metrics updater tick 时刷新 allocator 统计：

```text
process_heap_memory_mib{kind="allocated"}
process_heap_memory_mib{kind="peak_or_resident"}
```

数据来源是 `aster_forge_alloc::stats()`：

- system allocator tracking：`allocated` 是当前已跟踪 heap MiB，`peak_or_resident` 是峰值 heap MiB。
- `jemalloc-stats`：`allocated` 是 jemalloc allocated MiB，`peak_or_resident` 是 jemalloc resident MiB。
- 只启用 `jemalloc` 但不启用 `jemalloc-stats`：两个值都是 `0`，这是 `aster_forge_alloc` 的显式降级行为。

产品仍然负责选择 `#[global_allocator]`。Forge metrics 只负责读取已经存在的 allocator stats 并导出低基数指标。

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

## Prometheus 产品指标

产品自定义指标推荐用 `product_metrics!` 声明成一个 typed metric set。注册阶段返回 typed handles；业务路径只调用 `inc`、`set`、`add`、`observe`，不需要直接保存裸 `ProductMetricHandle`，也不需要在每个调用点处理 metrics 错误。

```rust
#[cfg(feature = "metrics")]
aster_forge_metrics::product_metrics! {
    pub struct DriveProductMetrics {
        file_uploads: counter(
            "drive",
            "file_uploads_total",
            "Total Drive file upload attempts.",
            &["mode", "status"],
        ),
        active_upload_sessions: gauge(
            "drive",
            "active_upload_sessions",
            "Current active Drive upload sessions.",
            &["mode"],
        ),
        storage_operation_duration: histogram_with_buckets(
            "drive",
            "storage_operation_duration_seconds",
            "Drive storage operation duration.",
            &["driver", "operation", "status"],
            &[0.005, 0.025, 0.1, 0.5, 1.0, 5.0],
        ),
    }
}

#[cfg(feature = "metrics")]
fn register_product_metrics() -> aster_forge_metrics::prometheus::ProductMetricResult<DriveProductMetrics> {
    DriveProductMetrics::register()
}

#[cfg(feature = "metrics")]
fn record_upload(metrics: &DriveProductMetrics) {
    metrics.file_uploads.inc(&["direct", "ok"], 1);
    metrics.active_upload_sessions.set(&["direct"], 3.0);
    metrics
        .storage_operation_duration
        .observe(&["s3", "put_object", "ok"], 0.12);
}
```

实际 Prometheus metric name 会自动加上 subsystem 前缀，避免不同产品或模块都使用 `events_total` 这类短名时冲突。上面的 `file_uploads_total` 会导出为 `drive_file_uploads_total`。

如果产品需要严格处理 metrics 记录失败，可以使用 `try_*` 方法：

```rust
metrics.file_uploads.try_inc(&["direct", "ok"], 1)?;
metrics.active_upload_sessions.try_set(&["direct"], 3.0)?;
metrics
    .storage_operation_duration
    .try_observe(&["s3", "put_object", "ok"], 0.12)?;
```

推荐 API：

- `product_metrics! { ... }`：声明 typed metric set，并生成 `register()`。
- `ProductCounter::inc(...)` / `try_inc(...)`：记录 counter。
- `ProductGauge::set(...)` / `add(...)` / `try_set(...)` / `try_add(...)`：记录 gauge。
- `ProductHistogram::observe(...)` / `try_observe(...)`：记录 histogram。

低层 API 仍然保留，主要给框架适配、测试或动态 descriptor 场景使用：

- `register_product_metric(descriptor)`：注册单个产品指标。
- `register_product_metrics(descriptors)`：批量注册产品指标（全有或全无：中途失败会回滚本批已注册的族，修正后重试不会撞 `DuplicateRegistration`）。
- `inc_product_counter(handle, labels, value)`：记录 counter。
- `set_product_gauge(handle, labels, value)` / `add_product_gauge(handle, labels, value)`：记录 gauge。
- `observe_product_histogram(handle, labels, value)`：记录 histogram。
- `MetricDescriptor::histogram_with_buckets(...)`：为产品 histogram 指定 bucket。

边界规则：

- label value 数量必须和 descriptor 的 label names 完全一致，否则返回 `LabelCountMismatch`。
- counter/gauge/histogram 不能混用 handle，否则返回 `WrongKind`。
- 同一个 `subsystem + name` 只能注册一次。
- label 仍然必须低基数；不要把用户 ID、文件名、错误文本、config key、topic、endpoint 或 runtime ID 放进 label。

## 接入边界

Forge 可以定义通用指标描述：

- DB query duration。
- config reload / mutation outcome。
- background task transition。
- external operation duration。

产品侧应该决定：

- label cardinality 限制。
- dashboard 和 alert。
- 哪些业务动作值得注册为产品指标。

## 测试要求

- 重复注册同一 subsystem/name 会报错。
- Prometheus feature 下产品指标注册、重复注册、label 数量错误、kind 错误和 histogram bucket 都要有测试。
- metrics disabled 时 `NoopMetrics` 不影响业务流程。
- 高基数字段不要作为 label，需要 code review 明确拦住。

## 参考项目

- AsterDrive：`metrics` feature 已直接启用 `allocator-metrics`、`backend-prometheus` 和 `runtime-health`。database/cache/remote-node diagnostics 聚合成 Forge `SystemHealthReport` 后直接调用 `record_metrics()`；Drive 只保留文件、上传、存储 driver 等产品指标，不再复制 health metric 记录逻辑。
- AsterYggdrasil：后台任务 transition、pending task、DB metrics、config reload / mutation metrics 的轻量接入。

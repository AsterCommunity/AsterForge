# aster_forge_metrics

Forge 拥有实例级指标内核、通用基础设施 schema、typed instruments、采样和导出机制。产品拥有业务指标、标签分类、配置来源、认证策略及错误映射。日志和 traces 不属于本次 metrics 接口。

## Cargo 与启用边界

| Cargo feature | 编译能力 | 运行方式 |
| --- | --- | --- |
| 默认 | descriptors、typed instruments、Noop provider | builder 不添加 reader 时不采集 |
| `backend-prometheus` | Prometheus reader | 显式 `with_prometheus()` |
| `otlp` | OTLP HTTP/protobuf reader | 显式 `with_otlp(config)` |
| 两者 | 同一个 MeterProvider 的两个 reader | 同时添加，不重复业务埋点 |
| `allocator-metrics` | allocator 快照 | infrastructure updater 采样 |
| `runtime-health` | health report adapter | `recorder.record_health(scope, report)` |

feature 只提供能力，不初始化全局 provider，也不注册 HTTP 路由。配置和部署默认值由产品决定。

```toml
[dependencies]
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge", features = ["backend-prometheus", "runtime-health"] }
```

## 最小接入

```rust
use aster_forge_metrics::{MetricsProvider, InfrastructureMetricsRecorder, MetricResult};

aster_forge_metrics::product_metrics! {
    pub struct ProductMetrics {
        bytes: counter("file", "bytes_total", "Observed bytes.", &["direction"]).with_unit("By"),
        active: gauge("file", "active", "Active transfers.", &["direction"]),
        duration: histogram_with_buckets("file", "duration_seconds", "Transfer duration.", &["outcome"], &[0.01, 0.1, 1.0, 10.0]).with_unit("s"),
    }
}

fn create() -> MetricResult<(MetricsProvider, InfrastructureMetricsRecorder, ProductMetrics)> {
    let provider = MetricsProvider::builder().with_prometheus()?.build()?;
    let shared = InfrastructureMetricsRecorder::new(provider.clone())?;
    let product = ProductMetrics::register(&provider)?;
    product.bytes.inc(&["ingress"], 1024);
    Ok((provider, shared, product))
}
```

一个 metric set 先校验全部描述，再一次注册。重复名称、histogram 派生名称冲突、错误 kind、重复/非法 label、非法 unit/buckets 和资源超限返回 `MetricError`，失败不留下半套 schema。handles 直接绑定创建它们的实例；没有通过名称查全局 registry 的记录路径。

`inc`、`set`、`add`、`observe` 在观测失败时跳过该次记录；需要断言错误的边界使用 `try_*`。未知、高基数 label 不自动合并进一个误导性的正常系列。`MetricsLimits` 默认限制 512 instruments、每 instrument 4096 系列、全实例 16384 系列、单 label 256 bytes、exposition 16 MiB。

计数器在内核中用 `u64` 并检查溢出；gauge 使用有限 `f64`，支持有序 add/set；histogram 接受有限非负值。Prometheus 的数值格式存在 IEEE-754 精度边界，超过 2^53 的整数可能近似表示。OTLP 整数样本是 i64，超界 `u64` 在转换前拒绝该批导出，不能被转换成 0 或负数。没有承诺无限精度。

系列首次出现时分配 label 和绑定状态；预热后的记录复用借用查询和 OTel bound instruments。没有 exporter 网络调用或每条事件的标签字符串分配。系列在 provider 生命周期内保留，SDK 不提供 unregister；不能靠删除产品侧 handle 来回收 SDK 的系列预算。

## 通用指标与兼容

`InfrastructureMetricsRecorder` 提供 HTTP、DB、auth、application events、config reload/mutation、background tasks、external operations、health 与 process 指标。原 Prometheus 名称、HELP、类型、labels 和显式 buckets 保持；subsystem 不为空的产品指标仍按 `subsystem_name` 导出。单位由 descriptor 单独表达，exporter 不自动添加单位/`_total` 后缀。

`process_cpu_milliseconds_total` 仍是 **毫秒累计值的 gauge**，没有悄悄改为秒 counter。`process_memory_rss_bytes`、`process_uptime_seconds` 和可选 `process_heap_memory_mib{kind}` 的采样移到实例 updater；scrape 不刷新系统或 allocator 数据。

HTTP `route` 必须是路由模板；DB 只记录 backend/kind/status/elapsed，不记录 SQL。资源身份通过 `MetricsResource` 显式注入，不读取 SDK 环境资源检测器；产品应在每次进程启动时生成、并在进程内保持稳定的 `service.instance.id`，避免多个 OTLP 实例覆盖同一累计系列。产品用户、文件、对象 key、URL、SQL 和凭据不能成为指标 labels。

## OTLP

`OtlpMetricsConfig` 显式持有完整 `/v1/metrics` endpoint、headers、timeout、interval、请求/响应大小上限。只实现 HTTP/protobuf，HTTPS 使用系统信任；不提供无效的 gRPC、traces 或 logs 开关。不读取 `OTEL_*`、代理环境变量或 SDK credential headers；禁止重定向、URL 用户信息、query/fragment、保留 transport headers 和畸形 header。

同一 OTel MeterProvider 为 Prometheus 与 OTLP 分别维护 reader aggregation。OTLP 使用 cumulative temporality 与 descriptor buckets；重启会有新的 start timestamp。失败在下一周期重发累计快照，没有业务重试队列；scrape 不触发 push。只有 collector 成功确认才更新本地成功时间，不代表下游平台已经持久化。

`provider.otlp_diagnostics()` 提供本地成功/失败累计值与最后成功时间，不依赖故障 exporter 自报。infrastructure updater 同步到 `telemetry_export_successes`、`telemetry_export_failures`、`telemetry_export_last_success_timestamp_seconds{exporter="otlp"}`。endpoint、headers 和远端错误响应不出现在 Debug、错误或这些 labels 中。

## Runtime 与关闭

产品持有显式 provider/recorder，向 DB、HTTP middleware 和任务注入 recorder；通过 `aster_forge_observability` 注入 exposition resource。系统 updater 每个实例只启动一次，取消时结束，不在 scrape 中执行。

停止 HTTP/worker producer，等待仍产生指标的 audit、mail 和业务任务退出后，再 `provider.shutdown(timeout).await`。`force_flush` 和 shutdown 共用有界生命周期额度。超时仅表示调用者停止等待；blocking job 仍持有额度，可通过 `shutdown_result()` 读取真正结果，不允许超时后无限堆后台任务。成功/失败结果均缓存，关闭后的 handles 拒绝记录。产品决定 exporter 故障是否影响启动、readiness 或退出码。

## API 迁移

| 旧接口 | 替换 |
| --- | --- |
| 全局 `init_configured_or_noop` / `prometheus::init_metrics` | `MetricsProvider::builder()` + 显式 reader + `build()` |
| `PrometheusMetricsRecorder` | `InfrastructureMetricsRecorder::new(provider)` |
| `ProductMetrics::register()` | `ProductMetrics::register(&provider)` |
| `prometheus::Product*` handles/errors | crate 根的 `ProductCounter/Gauge/Histogram`、`MetricError` |
| 全局 `export_metrics` | 注入 provider 的 exposition resource |
| 全局 health report | 注入的 recorder `record_health` |

旧全局实现直接删除，不保留平行 registry 或双写。AsterDrive 本轮按本地/锁定 revision 验证；其他外部产品需要按本表升级，不能将 Forge 库内通过当作已经接入所有消费者。

## 验证入口

```bash
cargo test -p aster_forge_metrics --no-default-features
cargo test -p aster_forge_metrics --features backend-prometheus
cargo test -p aster_forge_metrics --features otlp
cargo test -p aster_forge_metrics --all-features
cargo run --release -p aster_forge_metrics --features backend-prometheus --example recording_cost
```

测试覆盖实例隔离、整批注册、schema、数值/基数、并发记录、Noop、关闭、真实本地 OTLP receiver 和双导出；微基准只测预热后记录成本，不代表产品端到端吞吐损失。

参考消费者为 AsterDrive `crates/aster_drive_metrics` 和 bundled Actix template；产品 upload、quota、blob cleanup 和 remote 语义不进入 Forge。

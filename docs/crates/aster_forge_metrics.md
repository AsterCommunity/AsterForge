# aster_forge_metrics

Forge 拥有实例级指标内核、通用基础设施 schema、typed instruments、采样和导出机制。产品拥有业务指标、标签分类、配置来源、认证策略及错误映射。日志和 traces 不属于本次 metrics 接口。

## 适用场景与概念

用于需要隔离实例、限制高基数、为基础设施和产品指标统一选择导出后端的 Rust 服务。HTTP、数据库、任务等模块依赖 recorder trait；产品启动层持有 provider。每个 provider 独立管理 schema、系列、reader 和关闭状态，不安装全局 OpenTelemetry provider。

| 名称 | 在本实现中负责什么 |
| --- | --- |
| metric | 一个被观测的量，如请求数、当前活跃数、耗时分布 |
| OpenTelemetry / OTel | 指标 API、SDK 聚合和 resource 模型；并非另一套业务指标 |
| OTLP | 把聚合后的数据推送给 collector 的协议；本 crate 仅实现 metrics 的 HTTP/protobuf |
| Prometheus | 定期抓取 HTTP exposition，存储时间序列并运行 PromQL |
| Grafana | 查询 Prometheus 等数据源，展示 dashboard 与告警 |
| Tempo | 保存与查询 traces；本 crate 不产生或导出 traces |

一次业务记录进入同一个 OTel MeterProvider，Prometheus reader 和 OTLP reader 分别读取自己的累计聚合。启用双导出不需要在业务调用处双写。耗时 histogram 提供次数、总耗时和桶分布，可计算速率或分位数；它不能告诉你某个具体请求在哪一层耗时。后者需要单独接入 tracing 与上下文传播。

```text
HTTP / DB / auth / tasks / 产品事件
  -> InfrastructureMetricsRecorder / product_metrics! typed handles
  -> 实例 MetricsProvider（校验、系列预算、OTel SDK）
       -> Prometheus reader -> exposition adapter <- Prometheus <- Grafana
       -> periodic OTLP reader -> collector -> 指标存储 <- Grafana
```

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

系列首次出现时分配 label 和绑定状态；预热后的记录复用借用查询和 OTel bound instruments。typed handle 记录路径不执行 exporter 网络调用，也不为预热系列复制标签字符串；上层 adapter 自身构造标签的成本仍需单独评估。系列在 provider 生命周期内保留，SDK 不提供 unregister；不能靠删除产品侧 handle 来回收 SDK 的系列预算。

## Provider、resource 与产品配置

`MetricsProvider::builder().build()` 创建无 reader 的 Noop provider。`enabled()` 为 false，仍可检查注册的 schema，但记录不会创建实际时间序列。编译 exporter feature 本身不改变这个默认行为。产品应把“编译支持”和“静态启用”分开：请求启用未编译能力时在启动阶段拒绝配置，关闭时跳过采样任务和昂贵标签计算。

```rust
use aster_forge_metrics::{MetricResult, MetricsProvider, MetricsResource};

fn create(instance_id: String) -> MetricResult<MetricsProvider> {
    MetricsProvider::builder()
        .with_resource(MetricsResource {
            service_name: "example-service".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
            environment: "production".into(),
            role: "api".into(),
            instance_id,
        })?
        .with_prometheus()?
        .build()
}
```

`with_resource` 要求非空 `service_name`；每个字段至多 128 bytes，不能包含控制字符。空的可选字段不导出。产品应生成每次进程启动唯一、进程内稳定的 instance ID，在创建 provider 前完成角色和环境判定。

| Rust 字段 | OTel resource attribute | Prometheus label |
| --- | --- | --- |
| `service_name` | `service.name` | `service_name` |
| `service_version` | `service.version` | `service_version` |
| `environment` | `deployment.environment.name` | `deployment_environment_name` |
| `role` | `aster.role` | `aster_role` |
| `instance_id` | `service.instance.id` | `service_instance_id` |

Prometheus 侧不另生成 `target_info`；上述属性直接附到样本。`job`、`instance` 通常由 Prometheus 抓取配置附加，不能混同于进程的 `service_instance_id`。OTLP 使用显式 resource，不运行 SDK 环境资源检测器。

provider 的 clone 共享实例，创建第二个 provider 才有独立系列和预算。共享 schema 和每套产品 schema 各注册一次，再 clone recorder/handles 注入消费者；重复注册名称会返回错误。

## 数值、schema 与资源预算

| 类型 | 记录 API | 值的边界与含义 |
| --- | --- | --- |
| Counter | `inc` / `try_inc` | `u64` 非负增量，累计溢出时拒绝 |
| Gauge | `set`、`add` / `try_set`、`try_add` | 有限 `f64`；可升可降，set/add 按系列同步 |
| Histogram | `observe` / `try_observe` | 有限非负 `f64`；显式递增桶边界 |

macro 中的 labels 是有序 schema；调用传入的值必须按相同顺序且数量相同。subsystem 非空时名称为 `subsystem_name`。单位存入 descriptor；Prometheus exporter 不自动添加单位或 `_total` 后缀，名称应由 schema 明确决定。histogram 的 `_bucket`、`_sum`、`_count` 派生名称也参与冲突检查。

| `MetricsLimits` 字段 | 默认值 | 构建时上限 |
| --- | --- | --- |
| `max_instruments` | 512 | 4096 |
| `max_series_per_instrument` | 4096 | 65536 |
| `max_total_series` | 16384 | 1048576 |
| `max_label_bytes` | 256 bytes | 4096 bytes |
| `max_exposition_bytes` | 16 MiB | 64 MiB |

预算均须大于零；通过 `builder.with_limits(...)` 显式调整。每个 descriptor 最多八个 label。预算控制内存和输出规模，不能代替产品有限枚举分类。路由使用模板；用户 ID、文件名、对象 key、完整 URL、SQL、token 和其他凭据不能进入 labels。未知分类应在产品边界映射到有限的 `unknown`/`other`，不要把原始输入直接交给 recorder。

内核超限拒绝新系列，已有系列可继续记录；不会悄悄把不同业务对象汇入正常标签。系列不能 unregister，删除 handle 不释放 SDK 已注册的系列。长时间运行服务必须按最大可能组合数设计标签，而不是按一次请求的标签数估算。

## 通用指标与兼容

`InfrastructureMetricsRecorder` 提供 HTTP、DB、auth、application events、config reload/mutation、background tasks、external operations、health 与 process 指标。原 Prometheus 名称、HELP、类型、labels 和显式 buckets 保持；subsystem 不为空的产品指标仍按 `subsystem_name` 导出。单位由 descriptor 单独表达，exporter 不自动添加单位/`_total` 后缀。

`process_cpu_milliseconds_total` 仍是 **毫秒累计值的 gauge**，没有悄悄改为秒 counter。`process_memory_rss_bytes`、`process_uptime_seconds` 和可选 `process_heap_memory_mib{kind}` 的采样移到实例 updater；scrape 不刷新系统或 allocator 数据。

HTTP `route` 必须是路由模板；DB 只记录 backend/kind/status/elapsed，不记录 SQL。资源身份通过 `MetricsResource` 显式注入，不读取 SDK 环境资源检测器；产品应在每次进程启动时生成、并在进程内保持稳定的 `service.instance.id`，避免多个 OTLP 实例覆盖同一累计系列。产品用户、文件、对象 key、URL、SQL 和凭据不能成为指标 labels。

## OTLP

`OtlpMetricsConfig` 显式持有完整 `/v1/metrics` endpoint、headers、timeout、interval、请求/响应大小上限。只实现 HTTP/protobuf，HTTPS 使用系统信任；不提供无效的 gRPC、traces 或 logs 开关。不读取 `OTEL_*`、代理环境变量或 SDK credential headers；禁止重定向、URL 用户信息、query/fragment、保留 transport headers 和畸形 header。

同一 OTel MeterProvider 为 Prometheus 与 OTLP 分别维护 reader aggregation。OTLP 使用 cumulative temporality 与 descriptor buckets；重启会有新的 start timestamp。失败在下一周期重发累计快照，没有业务重试队列；scrape 不触发 push。只有 collector 成功确认才更新本地成功时间，不代表下游平台已经持久化。

`provider.otlp_diagnostics()` 提供本地成功/失败累计值与最后成功时间，不依赖故障 exporter 自报。infrastructure updater 同步到 `telemetry_export_successes`、`telemetry_export_failures`、`telemetry_export_last_success_timestamp_seconds{exporter="otlp"}`。endpoint、headers 和远端错误响应不出现在 Debug、错误或这些 labels 中。

### OTLP 完整配置示例

`OtlpMetricsConfig` 没有隐式部署默认值；产品明确填写各个字段。以下同时启用 pull 与 push，需要两个 exporter feature：

```rust
use aster_forge_metrics::{MetricResult, MetricsProvider, OtlpMetricsConfig};
use std::{collections::BTreeMap, time::Duration};

fn create() -> MetricResult<MetricsProvider> {
    let builder = MetricsProvider::builder().with_prometheus()?;
    let (builder, diagnostics) = builder.with_otlp(OtlpMetricsConfig {
        endpoint: "http://127.0.0.1:4318/v1/metrics".into(),
        headers: BTreeMap::new(),
        timeout: Duration::from_secs(5),
        interval: Duration::from_secs(30),
        max_request_bytes: 16 * 1024 * 1024,
        max_response_bytes: 64 * 1024,
    })?;
    let provider = builder.build()?;
    let _initial_snapshot = diagnostics.snapshot();
    Ok(provider)
}
```

上例为便于展示省略 resource；生产环境应像前例一样注入唯一实例身份。endpoint 是完整 signal URL，库不会自动追加 `/v1/metrics`；部署时必须填 collector 实际接收 metrics 的路径。

| 配置 | 约束 |
| --- | --- |
| endpoint | HTTP/HTTPS、必须有 host，不能有 URL 用户信息、query 或 fragment |
| timeout | 大于零、不超过 60s，且不超过 interval |
| interval | 1s 到 1h |
| headers | 最多 16 个；每个值最多 4096 bytes，名称和值必须为合法 HTTP header |
| 保留 headers | 不接受 `content-type`、`content-length`、`host`、`connection`、`transfer-encoding` |
| 请求预算 | 大于零，最多 64 MiB |
| 响应预算 | 大于零，最多 1 MiB |

不接受重定向，也不读取代理环境变量；需要代理时使用明确可达的 collector endpoint。SDK 定期 worker 负责后台发送，业务记录不执行网络 I/O。HTTP 成功还必须得到可接受的 OTLP acknowledgement；部分拒绝等情况按失败处理。Debug、错误、诊断计数不暴露 endpoint、headers 或远端响应体。

### 通用基础设施 schema

所有名称为实际导出名；C = counter，G = gauge，H = histogram。health、allocator 和 exporter 项分别需要对应 feature。

| 名称 | 类型/单位 | labels |
| --- | --- | --- |
| `http_requests_total` | C / 1 | method, route, status |
| `http_request_duration_seconds` | H / s | method, route, status |
| `db_queries_total` | C / 1 | backend, kind, status |
| `db_query_duration_seconds` | H / s | backend, kind, status |
| `auth_events_total` | C / 1 | action, status, reason |
| `application_events_total` | C / 1 | category, event, status |
| `config_reloads_total` | C / 1 | source, decision, status |
| `config_reload_duration_seconds` | H / s | source, decision, status |
| `config_reload_changed_keys` | H / 1 | source, decision, status |
| `config_mutations_total` | C / 1 | source, operation, status |
| `config_mutation_changed_keys` | H / 1 | source, operation, status |
| `background_tasks_total` | C / 1 | kind, status |
| `background_tasks_pending` | G / 1 | 无 |
| `background_task_retries_total` | C / 1 | kind |
| `external_operations_total` | C / 1 | system, operation, status |
| `external_operation_duration_seconds` | H / s | system, operation, status |
| `health_report_status` | G / 1 | scope |
| `health_report_duration_seconds` | H / s | scope, status |
| `health_component_status` | G / 1 | scope, component |
| `health_component_duration_seconds` | H / s | scope, component, status |
| `process_memory_rss_bytes` | G / By | 无 |
| `process_cpu_milliseconds_total` | G / ms | 无 |
| `process_uptime_seconds` | G / s | 无 |
| `process_heap_memory_mib` | G / MiBy | kind |
| `telemetry_export_successes` | G / 未指定 | exporter |
| `telemetry_export_failures` | G / 未指定 | exporter |
| `telemetry_export_last_success_timestamp_seconds` | G / s | exporter |

health 状态数值是 healthy=0、degraded=1、unhealthy=2。background task 的 `status="retry"` 同时增加 retry counter。heap 的 kind 为 `allocated` 与 `peak_or_resident`。导出成功/失败虽然是累计数，当前 schema 的类型仍为 gauge。

库提供 schema 与记录入口，不会自动拦截全部 HTTP/SQL/auth 流程。产品须显式接入 middleware、数据库 adapter、任务状态转换和认证流程。auth recorder 只接收有限的 action/status/reason，不拥有产品登录策略或用户身份。histogram 精确桶定义可通过 `provider.descriptors()` 或 `src/shared.rs` 查询；变更桶、名称、labels 和单位均应视作监控 schema 变更。

## Runtime 与关闭

产品持有显式 provider/recorder，向 DB、HTTP middleware 和任务注入 recorder；通过 `aster_forge_observability` 注入 exposition resource。系统 updater 每个实例只启动一次，取消时结束，不在 scrape 中执行。

停止 HTTP/worker producer，等待仍产生指标的 audit、mail 和业务任务退出后，再 `provider.shutdown(timeout).await`。`force_flush` 和 shutdown 共用有界生命周期额度。超时仅表示调用者停止等待；blocking job 仍持有额度，可通过 `shutdown_result()` 读取真正结果，不允许超时后无限堆后台任务。成功/失败结果均缓存，关闭后的 handles 拒绝记录。产品决定 exporter 故障是否影响启动、readiness 或退出码。

### 系统 updater 的显式启动

```rust
use aster_forge_metrics::{InfrastructureMetricsRecorder, MetricsRecorder};
use tokio_util::sync::CancellationToken;

async fn run_sampler(recorder: InfrastructureMetricsRecorder) {
    let stop = CancellationToken::new();
    if let Some(task) = recorder.system_metrics_updater_task(stop.clone()) {
        let worker = tokio::spawn(task);
        // 实际服务在 shutdown hook 中取消，并等待 worker 退出。
        stop.cancel();
        let _result = worker.await;
    }
}
```

每个实例只启动一次 updater。它立即开始首轮采样，之后每 15s 采样；错过 tick 时跳过补偿，不堆积采样任务。系统刷新在 blocking pool 中进行，Noop 不返回 updater。OTLP diagnostics 在采样时同步到通用指标，因此通过 metrics 看到的诊断可能落后一个采样周期；直接 `snapshot()` 可读取本地最新计数。

系统采样使用 `sysinfo 0.40`，最低 Rust 版本仍为 1.95。`System` 在 blocking pool 内按需初始化；初始化失败时记录警告并在下个采样周期重试，uptime、allocator 和 OTLP diagnostics 采样继续进行。初始化成功后复用同一个 `System`，保留进程 CPU 采样状态。

### 错误分类与排查

| `MetricError` | 常见原因与处理边界 |
| --- | --- |
| `InvalidConfiguration` / `Initialization` | 启动配置或 exporter 初始化失败；产品映射为启动错误 |
| `InvalidDescriptor` / `DuplicateRegistration` / `WrongKind` | schema 或 typed handle 使用错误；启动/测试时修正 |
| `InstrumentLimit` / `SeriesLimit` / `InvalidLabels` | 预算或标签设计错误；检查有限分类及系列组合 |
| `InvalidValue` | 非有限/负 histogram、无效 gauge、counter 累计溢出 |
| `Unavailable` | 当前实例没有所需 exporter，例如 OTLP-only 被用于 pull |
| `Busy` / `Timeout` | gather 或生命周期额度占满、等待超时；避免无限重试 |
| `Export` | 编码、大小或 reader 导出失败；检查预算及本地诊断 |
| `Closed` / `Shutdown` | 关闭后记录或 SDK 关闭失败；检查 producer 退出顺序 |

热路径 convenience API 有意丢弃该次观测错误，不应让指标失败改变业务事务结果。要求检查数值、预算或关闭合同的测试与控制边界使用 `try_*`。库不决定 exporter 故障是否降低 readiness，产品应明确该策略。

## API 迁移

| 旧接口 | 替换 |
| --- | --- |
| 全局 `init_configured_or_noop` / `prometheus::init_metrics` | `MetricsProvider::builder()` + 显式 reader + `build()` |
| `PrometheusMetricsRecorder` | `InfrastructureMetricsRecorder::new(provider)` |
| `ProductMetrics::register()` | `ProductMetrics::register(&provider)` |
| `prometheus::Product*` handles/errors | crate 根的 `ProductCounter/Gauge/Histogram`、`MetricError` |
| 全局 `export_metrics` | 注入 provider 的 exposition resource |
| 全局 health report | 注入的 recorder `record_health` |

旧全局实现直接删除，不保留平行 registry 或双写。AsterDrive 已合并实例级消费迁移；准确 consumer/provider revision、历史验证范围和未核验的发行状态见[消费者证据索引](../guide/reference-projects.md#观测消费者证据索引)。其他外部产品需要按本表升级，不能将 Forge 库内通过当作已经接入所有消费者。

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

当前消费版本与后续 traces 的边界统一见[消费者证据索引](../guide/reference-projects.md#观测消费者证据索引)。

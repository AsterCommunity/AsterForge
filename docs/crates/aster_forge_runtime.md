# aster_forge_runtime

`aster_forge_runtime` 提供 Aster 服务共用的运行时基础能力。它不是业务框架，也不替产品接管 `AppState`、启动模式、审计事件、后台任务顺序、数据库句柄或具体健康探针。这个 crate 只负责可复用的运行时机械层。

## 适用场景

- 统一组件健康报告模型。
- 用注册表并发执行健康检查，并处理 scope、超时和框架级失败。
- 用 Actix 风格的轻量 registry 注册健康检查。
- 在需要组件目录、依赖关系或 shutdown phase 时，用 component registry 描述运行时子系统。
- 记录 startup phase 的执行结果、耗时和失败策略。
- 创建短命 runtime 临时目录。
- 收集 shutdown phase 的执行顺序、耗时和错误。
- 等待 `SIGINT` / `SIGTERM` / `Ctrl+C`。
- 为产品入口提供 signal-to-cancel 和 shutdown report logging 这类小型 lifecycle helper。
- 用 `ServiceLifecycle` 收掉多 Actix 产品重复的 signal、server await 和 after-stop 收尾流程。
- 为 runtime 副作用队列提供通用的内存缓冲、批量 flush、延迟 flush 和溢出直写机制。
- 用 runtime lease supervisor 管理多实例 singleton worker group。

不适合放在这里的内容：

- 产品 runtime state。
- 产品配置 key。
- 审计动作和审计实体。
- 产品资源初始化顺序。
- 具体数据库、缓存、邮件或存储的业务检查。
- 产品 shutdown 顺序里的业务副作用。
- 产品审计、邮件、任务等具体实体和 repository 写入语义。

## Runtime Lease

模块：`aster_forge_runtime::lease`

Runtime lease 解决的是“多个服务实例里，哪个实例可以启动某一组 singleton 后台循环”。它不是任务 processing lease，也不是任务去重键：

```text
runtime_leases        -> 哪个进程拥有某个 runtime worker group
background_tasks      -> 具体有哪些任务和任务执行结果
task processing lease -> 哪个 worker 正在执行某一条任务
task dedupe key       -> 同一个计划触发点只能创建一条任务
```

典型 singleton group 包括 background task dispatcher、mail outbox dispatch、system health refresh，以及 session、token、audit、task artifact cleanup 等周期维护任务。每个实例都可以启动 supervisor，但只有拿到 lease 的实例会启动 worker group；其他实例 standby 并按间隔重试。

```rust
let lease_config = aster_forge_runtime::RuntimeLeaseConfig::new(
    "aster_yggdrasil.background_tasks",
    aster_forge_runtime::new_runtime_lease_owner_id(),
)
.ttl(std::time::Duration::from_secs(30))
.renew_interval(std::time::Duration::from_secs(10))
.standby_retry_interval(std::time::Duration::from_secs(5));

aster_forge_runtime::run_runtime_lease_supervisor(
    lease_store,
    lease_config,
    shutdown_token,
    |leased_shutdown| spawn_singleton_background_tasks(leased_shutdown),
    |tasks| async move {
        tasks.shutdown().await;
    },
)
.await;
```

`RuntimeLeaseStore` 是存储抽象。数据库产品优先使用 `aster_forge_db::RuntimeLeaseDbStore` 和 `aster_forge_db::create_runtime_leases_table(...)`；如果未来某个产品需要 etcd、Redis 或其他协调后端，只需要实现同一个 trait。

## Runtime Component Registry

模块：`aster_forge_runtime::component`

主要类型：

- `RuntimeComponentRegistry`
- `RuntimeComponentBuilder`
- `RuntimeComponentBundle`
- `RuntimeComponentDescriptor`
- `RuntimeComponentGraphError`
- `RuntimeComponentKind`
- `RuntimeShutdownDescriptor`

`RuntimeComponentRegistry` 是组件元数据和 lifecycle hook 的组合注册表。它的目标不是接管产品 `AppState`，也不是替产品创建数据库、缓存、存储或邮件资源。它只让子系统用统一方式声明：

- 组件名称和类型。
- 组件依赖。
- 组件拥有的 health checks。
- 组件拥有的 shutdown phase。

它可以作为产品 runtime 的统一组件目录。只需要裸健康检查、不关心组件依赖或 shutdown 绑定时，仍然可以直接用下面的 `HealthCheckRegistry`；但对于 Aster 产品主服务，推荐用 `RuntimeComponentRegistry` 把 health、shutdown 和组件 descriptor 绑在一起，避免同一组组件在多处重复注册。

推荐接入方式和 Actix Web 的 route registration 一样：子系统暴露一个 `*_component(...)` 工厂，调用方只负责把 bundle 交给 `.component(...)` 或 `RuntimeComponentRegistry::register_bundle(...)`。不要让产品入口直接调用低层 registry 注册函数。

```rust
use aster_forge_runtime::{
    RuntimeComponentBundle, RuntimeComponentBundleRegistration,
};

pub fn core_health_component(
    state: &AppState,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle> {
    aster_forge_runtime::runtime_component((
        aster_forge_db::database_health_component(state.reader_db().clone()),
        aster_forge_cache::cache_health_component(
            state.config().cache.clone(),
            state.cache().clone(),
        ),
    ))
}
```

产品入口统一组合这些子系统：

```rust
let mut registry = RuntimeComponentRegistry::new();
registry
    .register_bundle(health_service::core_health_component(state))
    .register_bundle(storage_runtime::storage_health_component(state));

let diagnostics = registry
    .run_health(aster_forge_runtime::HealthCheckScope::Diagnostics)
    .await;
```

如果子系统拥有 shutdown-only 资源，例如 outbox drain handle、audit flush guard 或产品自定义 manager，优先使用 `shutdown_resource_component_after()`。产品仍然提供资源和 shutdown closure，Forge 负责组件 boilerplate、依赖声明和 once-only 资源消费：

```rust
let mail_outbox = aster_forge_runtime::shutdown_resource_component_after(
    "mail_outbox",
    RuntimeComponentKind::Mail,
    "mail_outbox_drain",
    &[aster_forge_tasks::BACKGROUND_TASKS_COMPONENT],
    resources,
    |resources| async move {
        drain_mail_outbox(resources).await.map_err(|error| error.to_string())
    },
);
```

更专门的公共 crate 可以在此基础上继续提供领域组件，例如：

- `aster_forge_tasks::background_task_component_with_definitions_from_shutdown(...)`
- `aster_forge_db::database_component_after(...)`

entrypoint 里可以直接注册这些 component：

```rust
AsterRuntime::builder()
    .component(http_component)
    .component(aster_forge_tasks::background_task_component_with_definitions_from_shutdown(
        registered_runtime_tasks(),
        |shutdown_token| spawn_runtime_background_tasks(state.clone(), shutdown_token),
    ))
    .component(mail_outbox)
    .component(aster_forge_db::database_component_after(...));
```

如果入口要组合多个资源包，可以用 tuple bundle：

```rust
let report = RuntimeComponentRegistry::shutdown_bundle((
    ShutdownComponents { background_tasks, db_handles },
    ProductAuditComponents,
)).await;
```

shutdown 也走同一个注册模型。普通产品代码优先使用领域 crate 暴露的 component factory；如果 phase 需要消费数据库句柄、后台任务集合这类只能关闭一次的资源，由领域 component 内部使用 `component_shutdown_once()` 处理 `Option<T>::take()` 这类机械层。数据库 component 例子：

```rust
pub fn database_component(
    db_handles: DbHandles,
) -> RuntimeComponentBundleRegistration<aster_forge_db::DatabaseRuntimeComponent> {
    aster_forge_db::database_component_after(
        db_handles,
        &[
            aster_forge_tasks::BACKGROUND_TASKS_COMPONENT,
            aster_forge_audit::AUDIT_LOGS_COMPONENT,
        ],
    )
}
```

`RuntimeComponentRegistry::shutdown()` 会按 component dependency graph 执行 shutdown phase，而不是按注册顺序执行。依赖组件会先于依赖它的组件执行；没有 shutdown phase 的依赖只作为 descriptor metadata，不会阻塞低层 registry shutdown。`AsterRuntime::builder().build()` 会先调用 `RuntimeComponentRegistry::validate()`，因此产品入口里的缺失依赖和依赖环会作为 `AsterRuntimeError::ComponentGraph` 失败，而不是带着错误图进入生产。

```text
background_tasks
mail_outbox    -> depends_on background_tasks
audit_logs     -> depends_on mail_outbox
audit_manager  -> depends_on audit_logs
database       -> depends_on background_tasks, mail_outbox, audit_manager
```

上面的图即使按 `database, audit_manager, audit_logs, mail_outbox, background_tasks` 的顺序注册，shutdown 仍会按 `background_tasks -> mail_outbox -> audit_logs -> audit_manager -> database` 执行。产品侧不应该再把注册顺序当成 shutdown 正确性的来源。

如果不需要链式添加依赖，可以直接使用 registry shortcut：

```rust
registry.component_shutdown_once(
    "background_tasks",
    RuntimeComponentKind::Tasks,
    "background_tasks",
    None,
    background_tasks,
    |background_tasks| async move {
        background_tasks.shutdown().await;
        Ok(())
    },
);
```

这些 shortcut 是给 shared crate 实现 component 时使用的低层 API。产品侧优先调用 `aster_forge_db::database_component_after(...)`、`aster_forge_tasks::background_task_component_with_definitions_from_shutdown(...)` 这类领域 component factory，不要在入口里重复写 shutdown 注册逻辑，也不要为了拿 runtime shutdown token 自己实现一层 `AsterRuntimeComponent`。

边界约定：

- Forge 负责 registry、descriptor、health/shutdown 机械层。
- 产品侧负责资源创建、`AppState` 组合、业务 startup 顺序、审计和 API 响应格式。
- 子系统不要自己创建根 registry；只暴露 `*_component(...)` 工厂。
- 产品入口尽量只声明 `.component(...)` 列表，不直接操作根 registry。
- 如果健康检查已经是产品 runtime component 的一部分，优先走 `RuntimeComponentRegistry`，这样 admin 组件目录、readiness、diagnostics 和 shutdown descriptor 使用同一份注册来源。
- `RuntimeComponentRegistry::validate()` 适合在产品自己的启动测试里单独调用；真实 `AsterRuntime` build 会自动校验。

## Cargo

```toml
[dependencies]
aster_forge_runtime = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## AsterRuntime

模块：`aster_forge_runtime::lifecycle`

主要类型：

- `AsterRuntime`
- `AsterRuntimeBuilder`
- `RuntimeServiceComponent`
- `runtime_component`

`AsterRuntime` 是产品 entrypoint 使用的组合入口。它不创建 HTTP server、不拥有 `AppState`、不决定业务资源怎么初始化；它只把这些产品已经创建好的东西按统一 lifecycle 串起来：

- service component；
- termination signal 后的 service stop hook；
- runtime component registration；
- component graph validation；
- dependency-aware component shutdown；
- shutdown report logging。

推荐的产品入口形态：

```rust
AsterRuntime::builder()
    .component(http_component(http_config, state.clone()))?
    .component(background_tasks_component(state.clone()))
    .component(mail_runtime_component(state.get_ref()))
    .component(audit_runtime_component(state.get_ref()))
    .component(database_component(state.get_ref().db_handles.clone()))
    .run()
    .await?;
```

`AsterRuntimeBuilder` 持有一个共享 shutdown token。HTTP server、background tasks、config reload subscription 等需要同一个 token 的子系统应该在自己的 component 构造函数里使用 `runtime_component_with_shutdown(...)` 或 `try_runtime_component_with_shutdown(...)`，然后让产品入口只写普通 `.component(...)`。产品入口不要再手工创建和传递 root `CancellationToken`。

HTTP 组件通常是很薄的产品函数：产品负责创建 Actix `Server`，Forge 只接收 service future、共享 shutdown token 和 stop hook。

```rust
fn http_component(
    config: HttpRuntimeConfig,
    state: actix_web::web::Data<AppState>,
) -> TryRuntimeComponentWithShutdown<
    RuntimeServiceComponent<actix_web::dev::Server>,
    impl FnOnce(tokio_util::sync::CancellationToken) -> std::io::Result<RuntimeServiceComponent<actix_web::dev::Server>>,
    std::io::Error,
> {
    try_runtime_component_with_shutdown(move |shutdown_token| {
        build_http_component(config, state, shutdown_token)
    })
}

fn build_http_component(
    config: HttpRuntimeConfig,
    state: actix_web::web::Data<AppState>,
    shutdown_token: tokio_util::sync::CancellationToken,
) -> std::io::Result<RuntimeServiceComponent<actix_web::dev::Server>> {
    let shutdown_data = actix_web::web::Data::new(shutdown_token.clone());
    let server = build_actix_server(config, state, shutdown_data)?;
    let handle = server.handle();
    Ok(RuntimeServiceComponent::new(
        "http",
        RuntimeComponentKind::Core,
        server,
        shutdown_token,
        move || async move {
            handle.stop(true).await;
        },
    ))
}
```

普通组件应该由子系统暴露成构造函数，内部用 `runtime_component(...)` 适配 `RuntimeComponentBundle`。调用方仍然只看到 `.component(...)`，不用自己接触 root registry：

```rust
pub fn database_component(db_handles: DbHandles) -> RuntimeComponentBundleRegistration<DatabaseComponents> {
    runtime_component(DatabaseComponents::new(db_handles))
}
```

如果产品有多个带资源的子系统，建议让各自业务模块暴露 domain component factory。入口可以按领域列出组件，但不要在入口里手工注册 shutdown phase 或直接操作 root registry：

```rust
pub fn mail_runtime_component(
    state: &AppState,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle> {
    mail_outbox_component(MailOutboxRuntimeResources::from_state(state))
}

pub fn audit_runtime_component(
    state: &AppState,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle> {
    audit_component(AuditRuntimeResources::from_state(state))
}
```

这样入口只表达“HTTP + tasks + mail + audit + database”，资源拆解留在各自业务模块里。需要持有整个 Actix state 的组件可以 clone `web::Data<AppState>`；只需要 database/runtime config/sender 的组件应该从 `&AppState` 抽最小资源，避免为方便而多包一层 `Arc` 或 clone 整个 state。

这就是 Forge runtime 的目标形态：产品入口只声明“有哪些组件”，组件自己把 health、startup、task、shutdown 和 descriptor 注册进 Forge registry。记录 server shutdown audit、drain mail outbox、flush audit manager、close database 这类生命周期动作应该属于各自 component，不应该散落在入口的裸 `before_shutdown` hook 里。Forge 不替产品创建资源，也不把业务 shutdown 动作藏在框架默认值里。

## 健康检查

模块：`aster_forge_runtime::health`

主要类型：

- `HealthStatus`
- `HealthComponentReport`
- `HealthComponentDetail`
- `SystemHealthReport`
- `HealthCheckDescriptor`
- `HealthCheckOptions`
- `HealthCheckRequirement`
- `HealthCheckScope`
- `HealthCheckScopes`
- `HealthCheckRegistry`
- `HealthCheckRegistryBuilder`
- `HealthMetricsRecorder`

### 报告模型

Forge 只提供健康报告模型和聚合规则：

- `Healthy`、`Degraded`、`Unhealthy` 三种状态。
- 通过 `HealthStatus::as_str()` 提供稳定的 wire value。
- `HealthComponentReport::healthy()` / `degraded()` / `unhealthy()` 构造单个组件报告。
- `HealthComponentReport::with_duration()` 可以附加产品侧已经测得的耗时；如果未设置，registry 会填入实际执行耗时。
- `HealthComponentReport::with_detail()` 可以附加 typed 结构化诊断信息，例如 cache backend、storage driver、queue depth、latency。
- `HealthComponentReport::detail()` 可以按 key 读取 typed 诊断值。
- `HealthComponentReport::duration_seconds()` 把组件耗时转成 metrics 友好的秒数。
- `SystemHealthReport::status()` 返回最差状态。
- `SystemHealthReport::has_issues()` 判断是否有问题。
- `SystemHealthReport::duration_seconds()` 把整体耗时转成 metrics 友好的秒数。
- `SystemHealthReport::summary()` 生成简洁的运维摘要。
- `SystemHealthReport::details()` 生成所有组件的诊断明细。
- `SystemHealthReport::issue_summary()` 只汇总 degraded/unhealthy 组件。
- `SystemHealthReport::issue_details()` 只输出 degraded/unhealthy 组件明细。
- `SystemHealthReport::record_metrics()` 通过产品提供的 `HealthMetricsRecorder` 桥接 metrics backend。

### 注册式运行器

`HealthCheckRegistry` 负责：

- 并发执行健康检查。
- 按注册顺序返回组件报告。
- 按 `HealthCheckScope` 选择 liveness、readiness 或 diagnostics 检查。
- 对每个检查应用独立超时。
- 将超时和 panic 映射成组件报告。
- 聚合成 `SystemHealthReport`。
- 通过 `descriptors()` / `descriptors_for_scope()` 暴露注册信息，方便 admin UI、诊断 API 或 metrics catalog 展示。

推荐接入方式同样是 component 风格：共享子系统优先提供 `*_health_component(...)` 工厂，产品侧把这些 bundle 注册到 `RuntimeComponentRegistry`。低层 registry 注册函数是领域 crate 的内部实现细节，不作为子系统 API 暴露。

```rust
let mut registry = aster_forge_runtime::RuntimeComponentRegistry::new();
registry
    .register_bundle(aster_forge_db::database_health_component(reader_db.clone()))
    .register_bundle(aster_forge_cache::cache_health_component(
        cache_config.clone(),
        cache.clone(),
    ));

let report = registry.run_health(HealthCheckScope::Diagnostics).await;
```

产品侧也可以继续定义自己的聚合 component，例如 `core_health_component(state)`，在内部组合数据库、缓存和产品特有检查。这样入口代码看到的是稳定 component，而不是分散的手写注册函数。

`HealthCheckRequirement` 用来区分框架级失败语义：

- `Required`：超时或 panic 视为 `Unhealthy`。
- `Optional`：超时或 panic 视为 `Degraded`。

`HealthCheckOptions` 是推荐的新注册入口：

- `HealthCheckOptions::required(timeout)`：关键组件，失败影响 readiness 或整体健康。
- `HealthCheckOptions::optional(timeout)`：可选组件，失败通常表示降级。
- `.with_scopes(HealthCheckScopes::readiness_and_diagnostics())`：声明检查在哪些视图里执行。

如果多个检查共享 timeout 或 scope，可以用 `HealthCheckRegistryBuilder`：

```rust
use aster_forge_runtime::{
    HealthCheckRegistryBuilder, HealthCheckScopes, HealthComponentReport,
};

let mut builder = HealthCheckRegistryBuilder::new()
    .default_timeout(Some(std::time::Duration::from_secs(5)))
    .default_scopes(HealthCheckScopes::diagnostics());

builder.register_required("database", || async {
    HealthComponentReport::healthy("database", "database ping succeeded")
});

let registry = builder.build();
```

常见 scope 约定：

- `Liveness`：只判断进程是否活着，通常不做数据库、缓存、外部服务探测。
- `Readiness`：判断实例能否接收流量，例如 database ping。
- `Diagnostics`：完整运维诊断，例如 database、cache、storage、mail、tasks。

产品 crate 仍然自己决定要检查哪些组件。比如服务在产品侧注册 database 和 cache 探针，`/readyz` 执行 `Readiness` scope，runtime health task 执行 `Diagnostics` scope，然后把返回值交给 Forge 的报告模型。

示例：

```rust
use std::time::Duration;

use aster_forge_runtime::{
    HealthCheckOptions, HealthCheckRegistry, HealthCheckScope, HealthCheckScopes,
    HealthComponentReport,
};

fn configure_health_checks(registry: &mut HealthCheckRegistry) {
    registry.register_with_options(
        "database",
        HealthCheckOptions::required(Some(Duration::from_secs(5)))
            .with_scopes(HealthCheckScopes::readiness_and_diagnostics()),
        || async { HealthComponentReport::healthy("database", "database ping succeeded") },
    );
}

let registry = HealthCheckRegistry::configured(configure_health_checks);

let report = registry.run_scope(HealthCheckScope::Diagnostics).await;
assert_eq!(report.summary(), "database healthy");
```

产品侧把 health report 映射到 task outcome、HTTP response 或 admin DTO 时，优先使用这些投影 helper。比如 Drive 和 Yggdrasil 的 runtime health task 都可以用 `issue_summary()` / `issue_details()` 把异常组件写进失败记录，同时保留产品自己的 task result 类型。结构化 component details 应该继续透传到产品 task/admin DTO，避免 cache backend、storage driver、queue depth 这类诊断值只停留在日志里。

### Typed Detail Schema

`HealthComponentDetail` 是 Forge 提供的公共 schema，产品侧不要再复制一套 detail value 类型。推荐做法是产品 DTO 直接使用 `Vec<aster_forge_runtime::HealthComponentDetail>`，只在产品侧保留自己的 component/status 外壳。

当前支持的 `HealthComponentDetailValue`：

- `Text(String)`：backend、driver、region、mode 等文本值。
- `Integer(i64)`：允许负数的计数或偏移。
- `Unsigned(u64)`：queue depth、pending count、retry count 等非负数。
- `Boolean(bool)`：开关、fallback active 等布尔状态。
- `DurationMillis(u64)`：latency、lag、age、timeout 等耗时，单位固定为毫秒。

序列化后的 JSON 形状稳定，方便 admin UI 按类型展示：

```json
[
  { "key": "backend", "value": { "type": "text", "value": "redis" } },
  { "key": "queue_depth", "value": { "type": "unsigned", "value": 12 } },
  { "key": "latency", "value": { "type": "duration_millis", "value": 42 } }
]
```

产品侧如果需要展示文本，可以用 `HealthComponentDetailValue::display_value()`。如果需要按类型渲染，应直接 match `HealthComponentDetailValue`，不要解析字符串。

### Metrics Bridge

Forge 不直接依赖具体 metrics backend。产品侧可以给自己的 recorder 或 adapter 实现 `HealthMetricsRecorder`：

```rust
use aster_forge_runtime::{
    HealthComponentReport, HealthMetricsRecorder, HealthStatus, SystemHealthReport,
};

struct ProductHealthMetrics<'a>(&'a ProductMetrics);

impl HealthMetricsRecorder for ProductHealthMetrics<'_> {
    fn record_health_report(
        &self,
        scope: &'static str,
        status: HealthStatus,
        duration_seconds: f64,
    ) {
        self.0.record_health_report(scope, status.as_str(), duration_seconds);
    }

    fn record_health_component(
        &self,
        scope: &'static str,
        component: &HealthComponentReport,
        duration_seconds: f64,
    ) {
        self.0.record_health_component(
            scope,
            component.name,
            component.status.as_str(),
            duration_seconds,
        );
    }
}

let report = SystemHealthReport::new(Vec::new());
report.record_metrics("diagnostics", &ProductHealthMetrics(metrics));
```

这样 Forge 定义稳定的 health report/metrics 语义，并通过 `aster_forge_metrics` 提供可选 backend。产品仓库只负责在入口选择 metrics feature、挂载观测 endpoint，并保持业务 label 低基数。

Yggdrasil 的接入方式是让 `PrometheusMetricsRecorder` 同时实现 `aster_forge_runtime::HealthMetricsRecorder`，并在 health service 完成 `Readiness` 或 `Diagnostics` scope 后调用 `SystemHealthReport::record_metrics()`。Prometheus 指标保持低基数：

- `health_report_status{scope}`：aggregate status，`healthy=0`、`degraded=1`、`unhealthy=2`。
- `health_report_duration_seconds{scope,status}`：aggregate health run duration。
- `health_component_status{scope,component}`：component status，`healthy=0`、`degraded=1`、`unhealthy=2`。
- `health_component_duration_seconds{scope,component,status}`：component check duration。

## Buffered Batch Writer

模块：`aster_forge_runtime::buffered`

主要类型：

- `BufferedBatchConfig`
- `BufferedBatchWriter<T>`

`BufferedBatchWriter<T>` 抽的是 Aster 服务里反复出现的一类 runtime 机械层：调用点需要快速接收一条记录，但真实写入可以合并成批量操作；如果迟迟凑不满批次，也要在短延迟后 flush；如果队列满了，则不能无限堆内存，而是把新记录走一次直写并触发后台 flush。

它适合承载这些公共规则：

- 内存队列上限。
- 达到批量大小后立即后台 flush。
- 首条记录进入空队列后安排延迟 flush。
- 队列溢出时对当前记录执行单条写入。
- shutdown 前取消延迟任务，并由产品侧显式 flush 剩余记录。

它不承载这些业务语义：

- 审计日志、邮件 outbox、任务记录等具体实体。
- repository、事务、错误类型和重试策略。
- 是否应该记录某个审计动作。
- shutdown 阶段应该在哪一步 flush。
- 指标名称、审计动作名称和产品日志格式。

示例：

```rust
use std::sync::Arc;
use std::time::Duration;

use aster_forge_runtime::{BufferedBatchConfig, BufferedBatchWriter};

let db_for_batch = db.clone();
let db_for_single = db.clone();

let writer = Arc::new(BufferedBatchWriter::new(
    BufferedBatchConfig::new(4096, 100, Duration::from_secs(1), "audit_log"),
    move |items: Vec<audit_log::ActiveModel>| {
        let db = db_for_batch.clone();
        async move {
            if let Err(error) = audit_log_repo::create_many(&db, items).await {
                tracing::warn!("failed to write audit log batch: {error}");
            }
        }
    },
    move |item: audit_log::ActiveModel| {
        let db = db_for_single.clone();
        async move {
            if let Err(error) = audit_log_repo::create(&db, item).await {
                tracing::warn!("failed to write audit log: {error}");
            }
        }
    },
));

writer.record(model).await;
```

shutdown 时产品侧应该先取消延迟 flush，再手动 flush：

```rust
writer.cancel();
writer.flush().await;
```

这个顺序可以避免延迟任务在 shutdown 过程中继续被唤醒，同时保留“进程退出前尽量写完内存队列”的行为。Yggdrasil 的 audit manager 采用的就是这个边界：Forge 负责队列调度和批量切片，Yggdrasil 负责 audit model 构造、database repository、错误日志和是否记录某个动作。

## Startup

模块：`aster_forge_runtime::startup`

主要类型和函数：

- `run_required_startup_phase()`
- `run_optional_startup_phase()`
- `StartupCoordinator`
- `StartupReport`
- `StartupPhaseOutcome`
- `StartupPhaseReport`
- `StartupPhaseStatus`
- `StartupPhaseFailurePolicy`
- `RuntimeTempDirError`
- `ensure_runtime_temp_dir()`
- `create_runtime_temp_dir_guard()`

### 单阶段 helper

`run_required_startup_phase()` 适合包裹会返回产品资源的初始化步骤，例如：

- 数据库连接和 reader/writer handle。
- runtime config snapshot。
- cache backend。
- storage driver registry。
- 最终 `AppState`。

Forge 会记录 phase 名称、耗时和日志，但保留产品错误类型，不把错误强行转换成 Forge 自己的错误。

```rust
use aster_forge_runtime::run_required_startup_phase;

let prepared = run_required_startup_phase("prepare_runtime_state", || async {
    product_runtime::prepare_state().await
})
.await?;

let state = prepared.value;
```

`run_optional_startup_phase()` 适合“失败不阻断启动”的步骤，例如可选 metrics exporter、可选 bootstrap 或非关键 warming。失败会记录成 `StartupPhaseStatus::SkippedAfterFailure`，调用方可以把 report 放进启动诊断里。

### Runtime 临时目录

`ensure_runtime_temp_dir()` 用 Forge 的 `_runtime` 临时目录布局创建启动时需要的短命运行时目录：

```rust
let runtime_dir = aster_forge_runtime::ensure_runtime_temp_dir(&config.server.temp_dir)
    .await
    .map_err(|error| {
        AsterError::config_error(format!("failed to create runtime temp dir: {error}"))
    })?;
```

Forge 只负责目录布局和 IO 操作。清理时机、错误映射、是否在启动前或 shutdown 后清理，仍由产品侧决定。

如果是单次操作需要临时目录，可以用 `create_runtime_temp_dir_guard()` 创建 scope-local 目录：

```rust
let temp_dir = aster_forge_runtime::create_runtime_temp_dir_guard(
    &config.server.temp_dir,
    "thumbnail",
    "thumbnail render temp dir",
)
.await
.map_err(|error| AsterError::config_error(error.to_string()))?;

let work_dir = temp_dir.path();
```

这个 helper 创建的是 `temp_root/_runtime/<scope>/<random-token>`，并返回
`aster_forge_utils::raii::TempDirGuard`。guard drop 时只清理这一层操作目录，不会删除共享的
`_runtime` 根目录。`scope` 只允许非空 ASCII 字母、数字、`-` 和 `_`，避免公共 API 被路径片段穿透。

### StartupCoordinator

`StartupCoordinator` 适合一组纯副作用 phase：

- 必须成功的 phase 用 `required()`。
- 失败后继续启动的 phase 用 `optional()`。
- required phase 失败后停止后续 phase。
- optional phase 失败会记录 report，但继续执行后续 phase。

它不适合直接承载会产出资源的初始化链。资源初始化优先用 `run_required_startup_phase()`，这样产品侧不需要为了框架化而把资源塞进 `Arc<Mutex<Option<T>>>` 之类的容器。

产品 crate 仍然自己决定实际 startup 动作：

- migration。
- system config seed。
- runtime config reload。
- cache/storage/mail 初始化。
- audit manager 初始化。
- 产品专属启动状态准备。

## Shutdown

模块：`aster_forge_runtime::shutdown`

主要类型和函数：

- `wait_for_termination_signal()`
- `spawn_termination_signal_handler()`
- `TerminationSignal`
- `RuntimeSignalError`
- `ShutdownCoordinator`
- `ShutdownReport`
- `ShutdownPhaseReport`
- `ShutdownPhaseStatus`
- `log_shutdown_report()`

### 终止信号

`wait_for_termination_signal()` 负责等待进程终止信号：

- Unix `SIGINT`
- Unix `SIGTERM`
- 非 Unix 平台上的 `Ctrl+C`

它只做信号等待和共享日志，不替产品决定 shutdown 顺序。

`spawn_termination_signal_handler()` 是低层 helper：它等待终止信号，取消产品传入的 `CancellationToken`，然后执行产品提供的 stop 回调。Forge 不依赖 Actix，也不直接停止 HTTP server；Yggdrasil 这类产品仍然把 `server.handle().stop(true)` 作为回调传进去。

已经采用 `AsterRuntime::builder()` 的产品不应该在入口里直接调用这个 helper；由 `AsterRuntime` 统一安装 signal handler。还没迁移到 component 模式的应用才需要这样手写：

```rust
let shutdown_token = tokio_util::sync::CancellationToken::new();
let server = build_product_server(shutdown_token.clone())?;
let handle = server.handle();

let _signal_task = aster_forge_runtime::spawn_termination_signal_handler(
    shutdown_token,
    move || async move {
        handle.stop(true).await;
    },
);

let server_result = server.await;
product_shutdown().await;
server_result
```

### ServiceLifecycle

`ServiceLifecycle` 是比 Actix server builder 更低一层的中间态抽象。它不依赖 Actix，也不创建 `App`、`HttpServer` 或产品 `AppState`。它只负责多个 Actix 产品已经重复的入口流程：

- 启动 termination signal handler。
- 收到信号后取消共享 `CancellationToken`。
- 执行产品传入的 stop callback，例如 `server.handle().stop(true)`。
- 等待产品 server future 结束。
- 执行产品传入的 after-stop callback，例如产品专属审计、停止后台任务、关闭 DB。
- 返回原始 server future 的结果。

推荐产品侧继续保留自己的 `runtime::entrypoint::run()`，并在其中构建 Actix app：

```rust
let shutdown_token = tokio_util::sync::CancellationToken::new();
let server = build_product_server(shutdown_token.clone())?;
let handle = server.handle();

aster_forge_runtime::ServiceLifecycle::new(server, shutdown_token)
    .run(
        move || async move {
            handle.stop(true).await;
        },
        move || async move {
            record_product_shutdown_event().await;
            perform_product_shutdown().await;
        },
    )
    .await
```

这条边界适合还没有切到 `AsterRuntime` component 模式的产品入口：产品可以先复用 signal、server await 和 after-stop 机械层，再逐步把 background tasks、mail outbox、audit manager、database close 这些生命周期资源收进 component graph。已经采用 `AsterRuntime::builder()` 的入口应优先把 shutdown 动作放到 component 里，而不是继续堆叠裸 after-stop hook。

### Shutdown 协调器

`ShutdownCoordinator` 负责：

- 按注册顺序执行 shutdown phase。
- 记录每个 phase 的耗时。
- 对 phase 错误做聚合。
- 支持独立超时。
- 支持一次性资源句柄通过 `FnMut` 进入 phase。

`ShutdownPhaseStatus` 包含：

- `Succeeded`
- `Failed(String)`
- `TimedOut`

`ShutdownReport` 会保留 phase 执行顺序，并提供 `has_failures()` 方便上层日志判断。
如果产品只需要统一记录最终结果，可以直接调用 `log_shutdown_report(&report)`。

注意：`ShutdownCoordinator` 是低层顺序执行器，不解析 component dependency。需要组件目录、依赖校验和 dependency-aware shutdown 时，使用 `RuntimeComponentRegistry` 或 `AsterRuntime`。

示例：

```rust
use aster_forge_runtime::ShutdownCoordinator;
use std::time::Duration;

let mut coordinator = ShutdownCoordinator::new();
let mut db_handle = Some("db");

coordinator.phase("database", Some(Duration::from_secs(5)), move || {
    let db_handle = db_handle.take();
    async move {
        if db_handle.is_some() {
            Ok(())
        } else {
            Err("db handle already consumed".to_string())
        }
    }
});
```

产品 crate 仍然自己决定实际 shutdown 动作：

- 记录 server-shutdown 审计事件。
- 停止后台任务。
- flush 日志、邮件或 outbox。
- 关闭数据库、缓存或外部连接。

## 错误边界

`RuntimeSignalError` 只描述安装或等待终止信号失败。
`ShutdownPhaseStatus::Failed(String)` 只保留 phase 自己提供的错误字符串。
产品 crate 应该在边界上把它们映射成自己的错误类型或日志格式。

## 测试

共享测试覆盖：

- 健康状态 wire value。
- 健康报告聚合。
- 健康注册表的顺序执行和超时映射。
- startup phase 的 required/optional 失败策略和有返回值 phase。
- component graph 的缺失依赖、依赖环、dependency-aware shutdown 顺序。
- shutdown coordinator 的顺序执行、超时和一次性句柄消费。
- 终止信号标签。

产品测试仍然应该覆盖自己的 startup 资源初始化、health probe 和 component graph。产品入口如果使用 `AsterRuntime`，建议至少有一个测试覆盖注册出的 component 名称和关键依赖。

## 参考项目

- AsterYggdrasil：使用 Forge 的 health registry、startup phase helper、`AsterRuntime` 和 dependency-aware component shutdown。数据库连接、cache health、mail outbox drain、audit manager flush、background task shutdown 的资源创建和业务语义仍留在产品代码里，但入口只注册 component。

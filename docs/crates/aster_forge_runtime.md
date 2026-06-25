# aster_forge_runtime

`aster_forge_runtime` 提供 Aster 服务共用的运行时基础能力。它不是业务框架，也不替产品接管 `AppState`、启动模式、审计事件、后台任务顺序、数据库句柄或具体健康探针。这个 crate 只负责可复用的运行时机械层。

## 适用场景

- 统一组件健康报告模型。
- 用注册表顺序执行健康检查，并处理超时。
- 记录 startup phase 的执行结果、耗时和失败策略。
- 创建短命 runtime 临时目录。
- 收集 shutdown phase 的执行顺序、耗时和错误。
- 等待 `SIGINT` / `SIGTERM` / `Ctrl+C`。

不适合放在这里的内容：

- 产品 runtime state。
- 产品配置 key。
- 审计动作和审计实体。
- 产品资源初始化顺序。
- 具体数据库、缓存、邮件或存储的业务检查。
- 产品 shutdown 顺序里的业务副作用。

## Cargo

```toml
[dependencies]
aster_forge_runtime = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## 健康检查

模块：`aster_forge_runtime::health`

主要类型：

- `HealthStatus`
- `HealthComponentReport`
- `SystemHealthReport`
- `HealthCheckCriticality`
- `HealthCheckRegistry`

### 报告模型

Forge 只提供健康报告模型和聚合规则：

- `Healthy`、`Degraded`、`Unhealthy` 三种状态。
- 通过 `HealthStatus::as_str()` 提供稳定的 wire value。
- `HealthComponentReport::healthy()` / `degraded()` / `unhealthy()` 构造单个组件报告。
- `SystemHealthReport::status()` 返回最差状态。
- `SystemHealthReport::has_issues()` 判断是否有问题。
- `SystemHealthReport::summary()` 生成简洁的运维摘要。
- `SystemHealthReport::details()` 生成所有组件的诊断明细。
- `SystemHealthReport::issue_summary()` 只汇总 degraded/unhealthy 组件。
- `SystemHealthReport::issue_details()` 只输出 degraded/unhealthy 组件明细。

### 注册式运行器

`HealthCheckRegistry` 负责：

- 按注册顺序执行健康检查。
- 对每个检查应用独立超时。
- 将超时映射成组件报告。
- 聚合成 `SystemHealthReport`。

`HealthCheckCriticality` 用来区分超时语义：

- `Critical`：超时视为 `Unhealthy`。
- `NonCritical`：超时视为 `Degraded`。

产品 crate 仍然自己决定要检查哪些组件。比如 Yggdrasil 会在产品服务里注册 database 和 cache 探针，然后把返回值交给 Forge 的报告模型。

示例：

```rust
use aster_forge_runtime::{HealthCheckCriticality, HealthCheckRegistry, HealthComponentReport};

let mut registry = HealthCheckRegistry::new();
registry.register(
    "database",
    HealthCheckCriticality::Critical,
    None,
    || async { HealthComponentReport::healthy("database", "database ping succeeded") },
);

let report = registry.run().await;
assert_eq!(report.summary(), "database healthy");
```

产品侧把 health report 映射到 task outcome、HTTP response 或 admin DTO 时，优先使用这些投影 helper。比如 Drive 和 Yggdrasil 的 runtime health task 都可以用 `issue_summary()` / `issue_details()` 把异常组件写进失败记录，同时保留产品自己的 task result 类型。

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
- primary/follower mode 分支。

## Shutdown

模块：`aster_forge_runtime::shutdown`

主要类型和函数：

- `wait_for_termination_signal()`
- `TerminationSignal`
- `RuntimeSignalError`
- `ShutdownCoordinator`
- `ShutdownReport`
- `ShutdownPhaseReport`
- `ShutdownPhaseStatus`

### 终止信号

`wait_for_termination_signal()` 负责等待进程终止信号：

- Unix `SIGINT`
- Unix `SIGTERM`
- 非 Unix 平台上的 `Ctrl+C`

它只做信号等待和共享日志，不替产品决定 shutdown 顺序。

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
- shutdown phase 顺序执行、超时和一次性句柄消费。
- 终止信号标签。

产品测试仍然应该覆盖自己的 startup 资源初始化、health probe 和 shutdown 顺序。

## 参考项目

- AsterYggdrasil：使用 Forge 的 health registry、startup phase helper 和 shutdown coordinator，但数据库、cache、audit、primary/follower state 和后台任务收尾仍留在产品代码里。

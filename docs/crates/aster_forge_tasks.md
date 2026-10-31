# aster_forge_tasks

`aster_forge_tasks` 是 Forge 里边界最重的 crate。它拥有后台任务的共享机械部分：step 状态、payload/result 序列化、retry 分类、任务 spec adapter、registry 宏、runtime worker、lease guard、heartbeat、lane claiming、dispatch stats、drain loop 和任务临时目录。

它故意不拥有产品数据库实体、SeaORM repository、task kind enum、业务 task payload/result、runtime config、metrics label 或具体任务实现。

## 适用场景

- 产品已有后台任务表，想复用 lease/heartbeat/dispatch 生命周期。
- 产品想用 typed task spec，避免 payload/result 手写 JSON 分散在多个 service。
- 产品需要按 lane 限制并发。
- 产品需要统一 shutdown、periodic task、dispatch backoff。
- 产品需要 task step 进度状态。
- 产品需要计划任务或入队请求的 dedupe key，避免多实例 leader 切换窗口重复创建任务。
- 产品需要持久化的 scheduled task catalog，由数据库行协调“哪个进程跑这一次触发”。

不适合放在这里的内容：

- `BackgroundTaskKind` 的业务枚举。
- 某个 task 的业务 payload/result。
- 任务 repository 的 SQL。
- 管理端 task API。
- 任务审计。

## Cargo feature

```toml
[dependencies]
aster_forge_tasks = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `openapi`：给 step/status 等类型派生 OpenAPI schema。
- `runtime`：启用 runtime worker、leased background tasks、periodic task 和 scheduled task runner。
- `runtime-component`：启用 `background_task_component(...)`、`background_task_component_with_definitions(...)` 以及从 `AsterRuntime` shutdown token 生成 worker 的 component factory，并自动启用 `runtime`。

## 模块地图

主要 API 分组：

- `consumer`：独立的持久化队列消费、持续补位、状态与两阶段关闭（`runtime` feature）。
- `dispatch`：原子 lane claim 契约、并发执行、dispatch stats、手动 drain。
- `dedupe`：`TaskDedupeKey` 和 `scheduled_task_dedupe_key(...)`。
- `execution`：claimed task 的完整执行生命周期。
- `heartbeat`：lease heartbeat loop。
- `lease`：`TaskLease`、`TaskLeaseGuard`、`TaskExecutionContext`。
- `registry`：`TaskRecord` 和 `task_registry!`。
- `retry`：`TaskRetryClass` 和默认 retry delay。
- `runtime`：periodic task、dispatch worker、`BackgroundTasks`。需要 `runtime` feature。
- `runtime_metadata`：`RuntimeTaskDefinition`、`RegisteredRuntimeTaskKind`、`RuntimeTaskName<K>` 和 `runtime_task_registry!`。
- `schedule`：`ScheduledTaskStore`、`ScheduledPeriodicTask`、`run_scheduled_periodic_task`、`run_scheduled_claim_renewal_loop`。需要 `runtime` feature。
- `spec`：typed task spec、payload/result codec、erased adapter。
- `steps`：task step 状态。
- `temp`：task/runtime 临时目录清理。

## 产品侧最小模型

产品仓库通常需要保留：

- task 数据库实体。
- task kind enum。
- lane enum 和 lane 配置。
- payload/result enum。
- repository 函数。
- metrics labels。
- 管理 API。

然后为 Forge 实现 trait：

- `TaskRecord<Kind>`
- `ClaimableTaskRecord<Kind>`
- `ExecutableTaskRecord<Kind>`
- `TaskClaimStore<Task, Kind, Lane>`
- `TaskHeartbeatStore`
- `ClaimedTaskExecutionStore<Task, Kind>`

## 注册 task spec

推荐用 `task_registry!` 生成 registry：

```rust
aster_forge_tasks::task_registry! {
    pub(super) mod registered {
        state: crate::runtime::AppState;
        task: crate::entities::background_task::Model;
        config: crate::config::RuntimeConfig;
        context: aster_forge_tasks::TaskExecutionContext;
        error: crate::errors::AsterError;
        kind: crate::types::BackgroundTaskKind;
        lane: crate::services::task_service::dispatch::TaskLane;
        payload: crate::services::task_service::types::TaskPayload;
        result: crate::services::task_service::types::TaskResult;
        specs {
            SYSTEM_RUNTIME: super::SystemRuntimeTask => crate::types::BackgroundTaskKind::SystemRuntime,
        }
        lanes {
            crate::services::task_service::dispatch::TaskLane::Fallback => [
                crate::types::BackgroundTaskKind::SystemRuntime,
            ],
        }
    }
}
```

宏只注册映射关系，不替产品定义业务类型。

AsterDrive 已直接使用 Forge typed spec、payload/result codec、erased adapter 和
`task_registry!`。Drive 只保留 `StoredTaskPayload`/`StoredTaskResult` 包装、业务 envelope
以及 presentation 组装，不再维护本地 adapter 或 kind/lane 双份 match。

## 注册 runtime task 元数据

如果产品有一组系统运行时任务，通常会同时维护：

- 存在 payload 里的 wire value；
- 管理端展示名；
- 前端/i18n presentation code；
- 从历史 wire value 解析回 enum 的逻辑。

这类逻辑在 Drive 和 Yggdrasil 都会重复，但任务列表本身是产品语义，所以 Forge 只提供元数据注册宏：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SystemRuntimeTaskKind {
    SystemHealthCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskPresentationCode {
    RuntimeTaskSystemHealthCheck,
}

aster_forge_tasks::runtime_task_registry! {
    mod registered_runtime_tasks {
        kind: SystemRuntimeTaskKind;
        presentation: TaskPresentationCode;
        tasks {
            SystemRuntimeTaskKind::SystemHealthCheck => {
                wire: "system-health-check",
                display: "System health check",
                presentation: TaskPresentationCode::RuntimeTaskSystemHealthCheck,
            },
        }
    }
}

assert_eq!(registered_runtime_tasks::as_str(SystemRuntimeTaskKind::SystemHealthCheck), "system-health-check");
assert_eq!(
    registered_runtime_tasks::from_wire_value("system-health-check"),
    Some(SystemRuntimeTaskKind::SystemHealthCheck)
);
```

宏会生成一致的 lookup 函数，并在测试里检查 wire value、display name、presentation code 和反向解析是否一致。产品仍然自己决定：

- runtime task enum 有哪些 variant；
- 哪些任务被实际调度；
- task payload/result 的 schema；
- task presentation code 的 i18n 含义；
- runtime task 记录写入数据库的策略。

如果 runtime task payload 需要保存“已知任务或历史任务字符串”，可以让产品 enum 实现
`RegisteredRuntimeTaskKind`，然后直接使用 `RuntimeTaskName<SystemRuntimeTaskKind>`：

```rust
impl aster_forge_tasks::RegisteredRuntimeTaskKind for SystemRuntimeTaskKind {
    fn as_str(self) -> &'static str {
        registered_runtime_tasks::as_str(self)
    }

    fn display_name(self) -> &'static str {
        registered_runtime_tasks::display_name(self)
    }

    fn from_wire_value(value: &str) -> Option<Self> {
        registered_runtime_tasks::from_wire_value(value)
    }
}

type RuntimeTaskName = aster_forge_tasks::RuntimeTaskName<SystemRuntimeTaskKind>;

let known = RuntimeTaskName::from(SystemRuntimeTaskKind::SystemHealthCheck);
let legacy = RuntimeTaskName::from("removed-maintenance-task");

assert_eq!(known.as_str(), "system-health-check");
assert_eq!(known.known(), Some(SystemRuntimeTaskKind::SystemHealthCheck));
assert_eq!(legacy.known(), None);
assert_eq!(legacy.display_name(), "removed maintenance task");
```

这样产品侧不需要重复写 serde、display、legacy fallback，但仍然保留自己的 task enum 和
payload/result schema。数据库里已有的旧 wire value 也可以继续反序列化并原样保存。

## Claimed task execution

如果产品已有“先 claim，再执行，再 mark succeeded/retry/failed”的流程，优先接：

- `run_claimed_task_batch_with_store`
- `ClaimedTaskExecutionStore`
- `ClaimedTaskExecutionConfig`

这样可以把 heartbeat、lease lost、shutdown release、retry/permanent failure、failed step 标记这些共享逻辑交给 Forge。

产品 store 负责：

- 调业务 processor。
- 根据产品错误判断 lease lost / timeout / shutdown。
- 写数据库状态。
- 记录 metrics。
- 唤醒 dispatcher。

AsterDrive 常驻消费使用 `TaskConsumer`/`run_task_consumer`，手动 drain 使用 batch primitives。
两者共享 `claim_due_for_lane`、`process_claimed_task`、Forge `TaskLease`/`TaskExecutionContext`。Drive 只保留 SeaORM store、
稳定 API error code 映射、metrics，以及 task context 到产品 storage cancellation 的适配。

## 独立消费者与原子认领（强制迁移）

普通任务消费与调度领导权分开。每个 worker 为每条 lane 启动 `run_task_consumer`，
共享调度组仅负责定时 firing。单机把两者组合进同一个进程，不维护第二套 handler。

```rust
let report = aster_forge_tasks::run_task_consumer(
    product_lane_consumer,
    stop_claiming,
    aster_forge_tasks::TaskConsumerConfig::default(),
    lane_health_status,
).await?;
```

每条 lane 和 scheduler group 必须使用独立的 `TaskRuntimeStatus`。status 属于单个 runner；产品层将多个 runner 和 scheduler 的健康状态聚合为角色级 readiness/liveness。禁止 runner 间、runner 与 scheduler 间共享 status，因为任何一个 runner 停止都会清空该实例的 active/running 状态。

`TaskConsumer` 的 `concurrency()` 是本进程上限；runner 按空槽 claim，单个任务完成即可补位。claim、release 和 handler 分别由 runner 内部的 JoinSet 持有；慢存储调用不会阻塞其他工作或 shutdown deadline。claim 在 shutdown 期间完成时，未启动的 lease 会 fenced release；release 也受最终关闭期限约束。最终期限到达后全部 abort 并 join，再允许关闭 audit/DB；未知结果的 claim 只能等待 lease 到期。runner 被丢弃时也会 abort 自己持有的任务，避免后台继续认领。空队列和 claim 错误保留轮询退避。
不允许先 claim 再等 semaphore，也不等待整批最慢任务。定时轮询保证最终发现，
`notified()` 只优化延迟；`idle_max_interval()` 给出无通知时的轮询上界。

旧 `TaskClaimStore` 的分步查询接口已移除。产品实现唯一的
`claim(TaskClaimRequest) -> Vec<(preclaim_snapshot, new_lease)>`：

1. 在同一事务内锁共享额度行，重新读权威额度，按有效 processing lease 计数。
2. 等锁后用 `request.at(Utc::now())` 刷新时间，限制为本地空槽与剩余额度的较小值。
3. 选择候选并加行锁（支持时 `SKIP LOCKED`），CAS 更新 token、状态和 lease。
4. 返回认领前快照及 token+1 的 lease；事务外不得先选候选或先数额度。

`TaskLaneConfig.limit` 是本进程容量，`cluster_limit` 是产品提供的可选共享额度。
`fast_continue`、旧锁 key 与 `.worker()` 已移除；单 owner worker 显式使用
`.singleton_worker()`。事务 SQL、quota 配置和产品状态仍由产品拥有。

默认 shutdown：立即停止 claim，等待已启动任务 20 秒，再协作取消 5 秒；总预算不得超过24小时。
超时遗弃 future 时不提前释放 lease：blocking I/O 可能仍在退出，后续 owner 要等租约到期。
产品组件外层 grace 应大于内部预算，并在关闭 audit/DB 前等待消费者。
`TaskRuntimeStatus.ready()` 需要初始化、无 claim 错误且未 draining，空队列和健康 standby 都健康；
`is_running()` 只表达本地活性，不把短暂数据库故障变成 liveness 失败。

## 协作式持久化取消

产品定义可取消状态、权限、取消 API 和持久化状态机。Forge 要求实现：

- `ExecutableTaskRecord::cancellation_requested()`：接管时已取消的快照不调用 handler。
- `TaskHeartbeatStore` 返回 `TaskHeartbeatOutcome::{Renewed, Lost, CancellationRequested}`。
- execution store 实现 typed cancellation 分类和同 token 的 `cancel_task_if_requested`。
- handler 在有副作用的边界调用 `ensure_active()`，等待 I/O 时选择 `stop_requested()`；
  它同时被进程关闭、lease 丢失、续约超时和用户取消唤醒。

执行结束、失败/retry CAS 竞争和进程 shutdown 前均检查持久化取消；取消优先，不伪报成功。
取消保留已提交的产品变更，Forge 不提供回滚承诺。外部副作用仍是至少一次语义。
产品需把 publication receipt/checkpoint 与文件、配额、版本或业务变更放在同一 fenced 事务内。
执行尝试只能清理 `cleanup_task_temp_dir_for_lease_in_root` 对应的 token 目录；
整条 task 目录的删除属于终态保留期或显式清理。

组件可 `.named("task_consumers")` / `.named("task_scheduler")` 分别挂载；
元数据用 `.with_definition_filter(...)` 只注册实际运行定义。
请求拥有的本地维护用 `.after_service()`，保持运行直到 HTTP producer 停止，随后才关闭依赖。

## Runtime worker

`BackgroundTasks` 管理 tokio task handles 和 shutdown token。`run_periodic_task`、`run_dispatch_worker` 适合产品 runtime 层接入。

接入时注意：

- `BACKGROUND_TASKS_COMPONENT` 是稳定组件名，默认 API 可用，方便 database、mail、audit 这类 crate 声明 shutdown 依赖而不启用 runtime worker。
- `BackgroundTasks`、`run_dispatch_worker(...)`、`run_leased_background_tasks(...)` 和 scheduled task runner 需要 `runtime` feature。
- `background_task_component(...)`、`background_task_component_with_definitions(...)`、`background_task_component_from_shutdown(...)` 和 `background_task_component_with_definitions_from_shutdown(...)` 需要 `runtime-component` feature。
- 产品启动时保存 `BackgroundTasks`。
- 如果产品已经使用 `aster_forge_runtime::AsterRuntime` component 模式，优先使用 `background_task_component_with_definitions_from_shutdown(...)`，让 Forge 把 runtime shutdown token 传给 worker spawner。只有产品已经提前创建好 `BackgroundTasks` 时，才直接用 `background_task_component()` 或 `background_task_component_with_definitions()`。
- 还没迁移到 component 模式的应用才需要直接调用 `shutdown().await`。
- periodic task 的记录逻辑由产品传入 hook。
- panic outcome 由产品决定如何持久化。
- `periodic_sleep_duration` 对零间隔钳到 1 秒下限（与 dispatch worker 的 `effective_dispatch_base_interval` 一致），防止配置事故让 runner 热 spin、scheduled 版打爆数据库；非零的亚秒间隔不干预（快速测试、内存轮询是合法用法）。
- `BackgroundTasks::shutdown` 会 drain 所有 worker 句柄：panic 退出的 worker 逐条记 `tracing::error!`（panic hook 之外补一层任务组上下文），被 `abort_all` 取消的 worker 属于预期路径不告警。

多实例服务如果有一组 process-level singleton worker，优先使用
`run_leased_background_tasks(...)`，不要在产品侧重复手写
`run_runtime_lease_supervisor(..., |tasks| tasks.shutdown())` 胶水：

```rust
aster_forge_tasks::run_leased_background_tasks(
    aster_forge_db::RuntimeLeaseDbStore::new(writer_db.clone()),
    aster_forge_runtime::RuntimeLeaseConfig::new("aster_product.background_tasks", owner_id),
    shutdown_token,
    move |leased_shutdown_token| spawn_singleton_runtime_background_tasks(state.clone(), leased_shutdown_token),
)
.await;
```

产品仍然负责 lease id、owner id、store、具体 worker 列表和每个 worker 的业务函数。Forge 只保证获得 lease 后启动 `BackgroundTasks`，失去 lease 或进程 shutdown 时按统一规则关闭这组 worker。

## Dedupe Key

runtime lease 能让正常情况下只有一个实例运行 scheduler，但 leader 切换、短暂 split-brain、进程重试或重复提交仍然可能让同一个计划触发点尝试创建多条任务。因此任务表应该有一个 nullable unique dedupe key：

```text
background_tasks.dedupe_key UNIQUE NULL
```

推荐策略：

- 普通用户手动创建任务：`dedupe_key = NULL`，允许创建多条。
- 计划任务触发：使用 `scheduled_task_dedupe_key(...)`。
- 外部幂等请求：使用产品自己的稳定 key。

```rust
let key = aster_forge_tasks::scheduled_task_dedupe_key(
    "aster_yggdrasil",
    "audit-cleanup",
    scheduled_at,
)?;
```

产品 repository 应该在 unique conflict 后按 `dedupe_key` 查询并返回已有 row。这样就算两个进程同时认为自己可以 enqueue，同一个逻辑触发点也只会落一条任务记录。

这层和 task processing lease 不冲突：

```text
dedupe key              -> 防重复创建任务
processing_token lease  -> 限定当前 owner 可提交的状态；崩溃后仍可能再次执行
runtime lease           -> 防多个实例同时运行调度器/维护循环
scheduled task claim    -> 防多个实例同时运行同一个计划触发点
```

## Scheduled Task Catalog

`run_scheduled_periodic_task` 是比普通 `run_periodic_task` 更适合多实例服务的周期任务入口。它会：

- 先通过 `ScheduledTaskStore::ensure_scheduled_task` 确保 catalog row 存在。
- 用 `ScheduledTaskStore::claim_scheduled_task` 原子 claim 当前 due firing。
- 只有 claim 成功的进程执行业务函数。
- 业务函数执行期间，runner 按 `scheduled_claim_renew_interval(claim_ttl)`（TTL 的三分之一，下限 10ms）周期性调用 `renew_scheduled_task_claim` 延长 claim，任务运行超过 `claim_ttl` 也不会被其他实例重复认领执行。
- 记录结果后用 `complete_scheduled_task` 推进 `next_run_at` 并释放 claim。
- 如果进程在执行中崩溃，续约随之停止，其他进程会在 `claim_ttl` 过期后重新 claim——崩溃恢复语义不变。

续约循环的错误语义和后台任务 heartbeat 对齐：

- `Ok(false)` 表示 ownership 谓词（task id + owner id + claim 时间戳）不再匹配，即 firing 已被其他 runtime 认领，续约循环停止；任务体不会被中止，`complete_scheduled_task` 的 ownership 检查仍然是最后防线。
- `Err(_)` 视为瞬时存储错误，记录 warn 后下一 tick 重试。
- 续约失败永远不中止业务函数——产品任务体跑到一半被强制取消可能比偶发重复执行更危险。

`ScheduledTaskDbStore::renew_claim` 只更新 `claim_expires_at`/`updated_at`，且谓词与 completion 相同：其他 runtime 已认领的 firing（新 owner 或更新的 `last_claimed_at`）无法被续约"复活"。过期但无人争抢的 row 可以被原 owner 续约续上，避免 DB 瞬时故障恢复后 firing 被重复执行。

`aster_forge_db` 提供了默认的 `scheduled_tasks` 表和 `ScheduledTaskDbStore`。产品迁移应该调用：

```rust
manager
    .create_table(aster_forge_db::create_scheduled_tasks_table(
        manager.get_database_backend(),
    ))
    .await?;
manager
    .create_index(aster_forge_db::create_scheduled_tasks_namespace_name_unique_index())
    .await?;
manager
    .create_index(aster_forge_db::create_scheduled_tasks_next_run_index())
    .await?;
```

产品 runtime 侧优先使用 `LeasedScheduledRuntimeConfig::new(...).run(...)`。这个入口同时承接：

- process-level runtime lease；
- lease-scoped `BackgroundTasks` group；
- process-unique owner id；
- scheduled task catalog store；
- scheduled task claim TTL；
- task panic 映射；
- task outcome record hook；
- 失去 lease 或进程 shutdown 时的整组 worker 关闭。

产品侧只声明常驻 singleton worker 和 scheduled task 列表：

```rust
let config = aster_forge_tasks::LeasedScheduledRuntimeConfig::new(
    "aster_yggdrasil",
    "aster_yggdrasil.background_tasks",
    aster_forge_db::RuntimeLeaseDbStore::new(writer_db.clone()),
    aster_forge_db::ScheduledTaskDbStore::new(writer_db.clone()),
    state,
    |panic_message| RuntimeTaskRunOutcome::failed(Some("Task panicked".to_string()), panic_message),
    record_scheduled_task_outcome,
)
.claim_ttl(Duration::from_secs(120))
.lease_ttl(Duration::from_secs(30))
.lease_renew_interval(Duration::from_secs(10))
.lease_standby_retry_interval(Duration::from_secs(5));

config
    .run(shutdown_token, |runtime| {
        runtime.scheduled(
            SystemRuntimeTaskKind::MailOutboxDispatch,
            mail_outbox_dispatch_interval,
            None,
            run_mail_outbox_dispatch,
        );
        runtime.scheduled(
            SystemRuntimeTaskKind::AuditCleanup,
            maintenance_cleanup_interval,
            Some(Duration::from_secs(30)),
            run_audit_cleanup,
        );
    })
    .await;
```

`runtime.singleton_worker(...)` 只用于确实需要全局单 owner 的常驻维护。普通持久化任务消费者必须独立启动，不能放进这个组，否则扩副本仍然只有一个进程消费。`runtime.scheduled(...)` 适合 mail outbox dispatch、audit cleanup、health check 这类固定周期任务。

`run_scheduled_periodic_task` 是低阶 primitive，只有在产品已经自己管理 runtime lease 和 `BackgroundTasks` group 时才应该直接用。常规产品入口不要手写 owner id、lease supervisor、scheduled task registrar 这类 glue。

这张表不替代产品的 `background_tasks` 表。推荐分工是：

- `scheduled_tasks`：计划目录、下一次触发时间、当前 claim owner。
- `background_tasks`：面向 admin UI 的运行历史，或者真正需要异步消费的业务任务。
- `background_tasks.dedupe_key`：把一次 scheduled firing 映射到唯一 history/task row。

Drive 那类更复杂的后台生成任务，比如缩略图、预览、离线下载、归档处理，应该继续作为产品业务任务注册到 typed task registry。Forge 承接的是调度、claim、lease、heartbeat、retry、dedupe、step 和 runtime worker，不承接文件/存储/媒体业务语义。

推荐入口形态：

```rust
fn register_runtime_task_descriptors(registry: &mut aster_forge_runtime::RuntimeComponentRegistry) {
    for task in registered_system_runtime_tasks() {
        registry.component_task(
            aster_forge_tasks::BACKGROUND_TASKS_COMPONENT,
            aster_forge_runtime::RuntimeComponentKind::Tasks,
            task.wire_value,
            task.display_name,
        );
    }
}

aster_forge_runtime::AsterRuntime::builder()
    .component(aster_forge_tasks::background_task_component_with_definitions_from_shutdown(
        registered_runtime_tasks(),
        |shutdown_token| spawn_runtime_background_tasks(state.clone(), shutdown_token),
    ));
```

这让 Forge 持有通用生命周期：组件名、shutdown phase、取消 token、等待 worker 退出、超时 abort 和 shutdown report。产品只保留任务目录、调度策略、具体 worker body 和运行结果持久化。

如果产品需要在 runtime descriptor 里暴露任务目录，应该先用
`runtime_task_registry!` 生成 `RuntimeTaskDefinition` 目录，再接
`background_task_component_with_definitions_from_shutdown(...)`。不要在产品侧再写一层只循环
`wire_value/display_name` 的 registrar，也不要为了拿 runtime shutdown token 自己实现
`AsterRuntimeComponent`。

## Step 状态

常用 API：

- `initial_task_steps_from_specs`
- `set_task_step_active`
- `set_task_step_succeeded`
- `set_task_step_skipped`
- `mark_active_step_failed`

步骤状态只表达 UI/进度语义，不替代数据库 task status。

AsterDrive 已直接使用 Forge 的 `TaskStepStatus`、`TaskStepInfo`、`TaskStepSpec` 和
step mutation helpers。产品侧只保留稳定 step key、`steps_json` parse/serialize，
以及 `AsterError` 到 `TaskRetryClass` 的业务分类。不要在产品 crate re-export
Forge 类型，也不要保留同签名转发 helper。

## 错误边界

Forge 使用 `TaskCoreError` 表达 lease lost、renewal timeout、shutdown requested、cancellation requested 等共享控制错误。产品应在 service 边界映射为自己的错误类型，并提供判断函数给 `ClaimedTaskExecutionStore`。

不要保留无意义薄封装，例如只把 `TaskExecutionContext` 包一层但不增加产品语义。直接用 Forge 类型。

## 测试要求

接入 task 系统时，至少覆盖：

- task kind 与 registry 双向一致。
- claim 使用 processing token fence。
- heartbeat lost lease 后不会写成功状态。
- shutdown 停止 claim；协作退出时 fenced release 到 retry，超时遗弃时保留 lease 到期接管。
- 用户取消与 retry/shutdown 竞争、过期 Canceling 接管、旧 token 不得确认取消。
- 单 worker 容量、持续补位、多个消费者、动态降容、集群额度和空队列健康。
- retryable/permanent/manual failure 分类。
- step JSON parse/serialize 和失败 step 标记。
- dispatch lane 并发限制。
- runtime worker shutdown 不重复 await handle。

## 参考项目

- AsterYggdrasil：当前最清晰的 Forge task 接入参考。业务 kind/payload/repository 留在产品侧，lease、dispatch、runtime 直接走 Forge。
- AsterDrive：已直接使用 Forge 的 typed spec/registry、step/retry、claim、
  lease/heartbeat、claimed execution、drain 和 task temp primitives；产品侧只保留 task API、
  SeaORM store、稳定错误码、storage cancellation、双 temp root 规则、presentation、
  存储迁移任务和运行时任务记录等业务语义。

## 本地验证与下游迁移

公共任务 API 变更应同时验证消费者。Forge 提供从当前 workspace manifest 生成 patch 的脚本：

```bash
python3 ../AsterForge/scripts/local-consumer-patch.py > /tmp/forge-local.toml
cargo test --config /tmp/forge-local.toml --lib
```

命令在下游仓库执行。遇到共享 sccache 服务导致本地 target 权限错误时，可临时加
`--config 'build.rustc-wrapper=""'`。本地 patch 只验证当前源码，不能作为已发布 Git revision 的证据；
发布后统一更新下游 revision，并在去掉 patch 后重新生成/验证 Cargo.lock。

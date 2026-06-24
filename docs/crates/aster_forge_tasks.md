# aster_forge_tasks

`aster_forge_tasks` 是 Forge 里边界最重的 crate。它拥有后台任务的共享机械部分：step 状态、payload/result 序列化、retry 分类、任务 spec adapter、registry 宏、runtime worker、lease guard、heartbeat、lane claiming、dispatch stats、drain loop 和任务临时目录。

它故意不拥有产品数据库实体、SeaORM repository、task kind enum、业务 task payload/result、runtime config、metrics label 或具体任务实现。

## 适用场景

- 产品已有后台任务表，想复用 lease/heartbeat/dispatch 生命周期。
- 产品想用 typed task spec，避免 payload/result 手写 JSON 分散在多个 service。
- 产品需要按 lane 限制并发。
- 产品需要统一 shutdown、periodic task、dispatch backoff。
- 产品需要 task step 进度状态。

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

## 模块地图

主要 reexport 分组：

- `dispatch`：lane claim、并发执行、dispatch stats、drain。
- `execution`：claimed task 的完整执行生命周期。
- `heartbeat`：lease heartbeat loop。
- `lease`：`TaskLease`、`TaskLeaseGuard`、`TaskExecutionContext`。
- `registry`：`TaskRecord` 和 `task_registry!`。
- `retry`：`TaskRetryClass` 和默认 retry delay。
- `runtime`：periodic task、dispatch worker、`BackgroundTasks`。
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

## Runtime worker

`BackgroundTasks` 管理 tokio task handles 和 shutdown token。`run_periodic_task`、`run_dispatch_worker` 适合产品 runtime 层接入。

接入时注意：

- 产品启动时保存 `BackgroundTasks`。
- graceful shutdown 时调用 `shutdown().await`。
- periodic task 的记录逻辑由产品传入 hook。
- panic outcome 由产品决定如何持久化。

## Step 状态

常用 API：

- `initial_task_steps_from_specs`
- `set_task_step_active`
- `set_task_step_succeeded`
- `set_task_step_skipped`
- `mark_active_step_failed`

步骤状态只表达 UI/进度语义，不替代数据库 task status。

## 错误边界

Forge 使用 `TaskCoreError` 表达 lease lost、renewal timeout、shutdown requested 等共享控制错误。产品应在 service 边界映射为自己的错误类型，并提供判断函数给 `ClaimedTaskExecutionStore`。

不要保留无意义薄封装，例如只把 `TaskExecutionContext` 包一层但不增加产品语义。直接用 Forge 类型。

## 测试要求

接入 task 系统时，至少覆盖：

- task kind 与 registry 双向一致。
- claim 使用 processing token fence。
- heartbeat lost lease 后不会写成功状态。
- shutdown 会 release processing task 到 retry。
- retryable/permanent/manual failure 分类。
- step JSON parse/serialize 和失败 step 标记。
- dispatch lane 并发限制。
- runtime worker shutdown 不重复 await handle。

## 参考项目

- AsterYggdrasil：当前最清晰的 Forge task 接入参考。业务 kind/payload/repository 留在产品侧，lease、dispatch、runtime 直接走 Forge。
- AsterDrive：功能更完整，后续接入时可以参考 task API、admin task、存储迁移任务和运行时任务记录。

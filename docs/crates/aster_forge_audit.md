# aster_forge_audit

`aster_forge_audit` 提供 Aster 产品共享的 audit runtime component，以及可选的数据库缓冲写入机制。它不定义产品审计动作，也不接管 audit presentation；它把 Aster 服务里重复的 shutdown 生命周期、batch/delayed flush、queue overflow 和显式 writer ownership 收敛到统一实现。

## 适用场景

- 产品有 audit log manager，需要在 shutdown 时 flush buffer。
- 产品希望在 HTTP / mail / task 停止后记录 `server_shutdown` 这类进程生命周期审计事件。
- 产品已经使用 `aster_forge_runtime::AsterRuntime` component 模式，希望 audit 组件参与同一份 shutdown dependency graph。
- 产品希望 database shutdown 自动等待 audit flush 完成。
- 产品使用 Forge `audit_logs` store，希望复用同一套批量阈值、延迟 flush 和 queue overflow direct-write 行为。

不适合放在这里的内容：

- 产品 `AuditAction` enum。
- 产品 audit detail JSON schema。
- 管理后台展示文案、权限和过滤规则。
- 审计配置 key、默认记录动作和敏感字段策略。
- 产品 presentation、权限和统计口径。

## Cargo

```toml
[dependencies]
aster_forge_audit = { git = "https://github.com/AsterCommunity/AsterForge", features = ["db-writer"] }
```

默认 feature 只提供 lifecycle component。需要 Forge 数据库缓冲 writer 时启用
`db-writer`；该 feature 会启用 `aster_forge_db/audit-log`，产品不需要再复制
`AuditWriter`。

默认不依赖 `aster_forge_mail`。如果产品有 mail outbox，并且希望 shutdown audit 在
outbox drain 之后执行，显式启用 `mail-outbox-dependency`。启用后调用点不需要变化，
仍然使用 `audit_component(...)`：

```toml
[dependencies]
aster_forge_audit = { git = "https://github.com/AsterCommunity/AsterForge", features = ["db-writer", "mail-outbox-dependency"] }
```

产品 migration、查询和统计仍直接配合 `aster_forge_db`：

```toml
[dependencies]
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Component

核心组件名和 shutdown phase 由 Forge 固定：

```text
audit_logs     -> no default dependency
audit_manager  -> depends_on audit_logs
```

常量：

- `AUDIT_LOGS_COMPONENT`
- `AUDIT_MANAGER_COMPONENT`
- `SERVER_START_AUDIT_PHASE`
- `SERVER_SHUTDOWN_AUDIT_PHASE`
- `AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE`

不使用 DB buffer 的产品可以提供资源和三个 hook。hook 内部处理错误并返回 `()` 时，
使用 `audit_component_infallible(...)`。使用 DB buffer 时优先使用下方的
`audit_component_with_writer(...)`，由 Forge 明确启动并停止 writer：

```rust
aster_forge_audit::audit_component_infallible(
    resources,
    |resources| async move { record_server_start(&resources).await },
    |resources| async move { record_server_shutdown(&resources).await },
    |()| async move { stop_product_audit_sink().await },
)
```

`record_server_start` 和 `record_server_shutdown` 是产品语义，所以仍然留在产品仓库。Forge 只保证 startup phase、shutdown phase 和 manager flush 使用同一套 lifecycle graph：server start 作为 required startup phase 执行；server shutdown 在调用方声明的依赖之后、`audit_manager` flush 之前执行。

如果 hook 需要把错误传播给 runtime phase，使用 `audit_component(...)`，三个 future
返回 `Result<(), String>`。Forge 同时保留 fallible 和 infallible 入口，不要求产品为了
公共 API 改变自己的错误策略。

启用 `mail-outbox-dependency` feature 后，同一个 `audit_component(...)` 会把
`audit_logs` 声明为依赖 `aster_forge_mail::MAIL_OUTBOX_COMPONENT`：

```text
mail_outbox    -> depends_on background_tasks
audit_logs     -> depends_on mail_outbox
audit_manager  -> depends_on audit_logs
```

`MAIL_OUTBOX_COMPONENT` 这个常量不需要启用 mail 的 `runtime-component` feature；只有产品实际注册 mail outbox drain 组件时，才需要在 `aster_forge_mail` 上启用 `runtime-component`。

如果产品有别的 shutdown 依赖，使用 caller-provided 变体：

```rust
aster_forge_audit::audit_component_after_infallible(
    resources,
    &[MY_COMPONENT],
    record_server_start,
    record_server_shutdown,
    flush_audit_manager,
)
```

常规 infallible 产品入口应该直接使用 `audit_component_infallible(...)`，需要自定义
依赖时才用 `audit_component_after_infallible(...)`；需要传播 hook 错误时使用对应的
fallible 变体 `audit_component(...)` / `audit_component_after(...)`。不要在产品侧手写 tuple 去拼
`server_start_audit_component(...)`、`server_shutdown_audit_component(...)` 和
`audit_manager_component(...)`。如果产品确实需要拆开注册，也应该使用这些 component
factory，再由产品聚合 component 统一注册 bundle。不要在产品侧直接调用低层 registry
注册函数。

## Database

audit runtime component 只管生命周期，不管表结构。共享 `audit_logs` schema 和基础写入 store 在 `aster_forge_db::audit_log`：

```text
audit_logs
  id            bigint primary key
  user_id       bigint not null default 0
  action        varchar(64) not null
  entity_type   varchar(64) not null
  entity_id     bigint null
  entity_name   varchar(255) null
  details       text null
  ip_address    varchar(128) null
  user_agent    varchar(512) null
  created_at    timestamp / datetime(6)
```

新产品或新 migration 应直接调用 Forge builder，不要复制列定义：

```rust
manager
    .create_table(aster_forge_db::create_audit_logs_table(
        manager.get_database_backend(),
    ))
    .await?;

for index in aster_forge_db::create_audit_logs_base_indexes() {
    manager.create_index(index).await?;
}

for index in aster_forge_db::create_audit_logs_query_indexes() {
    manager.create_index(index).await?;
}
```

历史产品 migration 如果已经发布，不要为了接 Forge 回改旧文件；用新 migration 补齐缺失索引或字段宽度。Schema 目标以 Forge 为准。

运行时写入和通用查询可以走 `AuditLogDbStore` 或函数式 helper：

```rust
let store = aster_forge_db::AuditLogDbStore::new(writer_db.clone());

store
    .create(aster_forge_db::AuditLogCreate {
        user_id: ctx.user_id,
        action: action.as_str().to_string(),
        entity_type: entity_type.as_str().to_string(),
        entity_id,
        entity_name: entity_name.map(ToOwned::to_owned),
        details: details.map(|value| value.to_string()),
        ip_address: ctx.ip_address.clone(),
        user_agent: ctx.user_agent.clone(),
        created_at: chrono::Utc::now(),
    })
    .await?;
```

批量写入优先用 `create_many_requests(...)` 或 `create_audit_log_requests(...)`，不要在产品侧重复维护通用 active model builder。

### Buffered writer

启用 `db-writer` 后，显式创建 `Arc<AuditWriter>` 放到产品资源里。构造不创建 worker、
timer 或执行数据库操作。默认 queue capacity 为 4096，batch size 为 100，部分批次延迟
1 秒；默认 overflow 为额外一个有界 direct-write slot。

```rust
use std::{sync::Arc, time::Duration};
use aster_forge_audit::{AuditWriter, audit_component_with_writer};

let writer = Arc::new(AuditWriter::new(writer_db.clone()));
let component = audit_component_with_writer(
    writer.clone(),
    writer.clone(),
    Duration::from_secs(5),
    |writer| async move {
        writer.record(product_server_start_request()).await
            .map(|_| ()).map_err(|error| error.to_string())
    },
    |writer| async move {
        writer.record(product_server_shutdown_request()).await
            .map(|_| ()).map_err(|error| error.to_string())
    },
);
// 注册 component；数据库关闭必须 depends_on AUDIT_MANAGER_COMPONENT。
```

完整 bundle 先注册 `audit_writer_start` required startup，再执行产品 server-start hook。
shutdown 顺序为 producer/mail drain → server-shutdown hook → writer begin_drain/flush/stop
→ database close。数据库组件必须声明依赖 `AUDIT_MANAGER_COMPONENT`；原有稳定组件名和
shutdown phase 名继续保留。`AsterRuntime::run()` 会在所有 shutdown phase 执行后，
将 drain 等失败作为 `AsterRuntimeError::Shutdown { report }` 返回产品；startup 中断时也
保留 cleanup report。startup hook panic 遵循 required 策略并进入清理；
产品 `before_shutdown` hook panic 同样会执行 audit drain 和数据库关闭，再返回
`AsterRuntimeError::BeforeShutdown { shutdown_report }`。没有 mail outbox 的 follower 可使用
`audit_component_with_writer_after(..., &[BACKGROUND_TASKS_COMPONENT], ...)`，显式覆盖生产者依赖。已有 lifecycle factory 的高级集成可以单独注册
`audit_writer_component(writer, drain_timeout)`，但必须在 server-start hook 前注册，
且不要再注册另一个 `audit_manager_component` 或含 flush 的完整旧 hook bundle。

产品完成 action/entity/detail 到 `AuditLogCreate` 的映射后，调用
`writer.record(request).await` 并处理结果：

- `Queued`：仅接收到内存，不代表持久化。mandatory 策略不能仅检查此结果。
- `DirectPersisted`：仅本次 overflow 记录已提交，不代表 queued batch 提交。
- `NotRunning(record)` / `Full(record)`：未接受记录，所有权返回产品。
- `DirectFailed(failure)`：已接受的 direct record 保留在 writer，错误保留原始 `DbError`。

需要自定义容量、延迟、拒绝 overflow 或有界观测事件时，使用
`AuditWriter::with_config(db, config, OverflowPolicy, events)`。
每个 writer 有独立的队列、worker、取消 token、状态和计数。batch capacity 包含在途记录，
direct slot 仅允许一个未确认记录，两条 lane 由同一 worker 公平调度。

每个 chunk 使用 Forge transaction/store 写入。事务内不自动重试：锁冲突等已知安全失败
标为 Retryable，参数/约束失败标为 Permanent，提交结果不确定标为 UnknownCommit。
`WriteFailureCause::Persistence` 保留原始错误及其 driver classification；sink panic 或
在途取消均保守标记为 UnknownCommit。失败 chunk 不从内存删除，也不计入 persisted。
`retry_failed()` 只显式恢复 Retryable，未知提交必须由产品核对数据库后决定恢复方式。
`snapshot().batch_failure_records` 标识队首受失败影响的记录数，后续记录尚未尝试。
`unconfirmed()` 返回原始请求，stop 后快照稳定。

手工生命周期调用使用 Tokio `Instant` deadline：

```rust
writer.start().await?; // 只允许一次；使用 component 时不要重复调用
writer.record(request).await?;
writer.flush(tokio::time::Instant::now() + Duration::from_secs(5)).await?;
let report = writer.stop(tokio::time::Instant::now() + Duration::from_secs(5)).await;
let remaining = report.snapshot.queued + report.snapshot.direct_pending;
```

stop 停止 admission，等待已接受写入直到 deadline 或不可继续的失败，再取消并 join worker。
超时时返回未确认记录数和原始错误，runtime phase 同时说明 worker 已 join、数据库可进入
后续关闭阶段。部分失败时继续 drain 另一条正常 lane；重复、并发 stop 安全。取消 direct
等待方或 flush future 不会丢弃已接受记录。取消 stop 等待方后仍需另一次 stop 完成 join。
产品应保留 writer 句柄，在失败退出前获取 `unconfirmed()` 并执行自己的恢复策略。

`flush(deadline)` 在 future 首次 poll 时固定已接收范围，batch 和 direct 必须分别确认。
后续记录不会延长等待，其失败不会污染先前已确认范围；过期 deadline 不发起新的强制
写入，空范围仍可成功。正常 threshold/timer 或其他 flush 请求仍可独立写入。
service future panic 时，`AsterRuntime` 仍执行 audit drain、worker join 和数据库关闭，
再返回 `AsterRuntimeError::Service { shutdown_report, before_shutdown_panicked }`。
关闭阶段超时取消时，即使产品 future 的析构 panic，也会记录 failed phase 并继续
audit drain、writer join 和数据库关闭；service future 完成后的析构 panic 同样进入清理。
root token 和独立 service token 在清理前都会取消，供产品 worker 退出；取消 run future
只能通知取消，不能替代显式等待完整 drain。

Observer 使用调用方提供的有界 Tokio mpsc channel，writer 仅 try_send，绝不执行用户
callback；慢 consumer、panic 或关闭 channel 不影响审计持久化，丢弃事件数可查。

### 迁移：旧入口直接删除

本次不提供兼容层。替换关系：

| 删除的 API | 新接入方式 |
| --- | --- |
| `AuditLogManager` | 产品资源持有 `Arc<AuditWriter>` |
| `init_global_audit_log_manager` / `global_audit_log_manager` | assembly 构造 writer，runtime component 显式 start |
| `record_audit_log` | `writer.record(request)`，显式处理 admission/error |
| `flush_global_audit_log_manager` | `writer.flush(deadline)` |
| `shutdown_global_audit_log_manager` / `cancel` | `writer.stop(deadline)` 并检查 remaining/failure |
| `write_audit_log_direct` | `aster_forge_db::create_audit_log_row(&connection_or_transaction, request)`，传播错误 |

startup/未初始化时不再隐式 direct fallback。事务内 mandatory audit 使用 DB store/helper，
与产品业务更新放在同一事务，并由产品映射错误；best-effort 策略也应显式处理记录拒绝、
缓冲写失败和 drain failure。产品 action/detail、脱敏、权限和展示继续留在产品仓库。

通用 cursor 查询、按 action 统计、distinct user 统计和 retention 删除也由 Forge 承接：

```rust
let page = store
    .find_with_filters_cursor(aster_forge_db::AuditLogQuery {
        user_id: filters.user_id,
        action: filters.action.as_deref(),
        entity_type: filters.entity_type.as_deref(),
        entity_id: filters.entity_id,
        after: filters.after,
        before: filters.before,
        limit,
        cursor,
    })
    .await?;
```

## 产品边界

产品侧通常保留：

- typed `AuditAction` 和 `AuditEntityType`。
- `should_record(action)` 和 runtime config 映射。
- detail struct、serde schema 和脱敏规则。
- admin API 的查询、权限、分页和展示模型。
- 哪些 action 归入登录活跃、Yggdrasil API 调用或管理操作。

Forge 侧承接：

- shutdown component 名称、依赖和 phase。
- `server_shutdown` hook 与 manager flush 的顺序。
- 调用方持有的 `AuditWriter`、显式生命周期、默认 buffering policy 和分类失败恢复边界。
- `audit_logs` 通用表结构、索引 builder 和基础 store。
- 通用 cursor query、count、distinct user count、delete 和 write helper。

测试覆盖构造无后台工作、startup 边界、threshold/delayed/manual flush、overflow 和在途容量、批次原子失败、原始错误、未知提交不重放、取消、drain timeout、并发 producer/stop、observer 隔离及数据库关闭顺序。

运行 `cargo test -p aster_forge_audit`、`cargo test -p aster_forge_audit --features db-writer`、
`cargo test -p aster_forge_audit --all-features`（含 Docker PostgreSQL 测试）、
`cargo test -p aster_forge_runtime --all-features` 和相关 crates 的 Clippy。这个边界避免 Drive、Yggdrasil 和后续 Aster 服务在表结构、shutdown 顺序、批量写入和索引名上继续分叉，同时不把业务审计语义塞进共享 crate。

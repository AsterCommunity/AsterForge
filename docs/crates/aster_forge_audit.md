# aster_forge_audit

`aster_forge_audit` 提供 Aster 产品共享的 audit runtime component，以及可选的数据库缓冲写入机制。它不定义产品审计动作，也不接管 audit presentation；它把 Aster 服务里重复的 shutdown 生命周期、batch/delayed flush、queue overflow fallback 和全局 manager 收敛到统一实现。

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
`AuditLogManager`。

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

产品侧只提供资源和三个 hook。大多数产品的审计写入会在内部处理错误，hook 返回
`()`；这种情况优先使用 `audit_component_infallible(...)`：

```rust
aster_forge_audit::audit_component_infallible(
    resources,
    |resources| async move { record_server_start(&resources).await },
    |resources| async move { record_server_shutdown(&resources).await },
    |()| async move { shutdown_global_audit_log_manager().await },
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

启用 `db-writer` 后，运行时直接初始化 Forge manager：

```rust
aster_forge_audit::init_global_audit_log_manager(writer_db.clone());
```

产品完成 action/entity/detail 到 `AuditLogCreate` 的映射后，直接记录：

```rust
aster_forge_audit::record_audit_log(
    state.writer_db(),
    aster_forge_db::AuditLogCreate {
        user_id: ctx.user_id,
        action: action.as_str().to_string(),
        entity_type: entity_type.as_str().to_string(),
        entity_id,
        entity_name: entity_name.map(ToOwned::to_owned),
        details: details.map(|value| value.to_string()),
        ip_address: ctx.ip_address.clone(),
        user_agent: ctx.user_agent.clone(),
        created_at: chrono::Utc::now(),
    },
)
.await;
```

`record_audit_log(...)` 在 manager 尚未初始化时使用传入的 writer DB 直接写入，因此
startup audit、测试和非标准入口不需要产品侧再维护 fallback 分支。默认策略为：

- queue capacity：`4096`
- immediate batch size：`100`
- delayed flush：`1s`
- queue 满时：当前记录 direct write，同时调度已缓冲记录 flush

需要不同策略时创建 `AuditLogManager::with_config(...)`；常规产品应使用默认的全局
manager API。shutdown component 的 flush hook 直接调用
`shutdown_global_audit_log_manager()`，不要再包一层同签名函数。

事务内或明确要求绕过进程 buffer 的写入直接使用
`write_audit_log_direct(...)`；它保留同样的 best-effort warning 行为，但不会改走全局
manager。

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
- `AuditLogManager`、默认 buffering policy、全局 manager 和未初始化时的 direct-write fallback。
- `audit_logs` 通用表结构、索引 builder 和基础 store。
- 通用 cursor query、count、distinct user count、delete 和 write helper。

测试至少覆盖 threshold flush、partial delayed flush、cancel 后显式 flush、manual flush 后继续记录、queue overflow direct write 和 shutdown 顺序。这个边界避免 Drive、Yggdrasil 和后续 Aster 服务在表结构、shutdown 顺序、批量写入和索引名上继续分叉，同时不把业务审计语义塞进共享 crate。

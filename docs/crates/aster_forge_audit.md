# aster_forge_audit

`aster_forge_audit` 提供 Aster 产品共享的 audit runtime component。它不定义产品审计动作，也不接管 audit presentation；它只把 Aster 服务里重复的 shutdown 生命周期收敛到统一组件。

## 适用场景

- 产品有 audit log manager，需要在 shutdown 时 flush buffer。
- 产品希望在 HTTP / mail / task 停止后记录 `server_shutdown` 这类进程生命周期审计事件。
- 产品已经使用 `aster_forge_runtime::AsterRuntime` component 模式，希望 audit 组件参与同一份 shutdown dependency graph。
- 产品希望 database shutdown 自动等待 audit flush 完成。

不适合放在这里的内容：

- 产品 `AuditAction` enum。
- 产品 audit detail JSON schema。
- 管理后台展示文案、权限和过滤规则。
- 审计配置 key、默认记录动作和敏感字段策略。
- 产品 presentation、权限和统计口径。

## Cargo

```toml
[dependencies]
aster_forge_audit = { git = "https://github.com/AsterCommunity/AsterForge" }
```

如果产品同时需要共享 audit table 和 store，应配合 `aster_forge_db`：

```toml
[dependencies]
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Component

核心组件名和 shutdown phase 由 Forge 固定：

```text
audit_logs     -> depends_on mail_outbox
audit_manager  -> depends_on audit_logs
```

常量：

- `AUDIT_LOGS_COMPONENT`
- `AUDIT_MANAGER_COMPONENT`
- `SERVER_SHUTDOWN_AUDIT_PHASE`
- `AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE`

推荐产品侧只提供资源和两个 hook：

```rust
pub fn audit_component(
    resources: AuditRuntimeResources,
) -> RuntimeComponentBundleRegistration<impl aster_forge_runtime::RuntimeComponentBundle> {
    aster_forge_audit::audit_component(
        resources,
        |resources| async move {
            record_server_shutdown(&resources).await;
            Ok(())
        },
        |()| async move {
            shutdown_global_audit_log_manager().await;
            Ok(())
        },
    )
}
```

`record_server_shutdown` 是产品语义，所以仍然留在产品仓库。Forge 只保证它在 `mail_outbox` drain 之后、`audit_manager` flush 之前执行。

如果产品需要拆开注册，也应该使用 `server_shutdown_audit_component(...)` 和
`audit_manager_component(...)` 这两个 component factory，再由入口或产品聚合 component
统一注册 bundle。不要在产品侧直接调用低层 registry 注册函数。

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
- `audit_logs` 通用表结构、索引 builder 和基础 store。
- 通用 cursor query、count、distinct user count、delete 和 write helper。

这个边界避免 Drive、Yggdrasil 和后续 Aster 服务在表结构、shutdown 顺序、批量写入和索引名上继续分叉，同时不把业务审计语义塞进共享 crate。

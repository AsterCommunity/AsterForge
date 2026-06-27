# aster_forge_db

`aster_forge_db` 提供 SeaORM 相关的共享基础设施：数据库连接、连接关闭、查询重试、分页构造、搜索 query 处理、排序 helper、事务封装、runtime lease 数据库 store、scheduled task catalog 数据库 store、system config store、mail outbox store 和 audit log store。

## 适用场景

- 多数据库 URL 连接和连接池配置。
- `DbHandles` 管理读写连接并在 shutdown 时关闭。
- transient 数据库错误重试。
- SeaORM 查询分页、排序、全文搜索条件复用。
- 事务 helper。
- 多实例 runtime lease 的默认数据库表和 store。
- 多实例 scheduled task catalog 的默认数据库表和 store。
- system config 的默认数据库表、唯一索引、实体和 store。
- mail outbox 的默认数据库表、索引和 dispatch store。
- audit logs 的默认数据库表、索引和基础写入/统计 store。

不适合放在这里的内容：

- 产品业务实体和产品专属 migration。
- repository 业务查询。
- 权限过滤。
- 数据库配置来源和加密存储。

## Runtime Leases

模块：`runtime_lease`

`RuntimeLeaseDbStore` 是 `aster_forge_runtime::RuntimeLeaseStore` 的 SeaORM 实现，用来协调多实例服务里的 process-level singleton worker group。它只管理 `runtime_leases` 表，不管理产品任务表。

表结构由 Forge 维护：

```text
runtime_leases
  lease_id          primary key
  owner_id          current process/node owner
  expires_at        takeover deadline
  last_renewed_at   last successful acquire/renew
  created_at
  updated_at
```

产品 migration crate 不应该复制这张表的列定义，直接调用 Forge builder：

```rust
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(aster_forge_db::create_runtime_leases_table(
                manager.get_database_backend(),
            ))
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(aster_forge_db::drop_runtime_leases_table())
            .await
    }
}
```

运行时接入：

```rust
let store = aster_forge_db::RuntimeLeaseDbStore::new(writer_db.clone());
```

acquire 语义：

- 没有 row：插入并获得 lease。
- row 已过期：条件更新 owner 并获得 lease。
- row 属于同 owner：更新 expiry，相当于续租。
- row 属于其他未过期 owner：返回 standby owner。

renew 和 release 都要求 `lease_id + owner_id` 匹配。这样旧 owner 在失去 lease 后不能再续约，也不能释放新 owner 的 lease。

## Scheduled Tasks

模块：`scheduled_task`

`ScheduledTaskDbStore` 是 `aster_forge_tasks::ScheduledTaskStore` 的 SeaORM 实现，用来协调多实例服务里的固定周期任务。调度 DTO 和 runner trait 归 `aster_forge_tasks` 所有；这个 crate 只管理 `scheduled_tasks` 表、建表/index builder 和 SeaORM store，不替产品执行任务，也不替代产品自己的 `background_tasks` 表。

表结构由 Forge 维护：

```text
scheduled_tasks
  task_id           primary key, usually namespace:task_name
  namespace         product namespace
  task_name         stable task wire name
  display_name      operator-facing display name
  next_run_at       next due time
  claim_owner_id    current runtime owner
  claim_expires_at  claim takeover deadline
  last_claimed_at
  last_finished_at
  created_at
  updated_at
```

产品 migration crate 不应该复制这张表的列定义，直接调用 Forge builder：

```rust
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(aster_forge_db::drop_scheduled_tasks_next_run_index())
            .await?;
        manager
            .drop_index(aster_forge_db::drop_scheduled_tasks_namespace_name_unique_index())
            .await?;
        manager
            .drop_table(aster_forge_db::drop_scheduled_tasks_table())
            .await
    }
}
```

运行时接入：

```rust
let store = aster_forge_db::ScheduledTaskDbStore::new(writer_db.clone());
```

claim 语义：

- row 不存在：由 runner 先 ensure catalog row。
- `next_run_at` 还没到：不执行。
- row 已到期且没有 fresh claim：当前 runtime 写入 `claim_owner_id` 并执行。
- row 被其他 runtime fresh claim：跳过本轮。
- claim 过期：其他 runtime 可以接管，适合进程崩溃恢复。
- completion 要求 claim owner 和 claim timestamp 匹配，然后推进 `next_run_at` 并清理 claim。

推荐配合 `background_tasks.dedupe_key` 使用。`scheduled_tasks` 解决“谁跑这一次计划触发”，`background_tasks.dedupe_key` 解决“这一次触发最多写一条历史/业务任务 row”。

## System Config

模块：`system_config`

`SystemConfigDbBinding` / `SystemConfigDbStore` 提供 Aster 产品通用的 `system_config` 表结构、唯一索引、SeaORM entity、默认值 seed/repair、upsert、delete、lock、cursor 查询和可见 custom config 查询。配置定义、normalizer、dependency validator、runtime snapshot 和 reload diff 仍然归 `aster_forge_config`；这个 crate 只负责数据库持久化边界。

表结构由 Forge 维护：

```text
system_config
  id                 primary key
  key                stable config key, unique, varchar(128)
  value              storage string, list values are JSON text
  value_type         string/multiline/string_array/string_enum/string_enum_set/number/boolean
  requires_restart   whether hot reload can apply the value
  is_sensitive       whether API and audit output must redact the value
  source             system/custom
  visibility         private/public/authenticated
  namespace          optional product namespace
  category           product UI grouping category
  description        product-facing backend description
  updated_at
  updated_by         optional actor user id
```

新产品 migration crate 不应该复制这张表的列定义，直接调用 Forge builder：

```rust
manager
    .create_table(aster_forge_db::create_system_config_table(
        manager.get_database_backend(),
    ))
    .await?;
manager
    .create_index(aster_forge_db::create_system_config_key_unique_index())
    .await?;
```

down migration：

```rust
manager
    .drop_index(aster_forge_db::drop_system_config_key_unique_index())
    .await?;
manager
    .drop_table(aster_forge_db::drop_system_config_table())
    .await?;
```

运行时接入：

```rust
static SYSTEM_CONFIG_STORE: aster_forge_db::SystemConfigDbBinding =
    aster_forge_db::SystemConfigDbBinding::new(
    &CONFIG_REGISTRY,
    DEPRECATED_SYSTEM_CONFIG_KEYS,
);

SYSTEM_CONFIG_STORE.ensure_defaults(writer_db).await?;
let row = SYSTEM_CONFIG_STORE
    .upsert(
        writer_db,
        aster_forge_db::SystemConfigUpsert {
            key,
            value: &normalized_storage,
            visibility,
            updated_by,
        },
    )
    .await?;
```

如果产品确实需要把一个 owned `DatabaseConnection` 和 registry 绑成值对象，也可以使用 `SystemConfigDbStore::new(...)`。新产品通常优先用 `SystemConfigDbBinding`，因为 repository function 已经能从 runtime state 里拿到 reader/writer connection。

产品侧只保留有业务语义的边界：

- config key 常量、默认值函数和 `ConfigRegistry`。
- normalizer / dependency validator。
- 管理 API DTO、权限和 audit action/detail。
- 产品错误映射，例如把“删除 system config”映射成 forbidden，把 missing key 映射成 not found。

API 展示可以用 Forge 的中间 presentation row，产品 DTO 只负责 API schema 和字段裁剪：

```rust
let presented = aster_forge_db::present_system_config(row, |error| {
    tracing::warn!(%error, "invalid stored config value");
});
```

运行时快照可以直接使用 Forge model，不需要产品再定义一份等价 entity：

```rust
let runtime_config = aster_forge_config::SyncRuntimeConfig::<
    aster_forge_db::system_config::Model,
>::new();
```

如果旧项目已经发布历史 migration，不要回改历史文件；后续新 migration 或新项目从 Forge builder 开始。

## Audit Logs

模块：`audit_log`

`AuditLogDbStore` 提供 Aster 产品通用的 audit log 表结构、索引 builder、基础写入、cursor 查询、统计和删除 helper。产品仍然负责 typed action enum、detail schema、权限、展示和统计口径。

表结构由 Forge 维护：

```text
audit_logs
  id            primary key
  user_id       actor user id, system events use 0
  action        stable action wire value, varchar(64)
  entity_type   target entity type, varchar(64)
  entity_id     optional target entity id
  entity_name   optional target display name
  details       optional product-owned JSON text
  ip_address    optional client IP
  user_agent    optional user-agent
  created_at
```

新产品 migration crate 不应该复制列定义，直接调用 Forge builder：

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

base indexes 覆盖普通时间、action、user 查询：

- `idx_audit_logs_created_at`
- `idx_audit_logs_action`
- `idx_audit_logs_user_id`

query indexes 覆盖 Aster 管理后台常见 cursor/aggregation 查询：

- `idx_audit_logs_action_created_user`
- `idx_audit_logs_created_id`
- `idx_audit_logs_user_created_id`
- `idx_audit_logs_action_created_id`
- `idx_audit_logs_entity_type_created_id`

down migration 可以直接用命名 drop builder：

```rust
manager
    .drop_index(aster_forge_db::drop_audit_logs_entity_type_created_id_index())
    .await?;
manager
    .drop_index(aster_forge_db::drop_audit_logs_action_created_id_index())
    .await?;
manager
    .drop_index(aster_forge_db::drop_audit_logs_user_created_id_index())
    .await?;
manager
    .drop_index(aster_forge_db::drop_audit_logs_created_id_index())
    .await?;
manager
    .drop_index(aster_forge_db::drop_audit_logs_action_created_user_index())
    .await?;
manager
    .drop_table(aster_forge_db::drop_audit_logs_table())
    .await?;
```

运行时写入：

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

批量写入优先用 `create_many_requests(...)` 或 `create_audit_log_requests(...)`。如果产品为了 API schema 仍保留自己的 typed SeaORM entity，也不要再复制通用 insert/batch insert 和 cursor query 逻辑；读写可以统一落到 Forge 的 string-action model，再在产品边界把 action string 转回 typed enum。

通用查询：

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

`AuditLogQuery` 固定按 `(created_at, id)` 倒序 cursor 查询，并把 `limit` 限制在 `1..=200`。这样 account/admin audit log 列表不需要在每个产品里重复写 cursor 条件。

store 负责：

- 校验 `user_id >= 0`。
- 校验 `action`、`entity_type` 非空且不超过 64 字节。
- 校验 `entity_name`、`ip_address`、`user_agent` 长度。
- 单条和批量插入。
- 通用 cursor filter 查询。
- `[start, end)` 时间范围统计。
- action 范围统计。
- action 范围内 distinct positive user 统计。
- retention 删除。

产品侧仍然负责：

- `AuditAction` enum、group 和 action allowlist。
- detail JSON schema、脱敏和序列化。
- admin/account API 的权限过滤和 presentation。
- 统计口径，比如哪些 action 算登录活跃、Yggdrasil API 调用或管理操作。

## Mail Outbox

模块：`mail_outbox`

`MailOutboxDbStore` 提供 Aster 产品通用的 mail outbox 持久化状态机。`aster_forge_mail`
拥有 `MailOutboxStatus`、`MailTemplateCode` 和 `StoredMailPayload`；这个 crate 拥有表结构、
索引 builder 和 SeaORM store。产品仍然负责模板 payload enum、模板渲染、发信审计和业务上下文。

表结构由 Forge 维护：

```text
mail_outbox
  id                     primary key
  template_code          shared Aster template code, varchar(64)
  to_address             recipient address
  to_name                optional recipient display name
  payload_json           stored template payload JSON
  status                 pending / processing / retry / sent / failed
  attempt_count
  next_attempt_at
  processing_started_at
  sent_at
  last_error
  created_at
  updated_at
```

产品 migration crate 不应该复制列定义，直接调用 Forge builder：

```rust
manager
    .create_table(aster_forge_db::create_mail_outbox_table(
        manager.get_database_backend(),
    ))
    .await?;
manager
    .create_index(aster_forge_db::create_mail_outbox_due_index())
    .await?;
manager
    .create_index(aster_forge_db::create_mail_outbox_processing_index())
    .await?;
manager
    .create_index(aster_forge_db::create_mail_outbox_sent_at_index())
    .await?;
```

`template_code` 当前按 64 字节建列。这个长度刻意比现有最长的
`external_auth_email_verification` 更宽，避免新增共享模板名时为了一个 code 再做产品迁移。
已经存在的产品库如果历史上建成 32 字节，应通过新的产品迁移放宽到 64，不要修改历史迁移文件。

运行时接入：

```rust
let store = aster_forge_db::MailOutboxDbStore::new(writer_db.clone());
```

store 负责：

- `create(...)` 插入 pending row 并做基础长度/空值校验。
- `list_claimable(...)` 找到 due pending/retry row 和 stale processing row。
- `try_claim(...)` 原子切到 processing。
- `mark_sent(...)` 切到 sent，清空 `payload_json` 和 `last_error`。
- `mark_retry(...)` 更新 attempt、next attempt 和 last error。
- `mark_failed(...)` 切到 failed，并清空 `payload_json`。
- `count_active(...)` 统计 pending/retry row。

产品 dispatch 仍然把 `MailOutboxDbStore` 传给 `aster_forge_mail::dispatch_mail_outbox(...)`，
并提供 deliver/audit hook。不要在产品侧重复写 claim/retry/sent/failed 状态机。

## Cargo 接入

```toml
[dependencies]
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge" }
```

默认 feature 只提供连接、retry、pagination、search、sort 和 transaction 这类基础数据库工具。共享表和 runtime 组件按需开启：

```toml
aster_forge_db = {
    git = "https://github.com/AsterCommunity/AsterForge",
    features = [
        "runtime-component",
        "runtime-lease",
        "scheduled-task",
        "system-config",
        "mail-outbox",
        "audit-log",
    ],
}
```

需要完整 Aster 平台数据库机械层的产品可以使用：

```toml
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge", features = ["all"] }
```

## 连接与关闭

核心类型：

- `DatabaseConfig`
- `DbHandles`
- `aster_forge_metrics::DbMetricsRecorder`
- `aster_forge_metrics::NoopDbMetrics`

典型接入：

```rust
let db = aster_forge_db::connect_with_metrics(&config.database, metrics.clone()).await?;
let db_handles = aster_forge_db::DbHandles::single(db);
```

如果产品已经使用 `aster_forge_runtime::AsterRuntime` component 模式，优先把数据库句柄注册成 runtime component：

```rust
aster_forge_runtime::AsterRuntime::builder()
    .component(aster_forge_db::database_component_after(
        db_handles,
        &[
            aster_forge_tasks::BACKGROUND_TASKS_COMPONENT,
            aster_forge_mail::MAIL_OUTBOX_COMPONENT,
            aster_forge_audit::AUDIT_MANAGER_COMPONENT,
        ],
    ));
```

`database_component_after()` 负责通用生命周期：注册 `database` 组件、注册标准 database ping health check、保存产品声明的 shutdown 依赖、在依赖组件关闭后消费 `DbHandles` 并调用 `close()`。产品仍然负责连接配置、migration、repository、额外业务健康检查和错误映射。

还没迁移到 `AsterRuntime` 的应用仍然需要在自己的关闭流程里直接关闭句柄：

```rust
db_handles.close().await?;
```

产品侧应把 `DbError` 映射到自己的启动错误或内部错误。不要吞掉 close 错误，至少要记录。

## 健康检查

如果产品使用 `aster_forge_runtime::RuntimeComponentRegistry`，可以直接注册标准数据库 ping 检查：

```rust
registry.register_bundle(aster_forge_db::database_health_component(
    db_handles.reader().clone(),
));
```

这个检查注册在 `database` component 下，覆盖 readiness 和 diagnostics scope，默认 timeout 为 `DATABASE_HEALTH_CHECK_TIMEOUT`。它只做 `DatabaseConnection::ping()`，返回标准的 `HealthComponentReport`：

- 成功：`database ping succeeded`
- 失败：`database ping failed: ...`

产品仍然负责决定使用 reader 还是 writer 连接、是否还需要 migration 状态、replica lag、follower readiness 等更高层诊断。不要在产品侧重复写普通 ping health，除非确实有额外业务语义。

新产品接入时优先使用 `database_component_after(...)` 或 `database_health_component(...)`。低层 registry 注册函数是 crate 内部实现细节，不作为子系统 API 暴露。

## 重试

模块：`retry`

`RetryConfig` 用于描述连接或查询重试策略。产品侧决定哪些调用允许重试，尤其要区分：

- 幂等读取可以重试。
- 事务内写入一般不要在外层盲目重试。
- 已经产生外部副作用的流程不能简单重放。

## 分页、排序、搜索

模块：

- `pagination`
- `sort`
- `search_query`

典型用途：

- 给 SeaORM query 添加 `limit/offset`。
- 按列和 id tie-breaker 排序。
- 生成 SQL LIKE 转义条件。
- 生成 SQLite FTS 或 MySQL boolean mode 查询。

产品侧仍然负责字段白名单和索引设计。

## 事务

模块：`transaction`

事务 helper 用来统一 SeaORM transaction 调用形式。Forge 负责事务机械行为，包括 begin、commit、rollback、rollback 失败日志和未显式结束事务的 guard 记录；业务规则仍然留在 repository/service 层。

手动事务边界直接返回 `DbError`：

```rust
let txn = aster_forge_db::transaction::begin(db).await?;
repository::write(&txn, input).await?;
aster_forge_db::transaction::commit(txn).await?;
```

产品仓库如果要保留自己的错误类型，可以在本地 facade 或 service 边界把 `DbError` 转成产品错误。

`with_transaction` 更适合 service/repository 组合调用。它允许回调返回产品错误类型 `E`，并只把 Forge 自己创建的事务边界错误映射成 `E`：

```rust
impl From<aster_forge_db::DbError> for AsterError {
    fn from(value: aster_forge_db::DbError) -> Self {
        AsterError::internal(value)
    }
}

let user = aster_forge_db::transaction::with_transaction(db, async |txn| {
    let user = user_repo::create(txn, input).await?;
    audit_repo::record_user_create(txn, user.id).await?;
    Ok::<_, AsterError>(user)
})
.await?;
```

错误边界规则：

- begin、commit、rollback 失败是 Forge DB 边界错误，来源类型是 `DbError`。
- `with_transaction` 的回调错误是产品或子系统错误，会原样返回，不会包装成 `DbError`。
- `with_transaction` 的错误类型需要满足 `E: From<DbError> + std::fmt::Display`，这样 commit/begin 失败可以进入产品错误边界，rollback 日志也能记录回调错误。
- 回调失败后 rollback 如果也失败，函数仍然返回原始回调错误，同时记录 rollback 失败日志；不要用 rollback 失败覆盖业务失败。

不要把校验错误、权限错误、协议错误等业务失败转换成 `DbError`。如果子系统有自己的错误类型，例如协议层错误，可以直接为该类型实现 `From<DbError>`，或者先转成产品错误再由子系统错误接收。

## 错误边界

`DbError` 表达共享数据库基础设施失败，包括连接、关闭、重试耗尽、查询 helper 参数错误和事务边界失败。产品侧应该在启动、service 或协议边界映射成自己的错误类型。

推荐边界：

- repository 只表达数据库读写需要的输入输出，不构造 HTTP 或协议错误。
- service 使用产品错误类型组合 repository、事务、audit 和外部副作用。
- API handler 把产品错误映射为稳定响应码、状态码、审计字段和本地化文案。
- Yggdrasil/Authlib-injector 这类协议端点可以使用协议错误类型，但仍然应该在协议边界接收 `DbError`，不要让 Forge 错误文案泄漏到协议响应。

## 测试要求

- SQLite 内存库至少覆盖连接、事务和基础 query helper。
- 产品 repository 要覆盖 token fence、状态转换和并发保护。
- shutdown 测试要确认 `DbHandles::close()` 被调用或错误被记录。

## 参考项目

- AsterDrive：复杂 repository、跨数据库行为和 migration CLI。
- AsterYggdrasil：较轻服务启动/关闭链路和任务 repository token fence。

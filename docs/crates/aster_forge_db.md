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

claim 生命周期有三个条件更新，全部以 task id + owner id + `last_claimed_at` 为 ownership 谓词：

- `claim_due`：原子认领 due firing，fresh claim 或未到期的 row 返回 `None`。
- `renew_claim`：任务体执行期间延长 `claim_expires_at`，谓词不匹配（firing 已被其他 runtime 认领）返回 `false`，不更新任何 row。
- `complete_claim`：推进 `next_run_at` 并释放 claim，谓词不匹配返回 `false`。

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
        aster_forge_db::drop_index_if_exists(
            manager.get_connection(),
            aster_forge_db::SCHEDULED_TASKS_TABLE,
            aster_forge_db::SCHEDULED_TASK_NEXT_RUN_INDEX,
        )
        .await?;
        aster_forge_db::drop_index_if_exists(
            manager.get_connection(),
            aster_forge_db::SCHEDULED_TASKS_TABLE,
            aster_forge_db::SCHEDULED_TASK_NAMESPACE_NAME_UNIQUE_INDEX,
        )
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
aster_forge_db::drop_index_if_exists(
    manager.get_connection(),
    aster_forge_db::SYSTEM_CONFIG_TABLE,
    aster_forge_db::SYSTEM_CONFIG_KEY_UNIQUE_INDEX,
)
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

down migration 直接传表名和稳定索引名给共享执行 helper：

```rust
for index_name in [
    aster_forge_db::AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX,
    aster_forge_db::AUDIT_LOG_ACTION_CREATED_ID_INDEX,
    aster_forge_db::AUDIT_LOG_USER_CREATED_ID_INDEX,
    aster_forge_db::AUDIT_LOG_CREATED_ID_INDEX,
    aster_forge_db::AUDIT_LOG_ACTION_CREATED_USER_INDEX,
] {
    aster_forge_db::drop_index_if_exists(
        manager.get_connection(),
        aster_forge_db::AUDIT_LOGS_TABLE,
        index_name,
    )
    .await?;
}
manager
    .drop_table(aster_forge_db::drop_audit_logs_table())
    .await?;
```

`drop_index_if_exists` 接受任意 SeaORM `ConnectionTrait`。migration 传入 `manager.get_connection()` 即可；SQLite/PostgreSQL 使用原生 `IF EXISTS`，MySQL 由 Forge 查询 `information_schema.statistics` 后再执行 `DROP INDEX ... ON ...`，产品不需要复制数据库 metadata 查询。MySQL 索引随业务表或字段改名时，使用幂等的 `rename_mysql_index_if_exists`；它只在源索引存在且目标索引不存在时执行重命名。

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
- `dispatch_due(...)` 运行一轮标准 outbox dispatch，内部复用 shared claim/retry/sent/failed 状态机。

常规产品接入应该直接调用 `MailOutboxDbStore::dispatch_due(...)`，只提供：

- `deliver(row)`：模板渲染和实际发送；
- `on_sent(context, attempt_count, subject)`：发送成功后的产品审计；
- `on_failed(context, attempt_count, error_message)`：永久失败后的产品审计。

不要在产品侧重复写 claim/retry/sent/failed 状态机。只有当产品没有使用 Forge 的
`mail_outbox` 表时，才需要直接接入 `aster_forge_mail::dispatch_mail_outbox(...)`。

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

产品侧应把 `DbError` 映射到自己的启动错误或内部错误。不要吞掉 close 错误，至少要记录。`close()` 总是尝试关闭所有池：split SQLite 配置下即使 reader 关闭失败，writer 池也会被关闭（`close` 消费句柄，提前返回会让 writer 池泄漏且无法重试），返回的是首个失败。

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

重试分三层，按优先级从高到低：

- 多语句事务：`transaction::with_transaction_retry`（见「事务」一节）。它重放完整
  `begin -> callback -> commit` 并分类 commit 结果，是唯一对事务安全的重试层。
- 幂等单语句（读取、upsert、按主键删除）：`with_sea_orm_retry` /
  `with_sea_orm_retry_timeout`。**禁止用在多语句事务内**：deadlock 会回滚整个事务，
  事后逐语句重跑会落在 autocommit 上，产生部分写入。
- crate 内部 `DbError` 工作流（连接启动等）：`with_retry`。

`RetryConfig` 是唯一的重试配置类型，通过画像构造再按需覆盖字段：

- `RetryConfig::connection()`（也是 `Default`）：连接/启动路径，3 次、100ms 基础、5s 上限。
- `RetryConfig::deadlock()`：deadlock/serialization 重试，3 次、5ms 基础、50ms 上限——
  锁竞争窗口很短，快退避才有效。`with_transaction_retry` 应该用这个画像。

延迟由 `aster_forge_utils::backoff` 的指数退避 + 50%–150% 乘性抖动 + 硬上限组成
（事务重试走无抖动的确定性退避）。产品侧决定哪些调用允许重试，尤其要区分：

- 幂等读取可以重试。
- 事务内写入一般不要在外层盲目重试。
- 已经产生外部副作用的流程不能简单重放。

### 结构化数据库错误

`database_error_kind(&sea_orm::DbErr)` 用于从 SeaORM/SQLx 驱动错误提取
产品无关、适合基础设施决策的错误分类。目前支持：

- `Deadlock`：MySQL `1213`、PostgreSQL SQLSTATE `40P01`；
- `SerializationFailure`：PostgreSQL SQLSTATE `40001`；
- `LockTimeout`：MySQL `1205`、PostgreSQL SQLSTATE `55P03`、SQLite `BUSY`(5)/`LOCKED`(6)
  家族（按扩展结果码低字节匹配，覆盖 `BUSY_SNAPSHOT` 等扩展码）；
- `UniqueConstraint`：SQLx `ErrorKind::UniqueViolation`；
- `ForeignKeyConstraint`：SQLx `ErrorKind::ForeignKeyViolation`。

分类读取驱动提供的错误号、SQLSTATE 或 SQLx 的跨后端 `ErrorKind`，不匹配本地化的错误文本。`DbErr::sql_err()`
只覆盖常见唯一键和外键约束；deadlock 等其他错误仍应通过 `RuntimeErr::SqlxError`
下钻到驱动错误。Forge 只提供分类，不会替产品决定是否重跑事务。

从 `sea_orm::DbErr` 构造 `DbError` 时统一走 `DbError::from`（自动携带分类），不要用 `DbError::database_operation`——后者会丢弃分类，让上层重试判断退化为"未分类错误不可重试"。audit_log helper、health check ping 和 PRAGMA 设置都遵循这条规则；只有携带产品自定义消息（无对应驱动错误）时才用 `database_operation`。

所有重试层的可重试判断都从 `database_error_kind` 派生，任何一层都不读取错误消息文本：

- 连接获取失败（`ConnectionAcquire`/`Conn`）始终可重试——语句还没执行，重放不会重复。
- 语句执行失败只在分类为 `Deadlock`/`SerializationFailure`/`LockTimeout` 时可重试
  （`DatabaseErrorKind::is_transient_locking()`）。
- 未分类错误不可重试：没有驱动证据证明操作以可安全重放的方式失败，应立刻让调用方看到
  失败。`DbError::is_retryable()` 遵循同一规则；连接启动路径把连接失败映射为
  `DbError::DatabaseConnection`，PRAGMA 等设置步骤走 `DbError::from` 自动分类，
  因此启动重试只覆盖连接失败和 SQLite 瞬时锁竞争。

```rust
use aster_forge_db::{DatabaseErrorKind, database_error_kind};

if database_error_kind(&db_error) == Some(DatabaseErrorKind::Deadlock) {
    // 只有确认回调没有外部副作用时，产品侧才应开启有界事务重试。
}
```

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

LIKE 转义语义（三端一致）：`escape_like_query` 先转义 `\` 自身、再转义 `%` 和 `_`
（顺序不能换，否则已转义的 `\%` 会被二次转义成活通配符）；转义符约定为 `\`。MySQL 和
PostgreSQL 默认转义符就是 `\`，SQLite 没有默认转义符，所以自建条件必须显式配
`ESCAPE '\'` 子句（`search_query::lower_like_condition` 已通过
`LikeExpr::escape('\\')` 声明，三端渲染结果有测试锁定）。sea-query 只提供 ESCAPE
子句声明，不提供 pattern 内容转义，内容转义由 Forge 这一层负责。

产品侧仍然负责字段白名单和索引设计。

`sort::SortOrder` 直接重导出 `aster_forge_api::SortOrder`。API 查询参数和 repository
排序必须复用同一个方向类型，不要在产品仓库或 DB crate 里再定义一套等价 enum，也不需要
编写 `API SortOrder -> DB SortOrder` 转换 facade：

```rust
use aster_forge_api::SortOrder;
use aster_forge_db::sort::order_by_column_with_id;

let query = order_by_column_with_id(query, UserColumn::CreatedAt, order, UserColumn::Id);
```

`pagination::fetch_offset_page` 的错误类型是调用方选择的 `E`，要求
`E: From<aster_forge_db::DbError>`。产品 repository 可以直接返回自己的错误类型，不需要为
每一个分页查询重复 `.map_err(...)`：

```rust
pub async fn list_users(db: &DatabaseConnection) -> ProductResult<(Vec<User>, u64)> {
    fetch_offset_page(db, UserEntity::find(), 50, 0).await
}
```

## 事务

模块：`transaction`

事务 helper 用来统一 SeaORM transaction 调用形式。Forge 负责事务机械行为，包括 begin、commit、rollback、rollback 失败日志和未显式结束事务的 guard 记录；业务规则仍然留在 repository/service 层。

手动事务边界直接返回 `DbError`：

```rust
let txn = aster_forge_db::transaction::begin(db).await?;
repository::write(&txn, input).await?;
aster_forge_db::transaction::commit(txn).await?;
```

产品仓库通过 `From<DbError>` 在 service/repository 返回边界转换错误；不要为了保留旧 import
路径再包装一套同名 transaction facade。调用点可以直接 import Forge 模块：

```rust
use aster_forge_db::transaction;
```

`with_transaction` 更适合 service/repository 组合调用。它允许回调返回产品错误类型 `E`，并只把 Forge 自己创建的事务边界错误映射成 `E`：

```rust
impl From<aster_forge_db::DbError> for AsterError {
    fn from(value: aster_forge_db::DbError) -> Self {
        AsterError::internal(value)
    }
}

use aster_forge_db::transaction;

let user = transaction::with_transaction(
    db,
    async |txn| -> Result<User, AsterError> {
        let user = user_repo::create(txn, input).await?;
        audit_repo::record_user_create(txn, user.id).await?;
        Ok(user)
    },
)
.await?;
```

错误边界规则：

- begin、commit、rollback 失败是 Forge DB 边界错误，来源类型是 `DbError`。
- `with_transaction` 的回调错误是产品或子系统错误，会原样返回，不会包装成 `DbError`。
- `with_transaction` 的错误类型需要满足 `E: From<DbError> + std::fmt::Display`，这样 commit/begin 失败可以进入产品错误边界，rollback 日志也能记录回调错误。
- 回调失败后 rollback 如果也失败，函数仍然返回原始回调错误，同时记录 rollback 失败日志；不要用 rollback 失败覆盖业务失败。
- `with_transaction` 不会自动重跑回调。产品需要重试时，应在完整事务边界外重新 begin，
  并只对 `database_error_kind(...)` 识别出的可重试错误重跑；已经执行外部对象存储、邮件、
  HTTP 或消息副作用的步骤必须放在事务重试边界之外。

需要把完整事务边界纳入有限重试时，使用 `with_transaction_retry`。它重新执行的是完整的
`begin -> callback -> commit`，回调失败后会先 rollback；产品通过 `should_retry` 明确选择
可重试的错误分类：

```rust
use aster_forge_db::retry::RetryConfig;
let config = RetryConfig::deadlock();
use aster_forge_db::{DatabaseErrorKind, DbError};
let file = transaction::with_transaction_retry(
    db,
    &config,
    |txn| Box::pin(async move { file_repo::create(txn, input).await }),
    |error: &DbError| {
        error.database_error_kind() == Some(DatabaseErrorKind::Deadlock)
    },
)
.await?;
```

未能确认事务已经回滚的 commit 错误会转换成 `DbError::CommitOutcomeUnknown`，表示服务端
可能已经提交；即使产品的 `should_retry` 接受该分类，Forge 也不会重放结果不确定的事务。
产品侧不应在这种错误上删除外部对象或无条件重放副作用。已确认回滚且被产品选为可重试
的 commit 错误在重试耗尽后仍以其原始分类返回，例如 MySQL `1213` 表示事务已回滚，不会
被误标为结果不确定。Forge 只提供事务机械层和错误分类，具体后端、重试种类、清理策略仍
由产品决定。

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
- 错误分类至少覆盖 MySQL `1213`/`1205`、PostgreSQL `40P01`/`40001`/`55P03`、SQLite
  `BUSY`/`LOCKED` 家族（含扩展结果码）、unique/FK 约束和非驱动/普通业务错误不误判。
- 事务重试测试要验证每次重试都会重新 begin，回调失败会 rollback，达到上限后返回最后一个
  数据库错误，并且非数据库错误不会进入重试。
- shutdown 测试要确认 `DbHandles::close()` 被调用或错误被记录。

## 参考项目

- AsterDrive：复杂 repository、跨数据库行为和 migration CLI。
- AsterYggdrasil：较轻服务启动/关闭链路和任务 repository token fence。

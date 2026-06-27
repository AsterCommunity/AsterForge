# 新项目接入指南

这页给新的 Aster 产品项目使用。目标不是让产品代码变成 Forge 的从属模块，而是让新项目从第一天就复用同一套运行时、数据库、任务、邮件、审计、缓存、配置和错误边界，避免 Drive、Yggdrasil 过去那种重复实现慢慢分叉。

## 推荐项目形状

产品仓库仍然拥有自己的 `AppState`、配置加载、业务 service、API route、实体和 migration。Forge 负责共享机械层：

```text
src/
  main.rs                    只负责启动入口和错误上报
  runtime/
    mod.rs                   组合产品 runtime state
    assembly.rs              初始化 database/cache/config/mail/task/http 资源
    startup.rs               产品 startup phase
  config/
    mod.rs                   产品 config key、默认值和 schema
  db/
    runtime.rs               database component 和 health check 接入
    repository/              产品业务查询
  services/
    audit_service/           产品 audit action/detail/presentation/runtime component
    mail_outbox_service/     产品模板 payload、渲染、审计 hook 和 runtime component
    task_service/            产品 task enum、payload/result、执行体和 runtime component
  api/
    routes/                  产品 API
```

不要把 Forge API 再包一层没有语义的 facade。只有需要映射产品错误、注入产品配置、记录产品指标或保留 typed enum 边界时，才在产品侧放一个薄边界。

## Cargo 依赖

新服务一般从这些 crate 开始：

```toml
[dependencies]
aster_forge_actix_middleware = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_actix_middleware", features = ["metrics"] }
aster_forge_api = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_api" }
aster_forge_audit = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_audit" }
aster_forge_cache = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_cache", features = ["memory", "runtime-component"] }
aster_forge_config = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_config" }
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_db", features = ["all"] }
aster_forge_logging = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_logging" }
aster_forge_mail = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_mail", features = ["persistence", "runtime-component"] }
aster_forge_metrics = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_metrics" }
aster_forge_panic = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_panic" }
aster_forge_runtime = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_runtime" }
aster_forge_tasks = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_tasks", features = ["runtime-component"] }
aster_forge_utils = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_utils" }
aster_forge_validation = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_validation" }
```

按需开启 feature：

- cache 后端：`aster_forge_cache` 默认只启用 `memory`；需要 Redis 时显式启用 `redis`，需要 runtime health 组件时显式启用 `runtime-component`。
- config 同步：`aster_forge_config` 的 `redis-pubsub`。
- task runtime：只用 retry、dedupe、steps、spec 时不需要 feature；需要 worker、scheduled task 或 runtime component 时启用 `aster_forge_tasks` 的 `runtime-component`。
- external auth：`aster_forge_external_auth` 的 `github`、`google`、`microsoft`、`qq` 等连接器。
- OpenAPI：产品自己的 `openapi` feature 再转发到 Forge crate。

Feature 边界要保持显式。默认 feature 只应该带最小可用内核，不能因为某个产品接入方便就把 Redis、SeaORM 表、runtime worker、mail drain 或 OpenAPI schema 静默拖进来。

| crate | 默认 feature | 常见显式 feature | 说明 |
| --- | --- | --- | --- |
| `aster_forge_actix_middleware` | 无 | `metrics` | CSRF、CORS、rate limit、request id 默认可用；HTTP metrics 需要显式启用。 |
| `aster_forge_cache` | `memory` | `redis`, `runtime-component` | Redis 后端显式启用；runtime health component 单独启用。 |
| `aster_forge_config` | 无 | `redis-pubsub`, `sea-orm`, `openapi` | 配置 reload 通知后端和数据库转换能力分开启用。 |
| `aster_forge_db` | 无 | `all`, `audit-log`, `mail-outbox`, `runtime-component`, `runtime-lease`, `scheduled-task`, `system-config` | 连接、transaction、pagination 等基础能力默认可用；共享表/store 按需启用。 |
| `aster_forge_mail` | 无 | `persistence`, `runtime-component`, `openapi` | sender/template 默认可用；SeaORM outbox model 和 runtime drain component 分开启用。 |
| `aster_forge_tasks` | 无 | `runtime`, `runtime-component`, `openapi` | retry、dedupe、steps、spec 默认可用；worker/scheduled runtime 和 component factory 分开启用。 |

## main.rs 目标形态

入口应该只表达“初始化资源，然后把组件交给 runtime”。不要在 `main.rs` 里手写 shutdown 顺序、task drain、mail drain、audit flush、db close。

```rust
#[tokio::main]
async fn main() -> std::io::Result<()> {
    aster_forge_panic::install_panic_hook("aster_product");
    aster_forge_logging::init_tracing(&logging_config())
        .map_err(to_io_error)?;

    let state = runtime::assembly::prepare_state().await.map_err(to_io_error)?;
    let state = actix_web::web::Data::new(state);

    aster_forge_runtime::AsterRuntime::builder()
        .component(runtime::http::http_component(http_config(), state.clone()))?
        .component(tasks::runtime::background_tasks_component(state.clone()))
        .component(services::mail_outbox_service::runtime::mail_runtime_component(state.get_ref()))
        .component(services::audit_service::runtime::audit_runtime_component(state.get_ref()))
        .component(db::runtime::database_component(state.get_ref().db_handles.clone()))
        .run()
        .await
        .map_err(to_io_error)
}
```

每个领域模块暴露自己的 component factory，入口不直接碰 root registry，也不手写 shutdown 顺序：

```rust
pub fn audit_runtime_component(
    state: &AppState,
) -> aster_forge_runtime::RuntimeComponentBundleRegistration<
    impl aster_forge_runtime::RuntimeComponentBundle,
> {
    let resources = AuditRuntimeResources::from_state(state);
    audit_component(resources)
}
```

组件依赖决定 shutdown 顺序。产品入口不应该再写“先停 task、再 drain mail、再写 audit、再关 db”这种手工流程。需要整个 Actix state 的组件 clone `web::Data<AppState>`；只需要 database、runtime config、sender 这类资源的组件从 `&AppState` 抽最小句柄，避免为了方便 clone 整个 state 或再包一层 `Arc`。

## 初始化顺序

推荐 `runtime::assembly::prepare()` 按这个顺序创建资源：

1. 读取文件配置和环境变量。
2. 初始化 logging、panic hook、metrics recorder。
3. 连接数据库，运行 migration。
4. 初始化 runtime config snapshot，并按需启动 config sync。
5. 创建 cache。
6. 创建 mail sender、mail template catalog 和 mail outbox dispatcher resources。
7. 初始化 audit manager。
8. 创建 task registry、background task workers、scheduled task runtime。
9. 构建 Actix app 和 HTTP server handle。
10. 返回 `RuntimeAssembly`，由 component graph 接管 shutdown。

startup 阶段可以用 `aster_forge_runtime::StartupCoordinator` 记录必需/可选 phase。资源创建仍然留产品侧，因为每个产品的配置来源、migration、内置数据和启动审计不同。

## 数据库和 migration

新项目不要复制 Forge 已经拥有的基础表结构：

```rust
manager
    .create_table(aster_forge_db::create_runtime_leases_table(
        manager.get_database_backend(),
    ))
    .await?;

manager
    .create_table(aster_forge_db::create_scheduled_tasks_table(
        manager.get_database_backend(),
    ))
    .await?;

manager
    .create_table(aster_forge_db::create_system_config_table(
        manager.get_database_backend(),
    ))
    .await?;

manager
    .create_table(aster_forge_db::create_mail_outbox_table(
        manager.get_database_backend(),
    ))
    .await?;

manager
    .create_table(aster_forge_db::create_audit_logs_table(
        manager.get_database_backend(),
    ))
    .await?;
```

索引也从 Forge builder 来：

```rust
manager
    .create_index(aster_forge_db::create_system_config_key_unique_index())
    .await?;

for index in aster_forge_db::create_audit_logs_base_indexes() {
    manager.create_index(index).await?;
}

for index in aster_forge_db::create_audit_logs_query_indexes() {
    manager.create_index(index).await?;
}
```

产品 migration 只维护产品实体表和产品专属索引。旧项目如果已经发布 migration，不要回改历史文件，用新的 migration 补齐字段宽度或索引。

## Audit 接入

产品侧只保留业务语义：

- `AuditAction` enum。
- `AuditEntityType` enum。
- detail struct 和脱敏规则。
- presentation。
- 哪些 action 需要记录、哪些 action 归入业务统计。

写入、批量写入、cursor query、count、distinct user count、delete 用 Forge：

```rust
aster_forge_db::create_audit_log_row(
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
.await?;
```

如果产品需要 typed `AuditAction` 给 API schema 使用，可以保留一个查询边界，把 `aster_forge_db::audit_log::Model.action: String` 转回产品 enum。不要再复制 SeaORM cursor 条件。

## Mail 接入

Forge 承接 outbox 状态机、dispatch loop、retry decision、sender trait、SMTP sender、memory sender、模板 catalog 和 shutdown drain component。产品侧保留：

- 模板 payload enum。
- 模板默认文案和本地化。
- 业务 URL 构造。
- 发信审计 hook。
- 哪些业务流程创建 outbox row。

新项目应该让 `mail_outbox` 表和 claim/retry/sent/failed 状态机走 `aster_forge_db::MailOutboxDbStore`，不要自己再写 claimable 查询。

## Task 接入

Forge 承接：

- task registry 宏。
- task step 状态。
- lease guard、heartbeat、claim/release 执行流程。
- background task shutdown component。
- scheduled task catalog 和多实例 due-run claim。

产品侧保留：

- task kind enum。
- task payload/result enum。
- task lane 策略。
- 具体执行体。
- 管理端 presentation。

如果任务需要定时触发，优先使用 Forge scheduled task catalog。`scheduled_tasks` 解决“哪个实例跑这次触发”，产品 `background_tasks.dedupe_key` 解决“这次触发最多写一条业务任务 row”。

## Config 和 Cache

产品侧定义配置 key、默认值、展示 schema、normalizer、权限和审计语义。Forge 负责通用 `system_config` 表/store、runtime config snapshot、reload diff、配置同步消息、展示脱敏 helper、审计字符串 helper 和 cache backend。

新项目的系统配置边界建议长这样：

```rust
aster_forge_config::define_config_registry! {
    pub static CONFIG_REGISTRY = [
        BRANDING_TITLE,
        PUBLIC_SITE_URL,
    ];
}

static SYSTEM_CONFIG_STORE: aster_forge_db::SystemConfigDbBinding =
    aster_forge_db::SystemConfigDbBinding::new(
        &CONFIG_REGISTRY,
        DEPRECATED_SYSTEM_CONFIG_KEYS,
    );

SYSTEM_CONFIG_STORE.ensure_defaults(writer_db).await?;
let row = SYSTEM_CONFIG_STORE.find_by_key(reader_db, key).await?;
```

产品 API DTO 可以继续留在产品仓库，但 stored row 到展示 row 的字段搬运和 value 脱敏不要再手写：

```rust
let presented = aster_forge_db::present_system_config(
    row,
    |error| tracing::warn!(%error, "invalid stored config value"),
);
```

新项目的 config sync 配置结构应该表达“notification backend / pubsub backend”，具体 Redis、RabbitMQ 或其他实现通过 feature 和 backend adapter 接入。

启动时直接用 Forge builder，不要在产品仓库再写 backend match：

```rust
let config_sync = aster_forge_config::build_config_sync_runtime(
    &config.config_sync,
    "aster_product",
)?;
```

配置写入成功并更新本进程 runtime snapshot 后，再发 reload 信号：

```rust
config_sync
    .publish_reload(
        [saved.key.clone()],
        aster_forge_config::ConfigNotificationSource::Api,
    )
    .await?;
```

后台订阅任务只提供“从权威存储 reload”的回调：

```rust
config_sync
    .run_reload_subscription(shutdown, move |message| {
        let state = state.clone();
        async move {
            tracing::debug!(keys = ?message.keys, "remote config reload");
            state.runtime_config().reload(state.reader_db()).await?;
            Ok(())
        }
    })
    .await?;
```

## 错误边界

产品错误类型应该实现这些转换：

```rust
impl From<aster_forge_db::DbError> for ProductError {
    fn from(value: aster_forge_db::DbError) -> Self {
        ProductError::database(value.to_string())
    }
}
```

但不要把业务错误包装成 Forge 错误。事务回调里的权限失败、校验失败、协议失败，继续返回产品错误；Forge 只负责 begin/commit/rollback 这类机械失败。

## 测试清单

新项目接入 Forge 后，至少补这些测试：

- runtime component graph：组件存在、依赖正确、shutdown phase 顺序正确。
- database migration：Forge 表 builder 能在目标 backend 上执行。
- audit：Forge 写入后产品 typed query/presentation 能读回。
- mail outbox：pending/retry/processing/sent/failed 状态转换和 shutdown drain。
- task：claim fence、heartbeat lost、shutdown release、scheduled task 多实例去重。
- config reload：本地 reload 和远端通知不会重复应用自身消息。
- cache：memory fallback、redis unavailable、delete/take/set-if-absent 原子语义。

不要只测 happy path。Forge 是公共地基，产品越多，边界 bug 的成本越高。

Forge 自身改 public API 或 feature split 后，至少跑一次 feature matrix smoke check，防止“默认 feature 可用”和“显式 feature 可用”被互相污染：

```bash
cargo check -p aster_forge_actix_middleware --no-default-features --all-targets
cargo check -p aster_forge_actix_middleware --no-default-features --features metrics --all-targets
cargo check -p aster_forge_cache --no-default-features --all-targets
cargo check -p aster_forge_cache --no-default-features --features memory --all-targets
cargo check -p aster_forge_cache --no-default-features --features redis --all-targets
cargo check -p aster_forge_cache --no-default-features --features runtime-component --all-targets
cargo check -p aster_forge_config --no-default-features --all-targets
cargo check -p aster_forge_config --no-default-features --features redis-pubsub --all-targets
cargo check -p aster_forge_db --no-default-features --all-targets
cargo check -p aster_forge_db --no-default-features --features all --all-targets
cargo check -p aster_forge_mail --no-default-features --all-targets
cargo check -p aster_forge_mail --no-default-features --features persistence --all-targets
cargo check -p aster_forge_mail --no-default-features --features runtime-component --all-targets
cargo check -p aster_forge_tasks --no-default-features --all-targets
cargo check -p aster_forge_tasks --no-default-features --features runtime --all-targets
cargo check -p aster_forge_tasks --no-default-features --features runtime-component --all-targets
cargo check --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features
cargo clippy --all-targets --all-features
```

## 接入检查

新产品完成第一版接入时，代码应该满足这些条件：

- `main.rs` 没有手写 shutdown 顺序。
- 产品 migration 没有复制 Forge 已经提供的基础表列定义。
- audit 写入、查询、统计、删除都走 Forge。
- mail outbox claim/retry/sent/failed 都走 Forge。
- scheduled task catalog 和 runtime lease 走 Forge。
- 产品侧 facade 都有明确边界职责，不存在只转发函数名的空壳。
- clippy 以 `-D warnings` 通过。

做到这些，新项目才算真正接上 Forge，而不是把 Forge 当成零散工具箱。 

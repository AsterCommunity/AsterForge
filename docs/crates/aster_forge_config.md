# aster_forge_config

`aster_forge_config` 提供 Aster 服务的运行时配置公共内核。它负责配置定义注册、结构化值转换、存储字符串验证、默认值 seed 记录生成、定义元数据覆盖、展示脱敏、审计字符串转换、运行时快照 reload diff，以及可选的 Redis reload pub/sub 通知。

它不负责管理 API、前端配置页、翻译文案、业务 normalizer 的具体规则，也不负责把配置变更写入审计。`system_config` 的共享 SeaORM entity、store、table builder 和 index builder 放在 `aster_forge_db::system_config`；产品仓库只需要把自己的配置项注册进 Forge registry，并在产品边界绑定 registry、权限和审计语义。

## 适用场景

- 产品有一组系统配置项，想用统一 registry 驱动默认值、验证、schema 和服务端 metadata。
- 配置值以字符串存储，但 API 需要支持 string array / enum set 这类结构化输入。
- 多个配置项之间存在依赖校验，例如 CORS `allow_credentials=true` 时不能允许 `*` origin。
- 进程内需要可 reload 的配置快照，并且热更新要尊重 `requires_restart`。
- 多进程部署中需要通过 Redis pub/sub 发送“配置已变更，请重新从数据库加载”的通知。

不适合放在这里的内容：

- 产品自己的配置 key 常量。
- 产品自己的 category 列表。
- 产品自己的 i18n key。
- 产品自己的业务实体、产品专属 migration / repository SQL。
- `system_config` 的 SeaORM entity、store 和 table builder；这些属于 `aster_forge_db::system_config`。
- 产品自己的审计详情、权限判断和管理 API envelope。
- 由配置派生出的产品运行时状态，例如邮件模板、审计 action set、Yggdrasil 策略对象。

## Cargo feature

```toml
[dependencies]
aster_forge_config = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `openapi`：在 debug + openapi 构建下为公共 API 类型派生 `utoipa::ToSchema`。
- `redis-pubsub`：启用 `RedisConfigChangeNotifier` 和 `RedisConfigReloadListener`，只用于跨进程 reload 通知，不表示配置值存储在 Redis。

```toml
aster_forge_config = {
  git = "https://github.com/AsterCommunity/AsterForge",
  features = ["redis-pubsub"]
}
```

默认不启用 Redis。单进程服务和测试可以使用 `InMemoryConfigNotifier`。

## 模块地图

主要 API 分组：

- `ConfigDefinition`：一个系统配置项的静态定义。
- `ConfigRegistry`：配置定义注册表，负责 key 查找、结构验证、normalizer 调用、API value 到 storage value 的预处理、default seed 记录生成和 metadata overlay。
- `ConfigValue`：API-facing 配置值，当前支持 scalar string 和 string array，并提供存储转换、展示脱敏和读取容错。
- `ConfigValueType`：存储值类型，包括 `string`、`multiline`、`string_array`、`string_enum`、`string_enum_set`、`number`、`boolean`。
- `parse_single_string_enum_selection()`：解析 `string_enum`，并兼容历史单元素 JSON array。
- `parse_string_enum_set_selection()` / `normalize_string_enum_set_selection()`：解析和规范化 `string_enum_set`。
- `parse_string_array_config_value()`：解析配置存储中的 JSON string array，让产品侧继续负责后续 URL、域名、枚举等业务规范化。
- `ConfigSource`：`system` / `custom` 来源。
- `ConfigVisibility`：`private` / `public` / `authenticated` 可见性。
- `present_config_value()`：API 展示用脱敏和 lossy 读取 helper。
- `config_value_audit_string()`：审计详情用脱敏和 lossy 字符串 helper。
- `StoredConfig`：产品数据库行转换后的 Forge 存储模型。
- `AsyncRuntimeConfig` / `AsyncConfigSnapshot`：基于 `tokio::sync::RwLock` 的 async 配置快照和 reload diff。
- `SyncRuntimeConfig` / `SyncConfigSnapshot`：基于标准库 `RwLock` 的同步热读配置快照。
- `read_positive_u64` / `read_positive_u32` / `read_bounded_u64` / `read_bounded_u8` / `read_positive_i32` / `read_positive_usize` / `read_non_negative_u64` / `read_finite_f32` / `read_bool`：产品无关的 runtime 配置读取 helper。
- `ConfigChangeNotifier`：reload 通知抽象。
- `ConfigReloadMessage`：跨进程 reload 信号载荷。

### avatar

主要 API：

- `aster_forge_config::avatar::DEFAULT_GRAVATAR_BASE_URL`
- `aster_forge_config::avatar::normalize_gravatar_base_url_config_value(value)`
- `aster_forge_config::avatar::gravatar_base_url_or_default(value)`

这组 helper 负责 Gravatar 配置值的写入校验和运行时默认值回退。`normalize_gravatar_base_url_config_value`
接受空值并回退默认地址，非空值必须是没有 query/fragment 的 HTTP(S) base URL。
`gravatar_base_url_or_default` 用于运行时读取，空值回退默认值并去掉尾部斜杠。

产品侧仍然负责头像来源策略、上传头像路由、缓存头、可用尺寸，以及是否启用 Gravatar。

## Registry 边界

产品侧应该用 registry 作为唯一的系统配置定义来源。不要再维护一份独立的 `match key { ... }` 分发表，也不要让默认 seed、schema API、更新验证分别读取不同的定义列表。

最小定义示例：

```rust
use aster_forge_config::{ConfigDefinition, ConfigValueType};

fn default_site_name() -> String {
    "Aster".to_string()
}

pub const SITE_NAME: ConfigDefinition = ConfigDefinition {
    key: "site_name",
    label_i18n_key: "settings_site_name_label",
    description_i18n_key: "settings_site_name_desc",
    value_type: ConfigValueType::String,
    default_fn: default_site_name,
    category: "site.branding",
    description: "Application name shown in public UI contexts",
    ..ConfigDefinition::private_system()
};

aster_forge_config::define_config_registry! {
    pub static CONFIG_REGISTRY = [
        SITE_NAME,
    ];
}
```

`ConfigDefinition::private_system()` 只是减少样板。每个配置项仍然应该显式声明稳定 key、值类型、默认值、category、前端 i18n key 和后端描述。

## Normalizer 注册

产品 normalizer 仍然放在产品仓库，因为这些规则通常包含业务语义。区别是 normalizer 应该挂在对应 `ConfigDefinition` 上，而不是集中写一个不断膨胀的大 `match`。

```rust
use aster_forge_config::{ConfigValueLookup, Result};

fn normalize_site_name(
    _lookup: &dyn ConfigValueLookup,
    _key: &str,
    value: &str,
) -> Result<String> {
    let normalized = value.trim().to_string();
    if normalized.len() > 80 {
        return Err(aster_forge_config::ConfigCoreError::invalid_value(
            "site_name must be at most 80 characters",
        ));
    }
    Ok(normalized)
}

pub const SITE_NAME: ConfigDefinition = ConfigDefinition {
    normalize_fn: Some(normalize_site_name),
    ..ConfigDefinition::private_system()
};
```

跨字段校验可以直接在 normalizer 里读取 `lookup`，也可以放进 `dependency_validator_fn`。推荐规则：

- 单字段清洗和枚举校验放 `normalize_fn`。
- 需要当前快照中其他 key 的约束放 `dependency_validator_fn`。
- 默认值 seed 依赖其他 key 时，被依赖 key 必须排在 registry 前面。

## 默认值 seed

`ConfigRegistry::default_seed_records()` 会按 registry 顺序生成 `ConfigSeedRecord`，并对每个默认值执行结构验证、normalizer 和 dependency validator。产品 repository 只需要把 seed record 转成本地 ActiveModel：

```rust
for seed in CONFIG_REGISTRY.default_seed_records()? {
    let active = build_system_active_model(seed);
    insert_if_missing(active).await?;
}
```

如果已有行存在，repository 可以用 `ConfigRegistry::apply_definition()` 的语义修复 system row 的类型、敏感标记、可见性、category 和 description，同时保留用户已经修改过的 value。

## 更新流水线

推荐的系统配置更新顺序：

1. 判断 key 是否允许直接更新。
2. 判断 system key 是否禁止修改 visibility。
3. 从 registry 查找 definition。
4. 用 `ConfigRegistry::value_to_storage_for_key()` 把 API 值转成可保存的 storage value；注册 key 会走声明的类型和 normalizer，custom key 默认按 string 保存。
5. 产品 repository upsert。
6. 用 `ConfigRegistry::apply_definition()` 覆盖 metadata。
7. 更新本进程 runtime snapshot。
8. 记录审计。
9. 多进程部署时发布 `ConfigReloadMessage`。

custom key 不在 registry 中，通常按产品策略固定为 string 类型，并由产品自己决定 visibility 和权限边界。

## API 值和展示脱敏

`ConfigValue` 是产品管理 API 可以直接复用的配置值类型：

```rust
use aster_forge_config::{ConfigValue, ConfigValueType};

let value = ConfigValue::from_storage(ConfigValueType::StringArray, r#"["a","b"]"#.to_string())?;
assert_eq!(value.to_storage_for_type(ConfigValueType::StringArray)?, r#"["a","b"]"#);
```

写入路径优先使用 `ConfigRegistry::value_to_storage_for_key()`，它会把 `ConfigValue` 按注册定义转成 storage string，并执行结构校验、normalizer 和 dependency validator：

```rust
let normalized_storage = CONFIG_REGISTRY.value_to_storage_for_key(
    runtime_config.as_ref(),
    key,
    &value,
)?;
```

如果产品正在实现更底层的 repository 或迁移逻辑，也可以单独使用 `ConfigValue::to_storage_for_type()`。

读取和列表展示路径可以使用 `present_config_value()`：

```rust
let value = aster_forge_config::present_config_value(
    value_type,
    stored_value,
    is_sensitive,
    |error| tracing::warn!(%error, "invalid stored config value"),
);
```

这个 helper 的边界很明确：

- `is_sensitive=true` 时统一返回 `ConfigValue::String("***REDACTED***")`；
- 非敏感值按 `ConfigValueType` 从存储字符串解析；
- 如果历史数据里有 malformed JSON array，展示路径返回对应类型的空值，避免整个配置页打不开；
- 它不吞写入错误，产品保存配置时仍然必须走严格校验。

产品侧仍然负责 `ConfigValue` 到本地数据库 enum 的转换、权限判断、审计上下文和配置变更后的业务动作。不要为了“适配旧代码”再包一层等价的本地 enum。

审计详情里需要记录配置值时，使用同一套脱敏规则：

```rust
let audit_value = aster_forge_config::config_value_audit_string(
    value_type,
    stored_value,
    is_sensitive,
    |error| tracing::warn!(%error, "invalid stored config value"),
);
```

如果产品使用 Forge DB store，`system_config` 表、entity 和 repository 机械层应该来自 `aster_forge_db::system_config`。产品 repository 只绑定 `CONFIG_REGISTRY`、deprecated keys、产品错误映射和权限语义，不再复制 SeaORM entity 或等价 CRUD。

### String Enum 兼容解析

新的 `string_enum` 配置值应该存成普通字符串，例如 `"quality"`。早期部分 Aster 配置页曾把单选枚举按
enum set 写成 `["quality"]`，产品 normalizer 如果要兼容这类历史值，可以用
`parse_single_string_enum_selection()`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewProfile {
    Fast,
    Quality,
}

fn parse_profile(value: &str) -> Option<PreviewProfile> {
    match value {
        "fast" => Some(PreviewProfile::Fast),
        "quality" => Some(PreviewProfile::Quality),
        _ => None,
    }
}

let profile = aster_forge_config::parse_single_string_enum_selection(
    r#"["quality"]"#,
    "preview_profile",
    "fast or quality",
    parse_profile,
)?;
assert_eq!(profile, PreviewProfile::Quality);
```

Forge 只处理“标量字符串或单元素字符串数组”这层兼容格式。具体枚举值、默认值、错误映射和业务含义仍然留在产品 normalizer。

### String Enum Set 解析

`string_enum_set` 配置值应该存成 JSON string array。产品侧仍然拥有具体 enum、默认集合和业务含义，Forge 只统一解析、unknown 检查、duplicate 检查，以及按权威 enum 顺序生成稳定存储值。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditAction {
    ConfigUpdate,
    UserLogin,
}

const ALL_ACTIONS: &[AuditAction] = &[AuditAction::ConfigUpdate, AuditAction::UserLogin];

fn parse_action(value: &str) -> Option<AuditAction> {
    match value {
        "config_update" => Some(AuditAction::ConfigUpdate),
        "user_login" => Some(AuditAction::UserLogin),
        _ => None,
    }
}

fn action_name(action: AuditAction) -> &'static str {
    match action {
        AuditAction::ConfigUpdate => "config_update",
        AuditAction::UserLogin => "user_login",
    }
}

let normalized = aster_forge_config::normalize_string_enum_set_selection(
    r#"["user_login","config_update"]"#,
    "audit_log_recorded_actions",
    "audit action",
    ALL_ACTIONS,
    parse_action,
    action_name,
)?;

assert_eq!(normalized, vec!["config_update", "user_login"]);
```

如果产品运行时需要保留请求顺序，例如构建诊断信息，可以使用 `parse_string_enum_set_selection()`。如果产品要写回配置存储，优先使用 `normalize_string_enum_set_selection()`，保证 API 输入顺序不会造成无意义 diff。

### String Array 解析

`string_array` 和 `string_enum_set` 的结构化校验由 `validate_storage_value()` 统一处理。如果产品 normalizer 需要先解析数组，再对每一项做 URL、域名、枚举或路径规范化，可以直接使用 `parse_string_array_config_value()`：

```rust
let values = aster_forge_config::parse_string_array_config_value(
    r#"["https://example.com/api", "https://mirror.example.com/api"]"#,
    "public_base_urls",
)?;

let normalized = values
    .into_iter()
    .map(|value| normalize_product_url(&value))
    .collect::<Result<Vec<_>>>()?;
```

Forge 只保证输入必须是 JSON string array。数组项是否允许为空、是否去重、是否要求 HTTPS、是否支持 wildcard，都应该继续由产品 normalizer 决定。

## Runtime Config

Forge 提供两条 runtime cache 路径：

- `AsyncRuntimeConfig` 使用 `tokio::sync::RwLock`，适合新接入的 async-first 服务。
- `SyncRuntimeConfig` 使用标准库 `RwLock`，适合 request handler、middleware、policy builder、task registry 这类需要同步热读配置的路径。

已有产品如果已经有大量同步读取配置的 helper，应优先接入 `SyncRuntimeConfig`，不要为了使用公共 runtime cache 把读路径强行改成 async。

两条路径都保持同一套 `requires_restart` 语义：如果 key 已经存在，之后收到 `requires_restart=true` 的热更新会被忽略，直到进程重启后通过完整 reload 加载新值。

产品如果保留本地 runtime，也应该保持同样语义，避免配置在不同服务中表现不一致。

### Runtime 读取 helper

很多产品模块会有 `operations.rs` 这类 helper，用产品 key 和默认值读取运行时配置。Forge 不应该拥有这些 key，但可以统一“怎么读”：

```rust
pub fn background_task_dispatch_interval_secs(runtime_config: &RuntimeConfig) -> u64 {
    aster_forge_config::read_positive_u64(
        runtime_config,
        BACKGROUND_TASK_DISPATCH_INTERVAL_SECS_KEY,
        DEFAULT_BACKGROUND_TASK_DISPATCH_INTERVAL_SECS,
    )
}
```

当前提供：

- `parse_bool_like_value(value)`：解析 `true/false`、`1/0`、`yes/no`、`on/off`。
- `parse_strict_bool_value(value)`：只解析 `true/false`，适合不允许历史 bool-like 输入的配置项。
- `parse_positive_u64(value)`：解析正整数。
- `parse_positive_u32(value)`：解析正 `u32`。
- `parse_non_negative_u64(value)`：解析非负整数。
- `parse_bounded_u64(value, min, max)`：解析闭区间内的 `u64`。
- `parse_bounded_u8(value, min, max)`：解析闭区间内的 `u8`。
- `parse_positive_i32(value)`：解析正 `i32`。
- `parse_finite_f32(value)`：解析有限 `f32`，拒绝 `NaN` 和无穷大。
- `normalize_bool_config_value(key, value)`：把 bool-like 输入规范化为 `true` / `false` 存储值。
- `normalize_strict_bool_config_value(key, value)`：把严格 `true/false` 输入规范化为存储值，拒绝 `yes/on/1` 等兼容写法。
- `normalize_positive_u64_config_value(key, value)`：用于配置更新时规范化正整数存储值。
- `normalize_non_negative_u64_config_value(key, value)`：用于配置更新时规范化非负整数存储值。
- `normalize_bounded_u64_config_value(key, value, min, max)`：用于配置更新时规范化闭区间内的 `u64` 存储值。
- `normalize_positive_u32_config_value(key, value)`：用于配置更新时规范化正 `u32` 存储值。
- `normalize_bounded_u8_config_value(key, value, min, max)`：用于配置更新时规范化闭区间内的 `u8` 存储值。
- `normalize_finite_f32_config_value(key, value)`：用于配置更新时规范化有限 `f32` 存储值。
- `read_positive_u64(lookup, key, default)`：非法或缺失时返回默认值并记录 warning。
- `read_positive_u32(lookup, key, default)`：非法或缺失时返回默认值并记录 warning。
- `read_non_negative_u64(lookup, key, default)`：非法或缺失时返回默认值并记录 warning。
- `read_bounded_u64(lookup, key, default, min, max)`：非法、缺失或超出闭区间时返回默认值并记录 warning。
- `read_bounded_u8(lookup, key, default, min, max)`：非法、缺失或超出闭区间时返回默认值并记录 warning。
- `read_positive_i32(lookup, key, default)`：非法或缺失时返回默认值并记录 warning。
- `read_positive_usize(lookup, key, default)`：非法、缺失或超过 `usize` 时返回默认值。
- `read_finite_f32(lookup, key, default)`：非法、缺失、`NaN` 或无穷大时返回默认值并记录 warning。
- `read_bool(lookup, key, default)`：非法或缺失时返回默认值并记录 warning。

这些 helper 接收 `ConfigValueLookup`，所以产品可以传 `RuntimeConfig`、`SyncConfigSnapshot`、`HashMap<String, String>`、`BTreeMap<String, String>`，或者一个 `Fn(&str) -> Option<String>` 闭包。闭包适合把“临时覆盖值 + runtime snapshot”叠在一起读取，例如管理 API 预览某个策略时不必写一次性 adapter struct。

产品侧仍然保留：

- key 常量；
- 默认值常量；
- 上限/下限 clamp；
- 复杂枚举或结构化配置解析；
- 配置项对应的业务含义。

## Reload 通知

`ConfigChangeNotifier` 只发布 reload 信号，不携带配置值：

```rust
let message = ConfigReloadMessage::new(
    "aster_yggdrasil",
    runtime_id,
    ["site_name"],
    ConfigNotificationSource::Api,
);
notifier.publish_reload(message).await?;
```

收到通知的进程应该从权威存储重新加载配置，而不是信任消息里的旧值。这个设计避免 pub/sub 丢包、乱序或 stale payload 直接覆盖本地快照。

`redis-pubsub` feature 下可以用：

- `RedisConfigChangeNotifier`
- `RedisConfigReloadListener`

如果产品需要完整订阅循环，优先使用：

- `CONFIG_SYNC_BACKEND_DISABLED`
- `CONFIG_SYNC_BACKEND_REDIS`
- `ConfigSyncConfig`
- `ConfigSyncRuntime`
- `build_config_sync_runtime(config, namespace)`
- `build_config_sync_runtime_with_runtime_id(config, namespace, runtime_id)`
- `decode_config_reload_transport_payload(payload)`
- `ConfigReloadObservation`
- `ConfigReloadObserver`
- `ConfigSyncConnectionObservation`
- `ConfigSyncConnectionObserver`
- `ConfigSyncConnectionState`
- `ConfigReloadWorkerConfig`
- `handle_config_reload_notification()`
- `run_config_reload_supervisor()`
- `run_config_reload_supervisor_with_observers()`
- `run_config_reload_worker()`
- `run_config_reload_worker_with_observer()`

常规产品接入应该直接持有 `ConfigSyncRuntime`。它把 namespace、进程级 runtime ID、backend notifier、reload 发布和订阅 worker 绑定在一起，产品侧只需要：

1. 启动时调用 `build_config_sync_runtime(&config.config_sync, "aster_product")`。
2. 本地配置写入成功后调用 `runtime.publish_reload(keys, ConfigNotificationSource::Api).await`。
3. 后台任务中优先调用 `runtime.run_reload_subscription_with_reconcile_and_observers(...)`。`reconcile` 从产品自己的权威存储全量加载 runtime snapshot，并清理所有派生缓存；`reload_callback` 继续处理真实 pub/sub 通知，并可以按 `message.keys` 做精细缓存失效。
4. 如果产品启用了 metrics，分别把 `ConfigReloadObservation` 和 `ConfigSyncConnectionObservation` 映射到产品 recorder。

产品侧如果需要构造静态配置或测试数据，优先使用 `CONFIG_SYNC_BACKEND_DISABLED` 和 `CONFIG_SYNC_BACKEND_REDIS`，不要在各仓库散写 backend 字符串。

`build_config_sync_runtime_with_runtime_id(...)` 只在产品已有稳定进程 ID 或测试需要固定 origin 过滤时使用。普通服务用 `build_config_sync_runtime(...)` 生成 process-level runtime ID。

底层 `ConfigReloadWorkerConfig`、`handle_config_reload_notification()`、`decode_config_reload_transport_payload()` 和 `run_config_reload_worker()` 仍然保留给特殊运行器、transport adapter 或测试使用；普通产品不应该自己拼 reload message、notifier subscribe、namespace 过滤或 backend match。

这组 API 负责统一静态 config shape、backend factory、runtime ID 生成、namespace/topic 默认值、过滤 namespace、忽略本进程发出的消息、调用产品传入的 reload 回调，并在订阅建立失败或运行中断线后使用有界指数退避和 50%-100% 抖动自动重连。重连等待、正在建立订阅和接收循环都响应 shutdown cancellation。

"断线"覆盖三类事件，supervisor 对它们统一走"观测 `disconnected` → 退避 → 重新订阅 → reconcile"的路径：

1. 订阅建立失败（例如 Redis 连接被拒）。
2. transport 流报错或结束（例如 Redis 连接被重置）。
3. 本地广播通道 lag：接收方处理跟不上、堆积事件超过通道容量被丢弃。

退避参数：250ms 起始、30s 上限、50%-100% 抖动；订阅稳定运行 30s 后失败计数重置。supervisor 只在 shutdown cancellation 时退出——单次 reload 失败、reconcile 失败、广播 lag、Redis 抖动都不会杀死 worker，避免一次故障让跨进程配置同步永久瘫痪。

`run_config_reload_supervisor*` 在每次订阅成功后调用一次 `reconcile`，因此启动竞态和 Pub/Sub 断线期间丢失的通知（包括 lag 丢弃的事件）都由权威数据库全量加载补齐。Redis Pub/Sub 不重放历史消息；reconcile 是最终状态修复机制。
Forge 不假设 transport 只能是 Redis；后续 RabbitMQ、NATS 或其他 broker 可以实现同一个 `ConfigChangeNotifier` 边界，并接入 `build_config_sync_runtime()` 的 backend 分支。

transport adapter 接收到原始 payload 时，应该先调用 `decode_config_reload_transport_payload(payload)`。解析失败只记录 warning 并继续监听，不应该让一个 malformed message 杀掉订阅 worker。

### Reload 观测

`ConfigReloadObservation` 是 reload worker 发出的低基数观测事件。它包含：

- `source`：当前为 `pubsub`。
- `decision`：`reloaded`、`ignored_namespace` 或 `ignored_origin`。
- `status`：`ok` 或 `error`。
- `changed_keys`：消息中声明的 key 数量，不包含 key 名称。
- `duration_seconds`：处理该通知的耗时。

Forge 不直接依赖 `aster_forge_metrics`，避免 config crate 反向绑定具体 metrics surface。产品侧可以传入实现
`ConfigReloadObserver` 的 recorder adapter，也可以直接传闭包：

```rust
let metrics = state.metrics().clone();
let reload_observer = move |observation: aster_forge_config::ConfigReloadObservation| {
    metrics.record_config_reload(
        observation.source,
        observation.decision.as_label(),
        observation.status,
        observation.changed_keys,
        observation.duration_seconds,
    );
};
let connection_metrics = state.metrics().clone();
let connection_observer =
    move |observation: aster_forge_config::ConfigSyncConnectionObservation| {
        connection_metrics.record_config_sync_connection(
            observation.state.as_label(),
            observation.reconnect_attempt,
            observation.backoff_seconds,
        );
    };

runtime
    .run_reload_subscription_with_reconcile_and_observers(
        shutdown,
        move || {
            let state = state.clone();
            async move {
                state.runtime_config().reload(state.writer_db()).await?;
                product_state.invalidate_all_derived_config_caches();
                Ok(())
            }
        },
        reload_callback,
        Some(&reload_observer),
        Some(&connection_observer),
    )
    .await?;
```

连接 observer 的 `state` 只有 `connected`、`disconnected`、`reconnecting` 和 `recovered` 四种稳定值；`reconnect_attempt` 与 `backoff_seconds` 是观测字段，不应作为高基数 label。首次连接和每次恢复都会执行 reconcile。若 reconcile 本身失败，supervisor 会记录 warning 并保持当前订阅，后续通知仍可继续触发 reload。

观测事件不会把配置 key 放进 label。key 名称只能出现在 debug 日志或审计详情里，不能进入 Prometheus label，否则多项目接入后 cardinality 会失控。

## 错误边界

Forge 返回 `ConfigCoreError`。产品侧应该在 service/API 边界映射为自己的错误类型和错误码：

- `InvalidValue` 通常映射为 400 validation error。
- `UnknownKey` 可以映射为 404 或 400，取决于产品 API 语义。
- `Store` 和 `Notification` 通常是内部错误或服务不可用。
- `Json` 通常是 validation error，除非发生在内部序列化路径。

不要把 Forge 的错误类型直接暴露成产品公开 API contract。

## 测试要求

接入时至少覆盖：

- registry key 唯一。
- 每个 category 都在产品允许列表中。
- deprecated key 不与 active definition 重叠。
- default seed 会经过 normalizer。
- system row metadata repair 不覆盖已有 value。
- API value 到 storage 的类型转换。
- custom config visibility 可修改，system config visibility 不可修改。
- 跨字段校验，例如 CORS `*` 与 credentials 的组合。
- `requires_restart` 热更新不覆盖进程内旧值。
- Redis reload 如果启用，只触发 reload，不直接携带值。

## 参考项目

- AsterYggdrasil：先接入 registry、结构验证和 default seed，保留同步 runtime snapshot，适合已有服务渐进迁移。
- AsterDrive：配置项更完整，适合后续对齐 schema、admin action 和多进程 reload 策略。

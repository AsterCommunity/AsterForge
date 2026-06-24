# aster_forge_config

`aster_forge_config` 提供 Aster 服务的运行时配置公共内核。它负责配置定义注册、结构化值转换、存储字符串验证、默认值 seed 记录生成、定义元数据覆盖、运行时快照 reload diff，以及可选的 Redis reload pub/sub 通知。

它不负责产品数据库实体、SeaORM migration、管理 API、前端配置页、翻译文案、业务 normalizer 的具体规则，也不负责把配置变更写入审计。产品仓库只需要把自己的配置项注册进 Forge registry，并在存储边界把本地 DB enum 与 Forge enum 做显式转换。

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
- 产品自己的数据库 ActiveModel / migration / repository SQL。
- 产品自己的审计详情、权限判断和管理 API envelope。
- 由配置派生出的产品运行时状态，例如邮件模板、审计 action set、Yggdrasil 策略对象。

## Cargo feature

```toml
[dependencies]
aster_forge_config = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `redis`：启用 `RedisConfigChangeNotifier` 和 `RedisConfigReloadListener`。

```toml
aster_forge_config = {
  git = "https://github.com/AsterCommunity/AsterForge",
  features = ["redis"]
}
```

默认不启用 Redis。单进程服务和测试可以使用 `InMemoryConfigNotifier`。

## 模块地图

主要 API 分组：

- `ConfigDefinition`：一个系统配置项的静态定义。
- `ConfigRegistry`：配置定义注册表，负责 key 查找、结构验证、normalizer 调用、default seed 记录生成和 metadata overlay。
- `ConfigValue`：API-facing 配置值，当前支持 scalar string 和 string array。
- `ConfigValueType`：存储值类型，包括 `string`、`multiline`、`string_array`、`string_enum`、`string_enum_set`、`number`、`boolean`。
- `ConfigSource`：`system` / `custom` 来源。
- `ConfigVisibility`：`private` / `public` / `authenticated` 可见性。
- `StoredConfig`：产品数据库行转换后的 Forge 存储模型。
- `RuntimeConfig` / `ConfigSnapshot`：进程内配置快照和 reload diff。
- `ConfigChangeNotifier`：reload 通知抽象。
- `ConfigReloadMessage`：跨进程 reload 信号载荷。

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
4. 用 `ConfigValue::to_storage_for_type()` 把 API 值转成存储字符串。
5. 用 `ConfigRegistry::normalize_value()` 执行结构验证、normalizer 和依赖校验。
6. 产品 repository upsert。
7. 用 `ConfigRegistry::apply_definition()` 覆盖 metadata。
8. 更新本进程 runtime snapshot。
9. 记录审计。
10. 多进程部署时发布 `ConfigReloadMessage`。

custom key 不在 registry 中，通常按产品策略固定为 string 类型，并由产品自己决定 visibility 和权限边界。

## RuntimeConfig

Forge 的 `RuntimeConfig` 使用 `tokio::sync::RwLock`，适合新接入的 async runtime。已有产品如果已经有大量同步读取配置的 helper，可以先只接入 registry 和 validation，保留本地同步 runtime snapshot；等读路径清理后再切换到 Forge runtime。

`requires_restart` 的语义由 Forge runtime 保证：如果 key 已经存在，之后收到 `requires_restart=true` 的热更新会被忽略，直到进程重启后通过完整 reload 加载新值。

产品如果保留本地 runtime，也应该保持同样语义，避免配置在不同服务中表现不一致。

## Reload 通知

`ConfigChangeNotifier` 只发布 reload 信号，不携带配置值：

```rust
let message = ConfigReloadMessage::new(
    "aster_yggdrasil",
    node_id,
    ["site_name"],
    ConfigNotificationSource::Api,
);
notifier.publish_reload(message).await?;
```

收到通知的进程应该从权威存储重新加载配置，而不是信任消息里的旧值。这个设计避免 pub/sub 丢包、乱序或 stale payload 直接覆盖本地快照。

Redis feature 下可以用：

- `RedisConfigChangeNotifier`
- `RedisConfigReloadListener`

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

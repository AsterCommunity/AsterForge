# aster_forge_cache

`aster_forge_cache` 提供共享缓存抽象和内存/Redis 后端构造。公共 API 是 byte-oriented 的 object-safe trait，并通过扩展 trait 提供 JSON 便利方法。

## 适用场景

- 产品需要一个 `Arc<dyn CacheBackend>`。
- 本地开发或 Redis 不可用时回退到内存缓存。
- 多个服务复用同样的 Redis 健康检查和 fallback 逻辑。
- 需要原子 `take`、`set if absent` 或 prefix invalidation。

不适合放在这里的内容：

- 产品缓存 key 命名规范。
- 缓存失效的业务策略。
- Session、token、验证码等产品语义。

## Cargo feature

默认 feature：

- `memory`
- `redis`

按需禁用：

```toml
aster_forge_cache = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["memory"] }
```

## 配置结构

`CacheConfig` 可以直接嵌进产品启动配置结构：

```rust
#[derive(serde::Deserialize)]
struct Config {
    #[serde(default)]
    cache: aster_forge_cache::CacheConfig,
}
```

```rust
let config = aster_forge_cache::CacheConfig {
    backend: "redis".to_string(),
    endpoint: "redis://127.0.0.1/".to_string(),
    default_ttl: 3600,
};
let cache = aster_forge_cache::create_cache(&config).await;
```

默认配置使用 `memory` backend、空 `endpoint` 和 3600 秒 TTL，和 Aster 产品配置文件里的历史默认值保持一致。

配置文件里应该使用 `endpoint`。为了不破坏已有部署，`CacheConfig` 反序列化时仍接受历史键 `redis_url` 作为 alias；Rust API 不保留 `redis_url` 字段。

`create_cache()` 返回 `Arc<dyn CacheBackend>`。Redis 初始化失败时会记录 warn 并回退到 memory backend。

## CacheBackend 边界

核心 trait：

- `get_bytes`
- `take_bytes`
- `set_bytes`
- `set_bytes_if_absent`
- `delete`
- `delete_many`
- `invalidate_prefix`
- `health_check`

`CacheExt` 提供 JSON 方法：

- `get<T>`
- `set<T>`
- `take<T>`

JSON 反序列化失败时返回 `None`，不会抛出产品错误。对关键业务状态不要只靠这个静默行为，产品侧应该有兜底或重建逻辑。

## Redis fallback

Redis backend 有健康检查和 fallback circuit。产品侧应该决定：

- 是否使用 Forge 标准 health check 暴露 Redis fallback。
- fallback 期间是否允许登录、验证码、任务调度等功能继续运行。
- 是否把 fallback 状态暴露到 metrics 或 admin overview。

Forge 只负责后端机制，不负责产品可用性策略。

## 健康检查

如果产品使用 `aster_forge_runtime::RuntimeComponentRegistry`，可以直接注册标准 cache diagnostics：

```rust
registry.register_bundle(aster_forge_cache::cache_health_component(
    config.cache.clone(),
    cache.clone(),
));
```

这个检查注册在 `cache` component 下，只进入 diagnostics scope，并且是 optional health check。行为：

- 配置 backend 和 active backend 不一致时返回 degraded，例如 Redis 初始化失败后回退 memory。
- active backend `health_check()` 成功时返回 healthy。
- active backend `health_check()` 失败时返回 unhealthy。
- report detail 会包含 `active_backend`，fallback 时还会包含 `configured_backend`。

产品仍然决定这个 diagnostics 结果是否影响 readiness、admin overview 或告警策略。普通产品不应该再重复写 cache backend fallback/ping 的 report 拼装。

新产品接入时优先使用 `cache_health_component(...)`，这样 health 也保持 component 化。低层 registry 注册函数是 crate 内部实现细节，不作为子系统 API 暴露。

## 测试要求

- memory backend 的 TTL、take、set-if-absent。
- Redis backend 可用时的读写和 prefix invalidation。
- Redis 不可用时回退 memory。
- 产品关键缓存 key 的命名和失效策略。

## 参考项目

- AsterDrive：适合参考复杂服务如何集中创建 cache，并在健康检查里展示 backend 状态。
- AsterYggdrasil：适合参考轻量服务如何只依赖 `CacheBackend` trait，避免业务层绑定 Redis。

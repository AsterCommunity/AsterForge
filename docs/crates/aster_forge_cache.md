# aster_forge_cache

`aster_forge_cache` 提供共享缓存抽象和内存/Redis 后端构造。公共 API 是 byte-oriented 的 object-safe trait，并通过扩展 trait 提供 JSON 便利方法。

## 适用场景

- 产品需要一个 `Arc<dyn CacheBackend>`。
- 本地开发或 Redis 不可用时回退到内存缓存。
- 多个服务复用同样的 Redis 健康检查和 fallback 逻辑。
- 需要原子 `take`、`set if absent` 或 prefix invalidation。
- 需要 Bloom filter 为 cache-aside 查询提供低成本的存在性预筛选。

不适合放在这里的内容：

- 产品缓存 key 命名规范。
- 缓存失效的业务策略。
- Session、token、验证码等产品语义。

## Cargo feature

默认 feature：

- `memory`

可选 feature：

- `bloom`：启用并发 Bloom filter 和原子流式重建 session。
- `redis`：启用 Redis backend。Redis backend 内部使用 memory fallback，所以会自动启用 `memory`。
- `runtime-component`：启用 `cache_health_component(...)` 等 runtime health 组件。

只使用内存缓存时：

```toml
aster_forge_cache = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["memory"] }
```

使用 Redis 和标准 health component 时：

```toml
aster_forge_cache = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["redis", "runtime-component"] }
```

同时使用 Bloom 和内存/Redis backend 时：

```toml
aster_forge_cache = { git = "https://github.com/AsterCommunity/AsterForge", default-features = false, features = ["bloom", "redis"] }
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

## TTL 契约

- `ttl_secs: None` 使用 backend 构造时的默认 TTL。
- `ttl_secs: Some(0)` 表示立即过期。`set_bytes` 等价于删除该 key（Redis 会拒绝 `SETEX 0`，backend 统一归一化为这个语义而不是发出非法命令）；`set_bytes_if_absent` 只报告 key 当前是否有存活值，无论结果如何都不会留下值。
- memory 和 Redis backend 对这两个边界的行为一致，单元测试和真实 Redis 容器测试共同锁定契约。

## Redis fallback

Redis backend 有健康检查和 fallback circuit。fallback circuit 只对可用性故障打开：连接 IO 错误、cluster 连接缺失，以及 `LOADING`/`TRYAGAIN`/`CLUSTERDOWN`/`MASTERDOWN`/`READONLY` 等瞬时服务端错误。确定性命令错误（如 WRONGTYPE、非法参数）只会记录 warn 并对单次操作走 fallback，不会把整个 backend 切到降级状态。

产品侧应该决定：

- 是否使用 Forge 标准 health check 暴露 Redis fallback。
- fallback 期间是否允许登录、验证码、任务调度等功能继续运行。
- 是否把 fallback 状态暴露到 metrics 或 admin overview。

Forge 只负责后端机制，不负责产品可用性策略。

## Bloom filter

`bloom` feature 提供：

- `aster_forge_cache::bloom::BloomConfig`
- `aster_forge_cache::bloom::BloomFilter`
- `aster_forge_cache::bloom::BloomRebuild`
- `aster_forge_cache::bloom::BloomError`

产品直接组合这些 primitive，并保留自己的查询顺序、key 规范、TTL、持久化来源和 false-positive 处理：

```rust
use std::sync::Arc;

use aster_forge_cache::bloom::{BloomConfig, BloomFilter};

let filter = Arc::new(BloomFilter::new(BloomConfig::new(10_000, 0.001))?);
filter.insert("short-code");

if filter.contains("short-code") {
    // Continue with the product-owned negative/object/storage lookup chain.
}
```

大数据集重建使用 `start_rebuild()` 创建 session，逐批调用 `insert_many()`，数据源完整结束后再 `commit()`。commit 前读取端继续使用旧 filter；重建期间的并发 `insert()` 会被记录并合并到新 filter。数据源报错时直接丢弃 session，旧 filter 保持可用。

同一 filter 只允许一个 active rebuild。重建期间再次调用 `start_rebuild()` 或 `clear()` 会返回 `BloomError::RebuildInProgress`，避免清空并发写入 buffer。

## 健康检查

需要 Cargo feature：`runtime-component`。

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
- TTL 边界：`Some(0)` 立即过期、1 秒 TTL 在服务端真实过期。
- Redis 错误分类：WRONGTYPE 等命令错误不触发 fallback circuit。
- 产品关键缓存 key 的命名和失效策略。

真实 Redis 集成测试使用 `aster_forge_test` 的共享容器（`tests/redis_container.rs`），运行：

```bash
cargo test -p aster_forge_cache --features redis
```

共享容器在多次运行之间保留数据，测试 key 必须带进程唯一前缀，crate 内的 `unique_key(...)` helper 已处理。

primitive benchmark 位于 `crates/aster_forge_cache/benches/cache.rs`，覆盖 Bloom check/insert/bulk insert 和 memory backend get/set/delete。只编译 benchmark：

```bash
cargo bench -p aster_forge_cache --features bloom,memory --no-run
```

## 参考项目

- AsterDrive：适合参考复杂服务如何集中创建 cache，并在健康检查里展示 backend 状态。
- AsterYggdrasil：适合参考轻量服务如何只依赖 `CacheBackend` trait，避免业务层绑定 Redis。

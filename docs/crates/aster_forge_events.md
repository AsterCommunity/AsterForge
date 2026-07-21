# aster_forge_events

`aster_forge_events` 提供产品无关的 transient event bus、transport 和 subscription runtime。它负责进程内 broadcast、可选共享 transport 的组合、单次 subscription 抽象、断线重连、指数退避、jitter、shutdown cancellation、连接状态观测，以及可选的 Redis Pub/Sub transport；产品负责 payload schema、namespace、origin/self-echo 过滤和权限语义。

## Cargo feature

默认不启用 transport。

- `redis`：启用 `RedisEventBus`。

```toml
aster_forge_events = { git = "https://github.com/AsterCommunity/AsterForge", features = ["redis"] }
```

## 使用边界

`TransientEventBus<T, R>` 默认提供本地 broadcast，并可组合一个产品选择的共享 transport：

```rust
let bus = TransientEventBus::<ProductEvent, RedisEventBus>::from_optional_transport(
    1024,
    redis_bus,
);
let mut local_events = bus.subscribe();
bus.publish_local(event)?;
```

Forge 不编码 `ProductEvent`，也不自动把本地事件转发到远端。产品在发布时编码远端 payload，在订阅 callback 中完成解码、origin 过滤后调用 `publish_local()`。因此单进程和测试始终复用同一个本地入口，远端 transport 关闭时不需要平行的 sender 字段。

```rust
let bus = RedisEventBus::from_url(redis_url, "my_product.events")?;
bus.publish(payload).await?;
bus.run_subscription(shutdown, Some(&observer), |payload| async move {
    // 产品层解码、校验 namespace/origin，并写入自己的本地 broadcast。
}).await;
```

需要接入其他 transport 或 typed notifier 时，实现 `EventSubscriptionSource`，再消费共享 supervisor 发出的顺序 update：

```rust
let (updates_tx, mut updates_rx) = tokio::sync::mpsc::channel(1);
let supervisor = supervise_event_subscription(
    source,
    EventReconnectPolicy::default(),
    shutdown.clone(),
    updates_tx,
);
tokio::pin!(supervisor);

// 顺序处理 Connection(Connected/Recovered) 和 Item(payload)。
```

`aster_forge_config` 的 Redis config-sync 已复用这套 runtime。它在 `Connected` / `Recovered` update 后执行权威数据库 reconcile，再处理后续 typed reload item；`aster_forge_events` 不理解配置 key 或数据库。

- Forge 不定义产品事件类型，不解析 workspace、user、team、storage 或 task 语义。
- `TransientEventBus` 的本地 broadcast 默认可用，不依赖 `redis` feature；容量为零时按最小容量 1 创建。
- Redis Pub/Sub 只承载瞬时刷新提示，不提供历史 replay 或 exactly-once。
- 订阅断线、stream 结束和连接失败都会进入带 jitter 的有界重连。
- `EventConnectionState` 提供 `connected`、`disconnected`、`reconnecting`、`recovered` 低基数观测。
- `EventReconnectPolicy`、`EventSubscriptionSource`、`EventSubscriptionUpdate` 和 `supervise_event_subscription()` 与具体 broker 无关。
- malformed 产品 payload 由产品 callback 记录并跳过，不能终止 transport worker。

## 测试

crate 测试覆盖退避边界、空 topic、真实 Redis 投递、运行中断线恢复、恢复后继续投递，以及 backoff 期间 shutdown。

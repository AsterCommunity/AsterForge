# aster_forge_alloc

`aster_forge_alloc` 提供共享的 allocator 统计能力。产品仍然负责选择 `#[global_allocator]`，Forge 只提供可复用的 tracking allocator 和统一的 `stats()` 读取入口。

## 适用场景

- 在 system allocator 构建中统计当前和峰值分配量。
- 在 jemalloc 构建中读取 jemalloc allocated/resident 统计。
- 给健康检查、metrics updater 或诊断接口提供统一内存数据。

不适合放在这里的内容：

- 产品级内存告警阈值。
- Prometheus metric 名称。
- 诊断 API 的响应结构。

## Cargo feature

```toml
[dependencies]
aster_forge_alloc = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `jemalloc`：启用 jemalloc 构建路径，但不读取 stats。
- `jemalloc-stats`：启用 jemalloc stats 读取，包含 `jemalloc`。

默认 feature 为空，走 system allocator tracking API。

## system allocator 接入

产品侧选择全局 allocator：

```rust
#[global_allocator]
static GLOBAL: aster_forge_alloc::TrackingAlloc = aster_forge_alloc::TrackingAlloc;
```

然后通过统一入口读取：

```rust
let (allocated_mib, peak_mib) = aster_forge_alloc::stats();
```

这组值适合展示为近似运行时诊断，不适合作为精确计费或资源限制依据。

## jemalloc 接入

启用 `jemalloc-stats` 后：

```toml
aster_forge_alloc = { git = "https://github.com/AsterCommunity/AsterForge", features = ["jemalloc-stats"] }
```

`stats()` 返回：

- allocated MiB
- resident MiB

如果只启用 `jemalloc` 而不启用 `jemalloc-stats`，`stats()` 返回 `(0.0, 0.0)`。这是刻意设计，避免在不需要 stats 的构建里强行依赖 jemalloc ctl。

## 接入边界

产品仓库应该自己决定：

- 是否使用 jemalloc。
- 是否把 stats 注册到 metrics。
- stats 读取失败时是否降级、告警或忽略。
- 是否暴露给管理员 API。

Forge 不做这些产品决策。

## 测试要求

- system allocator 构建中 `stats()` 返回非负数。
- jemalloc 无 stats feature 时调用方能接受 `(0.0, 0.0)`。
- 如果产品把 stats 接到 metrics updater，需要测试 updater 不会因为读取失败中断。

## 参考项目

- AsterDrive：可对照 developer docs 中 jemalloc profiling 的诊断思路。
- AsterYggdrasil：适合参考轻量服务如何把内存信息接入健康或指标层。

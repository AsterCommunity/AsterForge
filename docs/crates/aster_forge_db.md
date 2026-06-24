# aster_forge_db

`aster_forge_db` 提供 SeaORM 相关的共享基础设施：数据库连接、连接关闭、查询重试、分页构造、搜索 query 处理、排序 helper 和事务封装。

## 适用场景

- 多数据库 URL 连接和连接池配置。
- `DbHandles` 管理读写连接并在 shutdown 时关闭。
- transient 数据库错误重试。
- SeaORM 查询分页、排序、全文搜索条件复用。
- 事务 helper。

不适合放在这里的内容：

- 产品实体和 migration。
- repository 业务查询。
- 权限过滤。
- 数据库配置来源和加密存储。

## Cargo 接入

```toml
[dependencies]
aster_forge_db = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。SeaORM backend feature 由 Forge workspace 统一启用。

## 连接与关闭

核心类型：

- `DatabaseConfig`
- `DbHandles`
- `DbMetricsRecorder`
- `NoopDbMetrics`

典型接入：

```rust
let db = aster_forge_db::connect_with_metrics(&config.database, metrics.clone()).await?;
let db_handles = aster_forge_db::DbHandles::single(db);
```

shutdown 时：

```rust
db_handles.close().await?;
```

产品侧应把 `DbError` 映射到自己的启动错误或内部错误。不要吞掉 close 错误，至少要记录。

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

事务 helper 用来统一 SeaORM transaction 调用形式。业务规则仍然留在 repository/service 层。

## 测试要求

- SQLite 内存库至少覆盖连接、事务和基础 query helper。
- 产品 repository 要覆盖 token fence、状态转换和并发保护。
- shutdown 测试要确认 `DbHandles::close()` 被调用或错误被记录。

## 参考项目

- AsterDrive：复杂 repository、跨数据库行为和 migration CLI。
- AsterYggdrasil：较轻服务启动/关闭链路和任务 repository token fence。

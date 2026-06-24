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

# aster_forge_db_migration

`aster_forge_db_migration` 提供产品无关的 SeaORM migration 并发协调、执行包装和 schema helper。产品仓库继续拥有 migration 名称与顺序、历史兼容策略、业务表定义、数据回填和应用 schema 判定。

## 适用场景

- 多个服务实例同时启动时，串行执行数据库 migration。
- 在同一个跨进程锁内运行标准 `MigratorTrait`，或运行产品自有的 migration history callback。
- 在 PostgreSQL、MySQL 和 SQLite 上幂等删除索引，以及按 MySQL 语义重命名索引。
- 构建通用 bigint 自增主键、UTC 时间列和 JSON text 分阶段迁移列。
- 构建 SQLite FTS5、PostgreSQL trigram 和 MySQL ngram full-text 的 migration 语句。

这个 crate 不接管产品 migration 列表、产品 entity、历史版本识别、升级策略和业务数据回填。

## Cargo 接入

```toml
[dependencies]
aster_forge_db_migration = {
    git = "https://github.com/AsterCommunity/AsterForge",
    package = "aster_forge_db_migration",
}
```

## Migration 并发协调

迁移历史没有额外兼容分支的产品，可以直接运行 migrator：

```rust
let options = aster_forge_db_migration::MigrationLockOptions::new(
    "aster_product:database_migrations",
);
aster_forge_db_migration::run_migrator_with_lock::<migration::Migrator>(
    writer_db,
    &options,
    None,
)
.await?;
```

需要自行检查历史 migration 的产品，保留本地策略并传入 callback：

```rust
aster_forge_db_migration::with_migration_lock(writer_db, &options, |connection| {
    Box::pin(apply_supported_product_migrations(connection))
})
.await?;
```

PostgreSQL 使用 transaction-scoped `pg_advisory_xact_lock`。MySQL 在同一条连接上执行 `GET_LOCK` 和 `RELEASE_LOCK`。SQLite 不额外申请进程级锁，但 callback 仍在事务内执行。

锁标识是持久化部署契约。已经有 migration lock 的产品接入时必须保留原 PostgreSQL advisory key 和 MySQL namespace，避免滚动升级期间新旧实例使用不同锁：

```rust
let options = aster_forge_db_migration::MigrationLockOptions::new(
    "aster_product:database_migrations",
)
.with_postgres_advisory_key(EXISTING_PRODUCT_LOCK_KEY)
.with_mysql_timeout_seconds(300);
```

MySQL namespace 必须为 1 到 64 字节且不得包含 NUL。timeout 必须能转换为有符号 64 位整数。非法配置会在打开事务前返回 `DbErr`。

## Schema helper

Migration 专用 helper 由 crate 直接导出：

```rust
aster_forge_db_migration::drop_index_if_exists(
    manager.get_connection(),
    "background_tasks",
    "idx_background_tasks_due",
)
.await?;

manager
    .create_table(
        Table::create()
            .table(Example::Table)
            .col(aster_forge_db_migration::big_integer_primary_key(Example::Id))
            .col(
                aster_forge_db_migration::utc_date_time_column(
                    manager,
                    Example::CreatedAt,
                )
                .not_null(),
            )
            .to_owned(),
    )
    .await?;
```

`big_integer_primary_key` 统一构建 signed 64-bit、非空、自增主键。`utc_date_time_column` 在 MySQL 使用 `datetime(6)`，在 PostgreSQL 和 SQLite 使用 SeaORM 对应的带时区时间表示。

`json_text_column_for_final_schema` 会避开 MySQL 不兼容的 TEXT 默认值，同时在 PostgreSQL 和 SQLite 上保留 `{}` 默认值。`nullable_json_text_column_for_backfill` 用于先加 nullable 列、完成数据回填、再收紧 `NOT NULL` 的分阶段 migration。

`drop_index_if_exists` 在 PostgreSQL 和 SQLite 使用原生 `IF EXISTS`；MySQL 会先查询 `information_schema.statistics`。`rename_mysql_index_if_exists` 仅用于 MySQL，并在源索引不存在或目标索引已经存在时保持幂等。

## 搜索加速 helper

SQLite FTS5 同步、PostgreSQL trigram extension/index 和 MySQL ngram full-text builder 也属于 migration 机械层。产品只保留具体表名、字段名、索引名和 trigger 名：

```rust
let config = aster_forge_db_migration::SqliteFtsConfig {
    virtual_table: "documents_search_fts",
    source_table: "documents",
    columns: &["name", "description"],
    insert_trigger: "trg_documents_search_fts_ai",
    delete_trigger: "trg_documents_search_fts_ad",
    update_trigger: "trg_documents_search_fts_au",
};
let statements = aster_forge_db_migration::sqlite_fts_up_statements(&config)?;
aster_forge_db_migration::execute_sqlite_statements(
    manager,
    statements,
    "create document search acceleration",
)
.await?;
```

生成 raw SQL 的 builder 会拒绝空字段列表，以及包含 ASCII 字母、数字和下划线以外字符的 identifier，避免把不受约束的字符串直接拼入 migration SQL。

## 错误边界

Callback 失败且清理成功时，原始 callback `DbErr` 会保持不变。锁申请、锁释放、commit 和 rollback 失败属于基础设施错误，由产品启动边界映射成产品日志或退出错误。Callback 与 MySQL lock release 同时失败时，返回错误会同时保留两部分信息。

产品不需要再包一层只做透传的 migration helper。只有注入稳定 lock key、选择 migration history 分支、记录产品指标或映射产品启动错误时，才保留产品侧 adapter。

## 测试

```bash
cargo test -p aster_forge_db_migration --lib
cargo test -p aster_forge_db_migration --test coordination_containers
cargo clippy -p aster_forge_db_migration --all-targets -- -D warnings
```

单元测试覆盖配置边界、SQLite commit/rollback、跨数据库 SQL 生成、索引幂等行为和非法 identifier。容器测试使用真实 PostgreSQL 与 MySQL，验证相同 namespace 的并发串行化，以及 callback 失败后锁可以继续获取。产品仓库仍需通过本地 Cargo patch 保留 migration history、fresh database 和滚动升级兼容测试。

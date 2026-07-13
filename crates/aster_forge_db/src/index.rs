use sea_orm::sea_query::{Alias, Index, IndexDropStatement};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

/// Drops an index when it exists across all supported database backends.
///
/// MySQL does not support `DROP INDEX IF EXISTS`, so this helper checks
/// `information_schema.statistics` before issuing the backend-specific drop.
pub async fn drop_index_if_exists<C>(
    db: &C,
    table_name: &str,
    index_name: &str,
) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    let backend = db.get_database_backend();
    if backend == DatabaseBackend::MySql && !mysql_index_exists(db, table_name, index_name).await? {
        return Ok(());
    }

    db.execute(&drop_index_for_backend(backend, table_name, index_name))
        .await?;
    Ok(())
}

/// Renames a MySQL index when the source exists and the target does not.
///
/// This is useful when a table or business term is renamed while preserving
/// the physical index definition. Calling it repeatedly is safe.
pub async fn rename_mysql_index_if_exists<C>(
    db: &C,
    table_name: &str,
    old_index_name: &str,
    new_index_name: &str,
) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    validate_mysql_identifier(table_name)?;
    validate_mysql_identifier(old_index_name)?;
    validate_mysql_identifier(new_index_name)?;

    if db.get_database_backend() != DatabaseBackend::MySql {
        return Err(DbErr::Custom(
            "rename_mysql_index_if_exists requires a MySQL connection".to_string(),
        ));
    }

    if !mysql_index_exists(db, table_name, old_index_name).await?
        || mysql_index_exists(db, table_name, new_index_name).await?
    {
        return Ok(());
    }

    db.execute_unprepared(&format!(
        "ALTER TABLE `{table_name}` RENAME INDEX `{old_index_name}` TO `{new_index_name}`"
    ))
    .await?;
    Ok(())
}

fn drop_index_for_backend(
    backend: DatabaseBackend,
    table_name: &str,
    index_name: &str,
) -> IndexDropStatement {
    let mut statement = Index::drop();
    statement
        .name(index_name.to_owned())
        .table(Alias::new(table_name));
    if backend != DatabaseBackend::MySql {
        statement.if_exists();
    }
    statement.to_owned()
}

async fn mysql_index_exists<C>(db: &C, table_name: &str, index_name: &str) -> Result<bool, DbErr>
where
    C: ConnectionTrait,
{
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT 1 FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = ? AND index_name = ? LIMIT 1",
            [table_name.into(), index_name.into()],
        ))
        .await?;
    Ok(row.is_some())
}

fn validate_mysql_identifier(identifier: &str) -> Result<(), DbErr> {
    if !identifier.is_empty()
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Ok(());
    }

    Err(DbErr::Custom(format!(
        "invalid MySQL migration identifier: {identifier:?}"
    )))
}

#[cfg(test)]
mod tests {
    use sea_orm::sea_query::{MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend};

    use super::{
        drop_index_for_backend, drop_index_if_exists, rename_mysql_index_if_exists,
        validate_mysql_identifier,
    };

    #[test]
    fn drop_index_sql_respects_backend_capabilities() {
        let mysql = drop_index_for_backend(DatabaseBackend::MySql, "example_table", "idx_example")
            .to_string(MysqlQueryBuilder);
        assert_eq!(mysql, "DROP INDEX `idx_example` ON `example_table`");

        let postgres =
            drop_index_for_backend(DatabaseBackend::Postgres, "example_table", "idx_example")
                .to_string(PostgresQueryBuilder);
        assert_eq!(postgres, "DROP INDEX IF EXISTS \"idx_example\"");

        let sqlite =
            drop_index_for_backend(DatabaseBackend::Sqlite, "example_table", "idx_example")
                .to_string(SqliteQueryBuilder);
        assert_eq!(sqlite, "DROP INDEX IF EXISTS \"idx_example\"");
    }

    #[test]
    fn mysql_identifier_validation_rejects_raw_sql_fragments() {
        assert!(validate_mysql_identifier("idx_example_2026").is_ok());
        assert!(validate_mysql_identifier("").is_err());
        assert!(validate_mysql_identifier("idx-example").is_err());
        assert!(validate_mysql_identifier("idx` DROP TABLE users").is_err());
    }

    #[tokio::test]
    async fn drop_index_if_exists_is_idempotent_on_sqlite() {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("SQLite migration helper test database should connect");
        db.execute_unprepared("CREATE TABLE example_table (id INTEGER PRIMARY KEY)")
            .await
            .expect("example table should be created");
        db.execute_unprepared("CREATE INDEX idx_example ON example_table (id)")
            .await
            .expect("example index should be created");

        drop_index_if_exists(&db, "example_table", "idx_example")
            .await
            .expect("existing index should be dropped");
        drop_index_if_exists(&db, "example_table", "idx_example")
            .await
            .expect("missing index should be ignored");
    }

    #[tokio::test]
    async fn mysql_index_rename_rejects_non_mysql_connections() {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("SQLite migration helper test database should connect");
        let error = rename_mysql_index_if_exists(
            &db,
            "example_table",
            "idx_example_old",
            "idx_example_new",
        )
        .await
        .expect_err("MySQL-only index rename should reject SQLite");

        assert!(error.to_string().contains("requires a MySQL connection"));
    }
}

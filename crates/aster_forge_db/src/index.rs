use sea_orm::DatabaseBackend;
use sea_orm::sea_query::{Alias, Index, IndexDropStatement};

pub(crate) fn drop_index_for_backend(
    backend: DatabaseBackend,
    table: Alias,
    name: &'static str,
) -> IndexDropStatement {
    let mut statement = Index::drop();
    statement.name(name).table(table);
    if backend != DatabaseBackend::MySql {
        statement.if_exists();
    }
    statement.to_owned()
}

#[cfg(test)]
mod tests {
    use sea_orm::DatabaseBackend;
    use sea_orm::sea_query::{Alias, MysqlQueryBuilder, PostgresQueryBuilder, SqliteQueryBuilder};

    use super::drop_index_for_backend;

    #[test]
    fn drop_index_sql_respects_backend_capabilities() {
        let mysql = drop_index_for_backend(
            DatabaseBackend::MySql,
            Alias::new("example_table"),
            "idx_example",
        )
        .to_string(MysqlQueryBuilder);
        assert_eq!(mysql, "DROP INDEX `idx_example` ON `example_table`");

        let postgres = drop_index_for_backend(
            DatabaseBackend::Postgres,
            Alias::new("example_table"),
            "idx_example",
        )
        .to_string(PostgresQueryBuilder);
        assert_eq!(postgres, "DROP INDEX IF EXISTS \"idx_example\"");

        let sqlite = drop_index_for_backend(
            DatabaseBackend::Sqlite,
            Alias::new("example_table"),
            "idx_example",
        )
        .to_string(SqliteQueryBuilder);
        assert_eq!(sqlite, "DROP INDEX IF EXISTS \"idx_example\"");
    }
}

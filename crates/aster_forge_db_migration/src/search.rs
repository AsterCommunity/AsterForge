use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;
use sea_orm_migration::sea_query::{
    Alias, IntoIndexColumn, PostgresQueryBuilder, extension::postgres::Extension,
};

/// `SQLite` FTS5 virtual table and synchronization-trigger names used by one migration.
pub struct SqliteFtsConfig<'a> {
    pub virtual_table: &'a str,
    pub source_table: &'a str,
    pub columns: &'a [&'a str],
    pub insert_trigger: &'a str,
    pub delete_trigger: &'a str,
    pub update_trigger: &'a str,
}

/// Builds the `SQLite` FTS5 table, backfill, and synchronization trigger statements.
///
/// # Errors
///
/// Returns an error when the configuration contains an invalid identifier or no indexed columns.
pub fn sqlite_fts_up_statements(config: &SqliteFtsConfig<'_>) -> Result<Vec<String>, DbErr> {
    validate_sqlite_fts_config(config)?;
    let column_list = config.columns.join(", ");
    let new_values = config
        .columns
        .iter()
        .map(|column| format!("new.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let update_assignments = config
        .columns
        .iter()
        .map(|column| format!("{column} = new.{column}"))
        .collect::<Vec<_>>()
        .join(", ");

    Ok(vec![
        format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING fts5({}, tokenize='trigram')",
            config.virtual_table, column_list
        ),
        format!("DELETE FROM {}", config.virtual_table),
        format!(
            "INSERT INTO {}(rowid, {}) SELECT id, {} FROM {}",
            config.virtual_table, column_list, column_list, config.source_table
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS {} AFTER INSERT ON {} BEGIN \
             INSERT INTO {}(rowid, {}) VALUES (new.id, {}); END",
            config.insert_trigger,
            config.source_table,
            config.virtual_table,
            column_list,
            new_values,
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS {} AFTER DELETE ON {} BEGIN \
             DELETE FROM {} WHERE rowid = old.id; END",
            config.delete_trigger, config.source_table, config.virtual_table
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS {} AFTER UPDATE OF {} ON {} BEGIN \
             UPDATE {} SET {} WHERE rowid = new.id; END",
            config.update_trigger,
            column_list,
            config.source_table,
            config.virtual_table,
            update_assignments,
        ),
    ])
}

/// Builds the `SQLite` statements that remove FTS synchronization and the virtual table.
///
/// # Errors
///
/// Returns an error when the configuration contains an invalid identifier or no indexed columns.
pub fn sqlite_fts_down_statements(config: &SqliteFtsConfig<'_>) -> Result<Vec<String>, DbErr> {
    validate_sqlite_fts_config(config)?;
    Ok(vec![
        format!("DROP TRIGGER IF EXISTS {}", config.insert_trigger),
        format!("DROP TRIGGER IF EXISTS {}", config.delete_trigger),
        format!("DROP TRIGGER IF EXISTS {}", config.update_trigger),
        format!("DROP TABLE IF EXISTS {}", config.virtual_table),
    ])
}

/// Executes generated `SQLite` migration statements in order with caller-provided error context.
///
/// # Errors
///
/// Returns an error containing `error_context` when any generated SQL statement fails.
pub async fn execute_sqlite_statements(
    manager: &SchemaManager<'_>,
    statements: impl IntoIterator<Item = String>,
    error_context: &str,
) -> Result<(), DbErr> {
    let db = manager.get_connection();
    for sql in statements {
        db.execute_unprepared(&sql)
            .await
            .map_err(|error| DbErr::Custom(format!("{error_context}: {error}")))?;
    }
    Ok(())
}

/// Creates a `PostgreSQL` extension when it is not already installed.
///
/// # Errors
///
/// Returns an error when the generated extension statement fails.
pub async fn ensure_postgres_extension(
    manager: &SchemaManager<'_>,
    extension_name: &str,
) -> Result<(), DbErr> {
    let sql = Extension::create()
        .name(extension_name)
        .if_not_exists()
        .to_string(PostgresQueryBuilder);
    manager.get_connection().execute_unprepared(&sql).await?;
    Ok(())
}

/// Builds a `PostgreSQL` GIN trigram index statement.
#[must_use]
pub fn postgres_trigram_index(
    index_name: &str,
    table_name: &str,
    column_name: &str,
) -> IndexCreateStatement {
    Index::create()
        .if_not_exists()
        .name(index_name)
        .table(Alias::new(table_name))
        .full_text()
        .col(
            Alias::new(column_name)
                .into_index_column()
                .with_operator_class("gin_trgm_ops"),
        )
        .to_owned()
}

/// Builds a portable `DROP INDEX IF EXISTS` statement for `PostgreSQL` migrations.
#[must_use]
pub fn postgres_drop_index(index_name: &str) -> IndexDropStatement {
    Index::drop().if_exists().name(index_name).to_owned()
}

/// Builds `MySQL`'s ngram-backed full-text index statement.
///
/// # Errors
///
/// Returns an error when the index, table, or column identifiers are invalid, or when `columns` is
/// empty.
pub fn mysql_fulltext_index_sql(
    index_name: &str,
    table_name: &str,
    columns: &[&str],
) -> Result<String, DbErr> {
    validate_identifier(index_name)?;
    validate_identifier(table_name)?;
    validate_columns(columns)?;
    Ok(format!(
        "CREATE FULLTEXT INDEX {index_name} ON {table_name} ({}) WITH PARSER ngram",
        columns.join(", ")
    ))
}

fn validate_sqlite_fts_config(config: &SqliteFtsConfig<'_>) -> Result<(), DbErr> {
    for identifier in [
        config.virtual_table,
        config.source_table,
        config.insert_trigger,
        config.delete_trigger,
        config.update_trigger,
    ] {
        validate_identifier(identifier)?;
    }
    validate_columns(config.columns)
}

fn validate_columns(columns: &[&str]) -> Result<(), DbErr> {
    if columns.is_empty() {
        return Err(DbErr::Custom(
            "migration search column list must not be empty".to_string(),
        ));
    }
    for column in columns {
        validate_identifier(column)?;
    }
    Ok(())
}

fn validate_identifier(identifier: &str) -> Result<(), DbErr> {
    if !identifier.is_empty()
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Ok(());
    }
    Err(DbErr::Custom(format!(
        "invalid migration search identifier: {identifier:?}"
    )))
}

#[cfg(test)]
mod tests {
    use sea_orm_migration::sea_query::PostgresQueryBuilder;

    use super::*;

    fn config<'a>(columns: &'a [&'a str]) -> SqliteFtsConfig<'a> {
        SqliteFtsConfig {
            virtual_table: "files_name_fts",
            source_table: "files",
            columns,
            insert_trigger: "files_name_fts_ai",
            delete_trigger: "files_name_fts_ad",
            update_trigger: "files_name_fts_au",
        }
    }

    #[test]
    fn sqlite_fts_statements_cover_create_backfill_triggers_and_drop() {
        let columns = ["name", "description"];
        let up = sqlite_fts_up_statements(&config(&columns)).unwrap();
        assert_eq!(up.len(), 6);
        assert!(up[0].contains("USING fts5(name, description, tokenize='trigram')"));
        assert!(up[2].contains("SELECT id, name, description FROM files"));
        assert!(up[3].contains("VALUES (new.id, new.name, new.description)"));
        assert!(up[5].contains("name = new.name, description = new.description"));

        let down = sqlite_fts_down_statements(&config(&columns)).unwrap();
        assert_eq!(down.len(), 4);
        assert_eq!(down[3], "DROP TABLE IF EXISTS files_name_fts");
    }

    #[test]
    fn search_statement_builders_reject_empty_and_unsafe_identifiers() {
        assert!(sqlite_fts_up_statements(&config(&[])).is_err());
        assert!(mysql_fulltext_index_sql("idx-name", "files", &["name"]).is_err());
        assert!(
            mysql_fulltext_index_sql("idx_name", "files; DROP TABLE users", &["name"]).is_err()
        );
        assert!(mysql_fulltext_index_sql("idx_name", "files", &["name`, secret"]).is_err());
    }

    #[test]
    fn backend_search_index_builders_render_expected_sql() {
        let postgres = postgres_trigram_index("idx_files_name", "files", "name")
            .to_string(PostgresQueryBuilder);
        assert!(postgres.contains("USING GIN"));
        assert!(postgres.contains("gin_trgm_ops"));

        let drop = postgres_drop_index("idx_files_name").to_string(PostgresQueryBuilder);
        assert_eq!(drop, "DROP INDEX IF EXISTS \"idx_files_name\"");

        let mysql = mysql_fulltext_index_sql("idx_files_name", "files", &["name"]).unwrap();
        assert_eq!(
            mysql,
            "CREATE FULLTEXT INDEX idx_files_name ON files (name) WITH PARSER ngram"
        );
    }
}

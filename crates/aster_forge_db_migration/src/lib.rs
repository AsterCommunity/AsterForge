//! Shared SeaORM migration coordination and schema helpers for Aster products.
//!
//! Products retain their migration lists, history compatibility policy, table definitions, and
//! data backfills. This crate owns the reusable execution mechanics and migration-only helpers.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod coordination;
mod index;
mod schema;
mod search;

pub use coordination::{
    MigrationFuture, MigrationLockOptions, run_migrator_with_lock, with_migration_lock,
};
pub use index::{drop_index_if_exists, rename_mysql_index_if_exists};
pub use schema::{
    big_integer_primary_key, json_text_column_for_final_schema,
    nullable_json_text_column_for_backfill, utc_date_time_column, utc_date_time_column_for_backend,
};
pub use search::{
    SqliteFtsConfig, ensure_postgres_extension, execute_sqlite_statements,
    mysql_fulltext_index_sql, postgres_drop_index, postgres_trigram_index,
    sqlite_fts_down_statements, sqlite_fts_up_statements,
};

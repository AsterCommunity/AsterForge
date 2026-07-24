//! Integration coverage for SQLite connection-pool transaction behavior.
//!
//! The test verifies the public connection helper preserves AsterDrive's single-writer SQLite
//! semantics: concurrent transactions are serialized by the pool instead of silently opening
//! multiple write-capable SQLite connections.

use aster_forge_db::{DatabaseConfig, connect};
use aster_forge_test::temp::{SqliteTestDatabase, TestTempDir, sqlite_database_url};
use sea_orm::{ConnectionTrait, Database, TransactionTrait};
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn sqlite_transactions_are_serialized_by_single_connection_pool() {
    let database = SqliteTestDatabase::new("single-connection-pool");
    let cfg = DatabaseConfig {
        url: database.url().to_string(),
        pool_size: 8,
        retry_count: 0,
    };
    let db = connect(&cfg).await.unwrap();

    let txn = db.begin().await.unwrap();
    let second_begin = timeout(Duration::from_millis(100), db.begin()).await;
    assert!(
        second_begin.is_err(),
        "SQLite should serialize transactions by exposing only one pooled connection"
    );

    txn.commit().await.unwrap();

    let second_txn = timeout(Duration::from_secs(1), db.begin())
        .await
        .expect("second transaction should start after the first commit")
        .unwrap();
    second_txn.commit().await.unwrap();

    db.close().await.unwrap();
}

#[tokio::test]
async fn sqlite_url_preserves_spaces_in_native_temp_paths() {
    let directory = TestTempDir::new("sqlite-url-space");
    let database_path = directory.join("database with space.sqlite3");
    let database_url = sqlite_database_url(&database_path);

    let db = Database::connect(&database_url)
        .await
        .expect("percent-encoded SQLite test URL should connect through SeaORM");
    db.execute_unprepared("CREATE TABLE path_guard (id INTEGER PRIMARY KEY);")
        .await
        .expect("SQLite test database should use the decoded native path");

    assert!(database_path.is_file());
    db.close().await.expect("SQLite test database should close");
}

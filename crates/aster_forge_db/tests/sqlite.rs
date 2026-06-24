//! Integration coverage for SQLite connection-pool transaction behavior.
//!
//! The test verifies the public connection helper preserves AsterDrive's single-writer SQLite
//! semantics: concurrent transactions are serialized by the pool instead of silently opening
//! multiple write-capable SQLite connections.

use aster_forge_db::{DatabaseConfig, connect};
use sea_orm::TransactionTrait;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn sqlite_transactions_are_serialized_by_single_connection_pool() {
    let database_path = format!("/tmp/aster-forge-sqlite-lock-{}.db", uuid::Uuid::new_v4());
    let database_url = format!("sqlite://{database_path}");
    let cfg = DatabaseConfig {
        url: database_url,
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

    let _ = tokio::fs::remove_file(database_path).await;
}

//! Integration coverage for `SQLite` connection-pool transaction behavior.
//!
//! The test verifies the public connection helper preserves `AsterDrive`'s single-writer `SQLite`
//! semantics: concurrent transactions are serialized by the pool instead of silently opening
//! multiple write-capable `SQLite` connections.

use aster_forge_db::{DatabaseConfig, connect};
use aster_forge_test::temp::{SqliteTestDatabase, TestTempDir, sqlite_database_url};
use sea_orm::{ConnectionTrait, Database, TransactionTrait};
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn pool_metrics_read_native_snapshot_without_inventing_reader() {
    use aster_forge_metrics::{DbMetricsRecorder, DbQueryMetric, MetricsRecorder};
    struct PoolSample {
        name: String,
        idle: u32,
        used: u32,
        max: Option<u32>,
    }
    #[derive(Default)]
    struct Capture(std::sync::Mutex<Vec<PoolSample>>);
    impl DbMetricsRecorder for Capture {
        fn enabled(&self) -> bool {
            true
        }
        fn record_db_query(&self, _: &DbQueryMetric) {}
    }
    impl MetricsRecorder for Capture {
        fn db_pool(&self, pool: &str, idle: u32, used: u32, max: Option<u32>) {
            self.0.lock().unwrap().push(PoolSample {
                name: pool.to_owned(),
                idle,
                used,
                max,
            });
        }
    }
    let db = Database::connect("sqlite::memory:").await.unwrap();
    let handles = aster_forge_db::DbHandles::single(db);
    let recorder = Capture::default();
    handles.record_pool_metrics("main", &recorder);
    let snapshots = recorder.0.lock().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].name, "main.writer");
    assert!(snapshots[0].max.is_some_and(|max| max > 0));
    assert!(snapshots[0].idle + snapshots[0].used <= snapshots[0].max.unwrap());
}

#[tokio::test]
async fn sqlite_transactions_are_serialized_by_single_connection_pool() {
    let database = SqliteTestDatabase::new("single-connection-pool");
    let cfg = DatabaseConfig {
        url: database.url().into(),
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

#[tokio::test]
async fn unsupported_driver_never_returns_credentials() {
    for url in [
        aster_forge_db::DatabaseUrl::from(
            "unsupported://secret-user:secret-password@localhost/database".to_owned(),
        ),
        aster_forge_db::DatabaseUrl::credentials(
            "unsupported://localhost/database",
            Some("secret-user".into()),
            Some("secret-password".into()),
        ),
    ] {
        let config = DatabaseConfig {
            url,
            pool_size: 1,
            retry_count: 0,
        };
        let error = connect(&config).await.unwrap_err().to_string();
        assert!(!error.contains("secret-password"), "{error}");
        assert!(!error.contains("secret-user"), "{error}");
    }
}

#[tokio::test]
async fn retry_diagnostics_redact_separated_credentials() {
    let logs = aster_forge_test::logs::CapturedLogs::default();
    let config = DatabaseConfig {
        url: aster_forge_db::DatabaseUrl::credentials(
            "unsupported://localhost/db",
            Some("private-user".into()),
            Some("private-password#".into()),
        ),
        pool_size: 1,
        retry_count: 1,
    };
    let result = logs.capture_future(connect(&config)).await;
    assert!(result.is_err());
    let text = logs.text();
    assert!(text.contains("retrying operation"), "{text}");
    for secret in ["private-user", "private-password", "private-password%23"] {
        assert!(!text.contains(secret), "{text}");
    }
}

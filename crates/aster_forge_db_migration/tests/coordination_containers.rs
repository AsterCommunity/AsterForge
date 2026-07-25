use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aster_forge_db_migration::{
    MigrationLockOptions, drop_index_if_exists, rename_mysql_index_if_exists, with_migration_lock,
};
use aster_forge_test::mysql::MysqlTestContainer;
use aster_forge_test::postgres::PostgresTestContainer;
use aster_forge_test::suite::TestContainerSuite;
use sea_orm_migration::sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, Statement,
};
use tokio::sync::{Barrier, Notify};

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("aster-forge-db-migration"))
}

fn unique_database_name(prefix: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn update_max(max_active: &AtomicUsize, active: usize) {
    let mut current = max_active.load(Ordering::SeqCst);
    while active > current {
        match max_active.compare_exchange(current, active, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

async fn assert_same_namespace_serializes(database: &DatabaseConnection, namespace: &str) {
    let start = Arc::new(Barrier::new(3));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let options = MigrationLockOptions::new(namespace).with_mysql_timeout_seconds(10);

    let spawn_runner = || {
        let database = database.clone();
        let start = start.clone();
        let active = active.clone();
        let max_active = max_active.clone();
        let completed = completed.clone();
        let options = options.clone();
        tokio::spawn(async move {
            start.wait().await;
            with_migration_lock(&database, &options, |_connection| {
                Box::pin(async move {
                    let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                    update_max(&max_active, now_active);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    completed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .await
        })
    };

    let first = spawn_runner();
    let second = spawn_runner();
    start.wait().await;
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();

    assert_eq!(completed.load(Ordering::SeqCst), 2);
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}

async fn assert_callback_error_releases_lock(database: &DatabaseConnection, namespace: &str) {
    let options = MigrationLockOptions::new(namespace).with_mysql_timeout_seconds(2);
    let error = with_migration_lock(database, &options, |_connection| {
        Box::pin(async { Err::<(), _>(DbErr::Custom("expected migration error".to_string())) })
    })
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "Custom Error: expected migration error");

    with_migration_lock(database, &options, |connection| {
        Box::pin(async move {
            connection
                .execute_unprepared("CREATE TABLE migration_lock_released (id INTEGER PRIMARY KEY)")
                .await?;
            Ok(())
        })
    })
    .await
    .unwrap();
}

async fn assert_different_namespaces_can_overlap(
    database: &DatabaseConnection,
    first_namespace: &str,
    second_namespace: &str,
) {
    let entered = Arc::new(Barrier::new(2));
    let spawn_runner = |namespace: String| {
        let database = database.clone();
        let entered = entered.clone();
        tokio::spawn(async move {
            let options = MigrationLockOptions::new(namespace).with_mysql_timeout_seconds(5);
            with_migration_lock(&database, &options, |_connection| {
                Box::pin(async move {
                    entered.wait().await;
                    Ok(())
                })
            })
            .await
        })
    };

    let first = spawn_runner(first_namespace.to_string());
    let second = spawn_runner(second_namespace.to_string());
    tokio::time::timeout(Duration::from_secs(5), async {
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    })
    .await
    .expect("different migration lock namespaces should not block each other");
}

async fn assert_mysql_lock_timeout(database: &DatabaseConnection, namespace: &str) {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Notify::new());
    let holder_database = database.clone();
    let holder_options = MigrationLockOptions::new(namespace).with_mysql_timeout_seconds(5);
    let holder_entered = entered.clone();
    let holder_release = release.clone();
    let holder = tokio::spawn(async move {
        with_migration_lock(&holder_database, &holder_options, |_connection| {
            Box::pin(async move {
                holder_entered.wait().await;
                holder_release.notified().await;
                Ok(())
            })
        })
        .await
    });

    entered.wait().await;
    let timeout_options = MigrationLockOptions::new(namespace).with_mysql_timeout_seconds(0);
    let error = with_migration_lock(database, &timeout_options, |_connection| {
        Box::pin(async { Ok(()) })
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("timed out after 0 seconds"));

    release.notify_one();
    holder.await.unwrap().unwrap();
}

async fn mysql_index_exists(
    database: &DatabaseConnection,
    table_name: &str,
    index_name: &str,
) -> bool {
    database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::MySql,
            "SELECT 1 FROM information_schema.statistics \
             WHERE table_schema = DATABASE() AND table_name = ? AND index_name = ? LIMIT 1",
            [table_name.into(), index_name.into()],
        ))
        .await
        .unwrap()
        .is_some()
}

async fn assert_mysql_index_helpers_are_idempotent(database: &DatabaseConnection) {
    database
        .execute_unprepared(
            "CREATE TABLE forge_index_helper_test (id BIGINT PRIMARY KEY, value BIGINT NOT NULL)",
        )
        .await
        .unwrap();
    database
        .execute_unprepared("CREATE INDEX idx_forge_helper_old ON forge_index_helper_test (value)")
        .await
        .unwrap();

    rename_mysql_index_if_exists(
        database,
        "forge_index_helper_test",
        "idx_forge_helper_old",
        "idx_forge_helper_new",
    )
    .await
    .unwrap();
    assert!(mysql_index_exists(database, "forge_index_helper_test", "idx_forge_helper_new").await);
    assert!(!mysql_index_exists(database, "forge_index_helper_test", "idx_forge_helper_old").await);

    rename_mysql_index_if_exists(
        database,
        "forge_index_helper_test",
        "idx_forge_helper_old",
        "idx_forge_helper_new",
    )
    .await
    .unwrap();
    database
        .execute_unprepared("CREATE INDEX idx_forge_helper_old ON forge_index_helper_test (value)")
        .await
        .unwrap();
    rename_mysql_index_if_exists(
        database,
        "forge_index_helper_test",
        "idx_forge_helper_old",
        "idx_forge_helper_new",
    )
    .await
    .unwrap();
    assert!(mysql_index_exists(database, "forge_index_helper_test", "idx_forge_helper_old").await);
    assert!(mysql_index_exists(database, "forge_index_helper_test", "idx_forge_helper_new").await);

    for index_name in ["idx_forge_helper_old", "idx_forge_helper_new"] {
        drop_index_if_exists(database, "forge_index_helper_test", index_name)
            .await
            .unwrap();
        drop_index_if_exists(database, "forge_index_helper_test", index_name)
            .await
            .unwrap();
        assert!(!mysql_index_exists(database, "forge_index_helper_test", index_name).await);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn postgres_serializes_migrations_and_releases_after_error() {
    let container = PostgresTestContainer::start(test_suite()).await;
    let test_database = container
        .create_database(&unique_database_name("forge_migration_pg"))
        .await;
    let database = test_database.connect().await;

    assert_same_namespace_serializes(&database, "forge:postgres:migration-lock").await;
    assert_different_namespaces_can_overlap(
        &database,
        "forge:postgres:independent-a",
        "forge:postgres:independent-b",
    )
    .await;
    assert_callback_error_releases_lock(&database, "forge:postgres:error-release").await;

    database.close().await.unwrap();
    test_database.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mysql_serializes_migrations_and_releases_after_error() {
    let container = MysqlTestContainer::start(test_suite()).await;
    let database_name = unique_database_name("forge_migration_mysql");
    container.remember_resource(&database_name);
    let root = Database::connect(container.root_url()).await.unwrap();
    root.execute_unprepared(&format!("CREATE DATABASE `{database_name}`"))
        .await
        .unwrap();
    let database = Database::connect(container.database_url(&database_name))
        .await
        .unwrap();

    assert_same_namespace_serializes(&database, "forge:mysql:migration-lock").await;
    assert_different_namespaces_can_overlap(
        &database,
        "forge:mysql:independent-a",
        "forge:mysql:independent-b",
    )
    .await;
    assert_mysql_lock_timeout(&database, "forge:mysql:timeout").await;
    assert_callback_error_releases_lock(&database, "forge:mysql:error-release").await;
    assert_mysql_index_helpers_are_idempotent(&database).await;

    database.close().await.unwrap();
    root.execute_unprepared(&format!("DROP DATABASE `{database_name}`"))
        .await
        .unwrap();
    root.close().await.unwrap();
    container.forget_resource(&database_name);
}

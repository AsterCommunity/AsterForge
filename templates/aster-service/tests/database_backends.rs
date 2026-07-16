//! PostgreSQL and MySQL migration smoke tests used by the CI backend matrix.

use std::time::Duration;

use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::prelude::{MigratorTrait, SchemaManager};
use testcontainers::{
    GenericImage, ImageExt, ReuseDirective, core::IntoContainerPort, runners::AsyncRunner,
};

const TEST_DATABASE_BACKEND_ENV: &str = "ASTER_TEST_DATABASE_BACKEND";
const FOUNDATION_TABLES: [&str; 5] = [
    "runtime_leases",
    "scheduled_tasks",
    "system_config",
    "mail_outbox",
    "audit_logs",
];

#[tokio::test]
async fn foundation_migration_runs_on_configured_backend() {
    let Ok(backend) = std::env::var(TEST_DATABASE_BACKEND_ENV) else {
        return;
    };

    match backend.as_str() {
        "postgres" => exercise_postgres().await,
        "mysql" => exercise_mysql().await,
        other => panic!("unsupported {TEST_DATABASE_BACKEND_ENV} value: {other}"),
    }
}

async fn exercise_postgres() {
    let container = GenericImage::new("postgres", "16")
        .with_exposed_port(5432.tcp())
        .with_container_name("aster-service-postgres-tests")
        .with_reuse(ReuseDirective::Always)
        .with_env_var("POSTGRES_USER", "postgres")
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .with_env_var("POSTGRES_DB", "aster_service")
        .start()
        .await
        .expect("start PostgreSQL test container");
    let port = container
        .get_host_port_ipv4(5432.tcp())
        .await
        .expect("resolve PostgreSQL test port");
    let database_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/aster_service");

    let db = wait_for_database(&database_url).await;
    exercise_foundation_migration(&db).await;
}

async fn exercise_mysql() {
    let container = GenericImage::new("mysql", "8.4")
        .with_exposed_port(3306.tcp())
        .with_container_name("aster-service-mysql-tests")
        .with_reuse(ReuseDirective::Always)
        .with_env_var("MYSQL_DATABASE", "aster_service")
        .with_env_var("MYSQL_USER", "aster")
        .with_env_var("MYSQL_PASSWORD", "asterpass")
        .with_env_var("MYSQL_ROOT_PASSWORD", "rootpass")
        .start()
        .await
        .expect("start MySQL test container");
    let port = container
        .get_host_port_ipv4(3306.tcp())
        .await
        .expect("resolve MySQL test port");
    let database_url = format!("mysql://aster:asterpass@127.0.0.1:{port}/aster_service");

    let db = wait_for_database(&database_url).await;
    exercise_foundation_migration(&db).await;
}

async fn wait_for_database(database_url: &str) -> DatabaseConnection {
    let mut last_error = None;
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match Database::connect(database_url).await {
                Ok(db) => break db,
                Err(error) => {
                    last_error = Some(error.to_string());
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    })
    .await;

    result.unwrap_or_else(|_| {
        panic!(
            "timed out waiting for database {database_url}: {}",
            last_error.unwrap_or_else(|| "unknown connection error".to_string())
        )
    })
}

async fn exercise_foundation_migration(db: &DatabaseConnection) {
    migration::Migrator::up(db, None)
        .await
        .expect("apply foundation migration");
    let manager = SchemaManager::new(db);

    for table in FOUNDATION_TABLES {
        assert!(
            manager
                .has_table(table)
                .await
                .expect("query migrated table"),
            "expected {table} to be created"
        );
    }

    migration::Migrator::down(db, None)
        .await
        .expect("roll back foundation migration");
    for table in FOUNDATION_TABLES {
        assert!(
            !manager
                .has_table(table)
                .await
                .expect("query rolled back table"),
            "expected {table} to be dropped"
        );
    }
}

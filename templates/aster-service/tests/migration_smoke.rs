//! Migration smoke tests.

use sea_orm::Database;
use sea_orm_migration::prelude::MigratorTrait;
use sea_orm_migration::prelude::SchemaManager;

#[test]
fn foundation_migration_is_registered_first() {
    let migrations = migration::Migrator::migrations();

    assert_eq!(migrations.len(), 1);
    assert_eq!(
        migrations[0].name(),
        "m20260627_000001_forge_foundation_schema"
    );
}

#[tokio::test]
async fn foundation_migration_applies_and_rolls_back_on_sqlite() {
    let database_url = format!("sqlite://{}?mode=rwc", unique_database_path().display());
    let db = Database::connect(&database_url)
        .await
        .expect("connect sqlite migration test database");

    migration::Migrator::up(&db, None)
        .await
        .expect("apply foundation migration");
    let manager = SchemaManager::new(&db);

    for table in [
        "runtime_leases",
        "scheduled_tasks",
        "system_config",
        "mail_outbox",
        "audit_logs",
    ] {
        assert!(
            manager
                .has_table(table)
                .await
                .expect("query migrated table"),
            "expected {table} to be created"
        );
    }

    migration::Migrator::down(&db, None)
        .await
        .expect("roll back foundation migration");
    for table in [
        "runtime_leases",
        "scheduled_tasks",
        "system_config",
        "mail_outbox",
        "audit_logs",
    ] {
        assert!(
            !manager
                .has_table(table)
                .await
                .expect("query rolled back table"),
            "expected {table} to be dropped"
        );
    }
}

fn unique_database_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}-migration-{nanos}.db", env!("CARGO_PKG_NAME")))
}

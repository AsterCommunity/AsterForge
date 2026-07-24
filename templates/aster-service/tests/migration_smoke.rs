//! Migration smoke tests.

use aster_forge_test::temp::SqliteTestDatabase;
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
    let database = SqliteTestDatabase::new("foundation-migration");
    let db = Database::connect(database.url())
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
    db.close()
        .await
        .expect("close sqlite migration test database");
}

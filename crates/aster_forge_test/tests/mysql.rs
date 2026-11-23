//! `MySQL` test container contracts: reusable startup, database provisioning, and
//! connection-cache tuning against a real server.
#![cfg(feature = "mysql")]

use aster_forge_test::{
    mysql::{MYSQL_TEST_MAX_CONNECTIONS, MYSQL_TEST_TABLE_DEFINITION_CACHE, MysqlTestContainer},
    suite::TestContainerSuite,
};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

fn unique_database_name(role: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    format!("forge_{role}_{}_{nonce}", std::process::id())
}

#[tokio::test]
async fn shared_mysql_configures_table_definition_cache_for_parallel_schemas() {
    let suite = TestContainerSuite::new("forge-test-mysql-cache");
    let container = MysqlTestContainer::start(&suite).await;
    let database = Database::connect(container.root_url())
        .await
        .expect("MySQL test connection should open");
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@GLOBAL.table_definition_cache",
        ))
        .await
        .expect("MySQL table definition cache should be readable")
        .expect("MySQL table definition cache query should return one row");
    let table_definition_cache: u64 = row
        .try_get_by_index(0)
        .expect("MySQL table definition cache should be an unsigned integer");

    assert!(
        table_definition_cache >= MYSQL_TEST_TABLE_DEFINITION_CACHE,
        "MySQL table definition cache must cover parallel isolated test schemas"
    );

    let max_connections: u64 = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT @@GLOBAL.max_connections",
        ))
        .await
        .expect("MySQL max connections should be readable")
        .expect("MySQL max connections query should return one row")
        .try_get_by_index(0)
        .expect("MySQL max connections should decode");
    assert!(
        max_connections >= MYSQL_TEST_MAX_CONNECTIONS,
        "MySQL max connections must cover nextest process pools"
    );

    database
        .close()
        .await
        .expect("MySQL test connection should close");
}

#[tokio::test]
async fn mysql_shared_database_has_explicit_lifecycle() {
    let suite = TestContainerSuite::new("forge-test-mysql-shared-database");
    let container = MysqlTestContainer::start(&suite).await;
    let database_name = unique_database_name("template");

    container.create_shared_database(&database_name).await;
    let database = Database::connect(container.database_url(&database_name))
        .await
        .expect("shared MySQL database should accept connections");
    database
        .execute_unprepared("CREATE TABLE fixture_marker (id BIGINT PRIMARY KEY)")
        .await
        .expect("shared MySQL database should be writable");
    database
        .close()
        .await
        .expect("shared MySQL database connection should close");

    container.drop_shared_database(&database_name).await;
}

#[tokio::test]
async fn mysql_schema_fixture_reuses_rebuilds_and_recovers_missing_database() {
    use aster_forge_test::fixture::{SchemaFixtureSpec, SuiteFixtureLock};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let suite = TestContainerSuite::new("forge-mysql-schema-fixture");
    let container = MysqlTestContainer::start(&suite).await;
    let resource = unique_database_name("schema");
    let fixture = format!("migrated-{resource}");
    let builds = &AtomicUsize::new(0);
    let spec = SchemaFixtureSpec {
        fixture: &fixture,
        resource: &resource,
        fingerprint: "schema-v1",
        producer_version: "producer-v1",
    };
    // A database left behind before state publication must be removed before migration.
    let _leftover = container.create_shared_database(&resource).await;
    for current in [
        spec,
        spec,
        SchemaFixtureSpec {
            fingerprint: "schema-v2",
            ..spec
        },
        SchemaFixtureSpec {
            fingerprint: "schema-v2",
            producer_version: "producer-v2",
            ..spec
        },
    ] {
        let name = container
            .ensure_schema_template(current, |db| async move {
                builds.fetch_add(1, Ordering::SeqCst);
                db.execute_unprepared("CREATE TABLE marker (id BIGINT PRIMARY KEY)")
                    .await
                    .unwrap();
                db.execute_unprepared("INSERT INTO marker VALUES (1)")
                    .await
                    .unwrap();
            })
            .await;
        assert_eq!(name, resource);
    }
    assert_eq!(builds.load(Ordering::SeqCst), 3);
    container.drop_shared_database(&resource).await;
    let current = SchemaFixtureSpec {
        fingerprint: "schema-v2",
        producer_version: "producer-v2",
        ..spec
    };
    let build = |db: sea_orm::DatabaseConnection| async move {
        builds.fetch_add(1, Ordering::SeqCst);
        db.execute_unprepared("CREATE TABLE marker (id BIGINT PRIMARY KEY)")
            .await
            .unwrap();
    };
    let (one, two) = tokio::join!(
        container.ensure_schema_template(current, build),
        container.ensure_schema_template(current, build)
    );
    assert_eq!(one, two);
    assert_eq!(builds.load(Ordering::SeqCst), 4);
    container.drop_shared_database(&resource).await;
    SuiteFixtureLock::acquire(&suite, spec.fixture).clear();
}

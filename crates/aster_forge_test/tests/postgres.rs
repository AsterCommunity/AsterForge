//! Postgres test container contracts: reusable startup, database provisioning, and
//! isolation between suites against a real server.
#![cfg(feature = "postgres")]

use aster_forge_test::{postgres::PostgresTestContainer, suite::TestContainerSuite};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

fn unique_database_name(role: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    format!("forge_{role}_{}_{nonce}", std::process::id())
}

#[tokio::test]
async fn postgres_database_can_clone_a_product_owned_template() {
    let suite = TestContainerSuite::new("forge-test-postgres-template");
    let container = PostgresTestContainer::start(&suite).await;
    let template = container
        .create_shared_database(&unique_database_name("template"))
        .await;
    let template_connection = template.connect().await;
    template_connection
        .execute_unprepared(
            "CREATE TABLE template_marker (id BIGINT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .await
        .expect("template schema should be created");
    template_connection
        .execute_unprepared("INSERT INTO template_marker (id, value) VALUES (1, 'copied')")
        .await
        .expect("template data should be created");
    template_connection
        .close()
        .await
        .expect("template connection should close before cloning");

    let cloned = container
        .create_database_from_template(&unique_database_name("clone"), template.name())
        .await;
    let cloned_connection = cloned.connect().await;
    let row = cloned_connection
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT value FROM template_marker WHERE id = 1",
        ))
        .await
        .expect("cloned template marker should be queryable")
        .expect("cloned template marker should exist");
    let value: String = row
        .try_get_by_index(0)
        .expect("cloned template marker should decode");
    assert_eq!(value, "copied");
    cloned_connection
        .close()
        .await
        .expect("cloned database connection should close");

    cloned.cleanup().await;
    template.cleanup().await;
}

#[tokio::test]
async fn concurrent_postgres_container_starts_share_one_reusable_container() {
    let suite = TestContainerSuite::new("forge-test-postgres-concurrent-start");
    let (first, second) = tokio::join!(
        PostgresTestContainer::start(&suite),
        PostgresTestContainer::start(&suite)
    );

    assert_eq!(first.container_identity(), second.container_identity());
}

#[tokio::test]
async fn postgres_schema_fixture_reuses_rebuilds_and_recovers_missing_database() {
    use aster_forge_test::fixture::{SchemaFixtureSpec, SuiteFixtureLock};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let suite = TestContainerSuite::new("forge-postgres-schema-fixture");
    let container = PostgresTestContainer::start(&suite).await;
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

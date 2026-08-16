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

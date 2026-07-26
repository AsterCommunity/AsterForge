//! Real-driver coverage for raw database credentials containing URL-reserved characters.

use std::sync::OnceLock;

use aster_forge_db::{DatabaseConfig, connect};
use aster_forge_test::{
    mysql::MysqlTestContainer, postgres::PostgresTestContainer, suite::TestContainerSuite,
};
use sea_orm::{ConnectionTrait, Database};
use url::Url;

const RAW_PASSWORD: &str = "db#[]{}^+=*@:/?%Special";

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("asterforge-db-credentials"))
}

fn base_url_without_userinfo(full_url: &str) -> String {
    let mut url = Url::parse(full_url).expect("test container URL should parse");
    url.set_password(None)
        .expect("test container URL should support a password");
    url.set_username("")
        .expect("test container URL should support a username");
    url.into()
}

fn base_url_for_database_without_userinfo(full_url: &str, database: &str) -> String {
    let mut url = Url::parse(&base_url_without_userinfo(full_url))
        .expect("credential-free test container URL should parse");
    url.set_path(&format!("/{database}"));
    url.into()
}

#[tokio::test]
async fn postgres_connects_with_raw_reserved_password() {
    let container = PostgresTestContainer::start(test_suite()).await;
    let admin = Database::connect(container.admin_url())
        .await
        .expect("PostgreSQL admin connection should open");
    let username = format!("forge_{}", uuid::Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("DROP ROLE IF EXISTS {username}"))
        .await
        .expect("stale PostgreSQL test role should be removable");
    admin
        .execute_unprepared(&format!(
            "CREATE ROLE {username} LOGIN PASSWORD '{RAW_PASSWORD}'"
        ))
        .await
        .expect("PostgreSQL test role should be created");

    let mut config = DatabaseConfig::with_credentials(
        base_url_without_userinfo(container.admin_url()),
        Some(username.clone()),
        Some(RAW_PASSWORD.to_string()),
    );
    config.retry_count = 0;
    let database = connect(&config)
        .await
        .expect("PostgreSQL should accept the raw reserved password");
    database
        .execute_unprepared("SELECT 1")
        .await
        .expect("credentialed PostgreSQL connection should execute a query");
    database
        .close()
        .await
        .expect("PostgreSQL pool should close");

    let wrong_password = "wrong#postgres-secret";
    let mut wrong_config = DatabaseConfig::with_credentials(
        base_url_without_userinfo(container.admin_url()),
        Some(username.clone()),
        Some(wrong_password.to_string()),
    );
    wrong_config.retry_count = 0;
    let error = connect(&wrong_config)
        .await
        .expect_err("wrong PostgreSQL password should reject the connection");
    let message = error.to_string();
    assert!(!message.contains(wrong_password));
    assert!(!message.contains("wrong%23postgres-secret"));

    admin
        .execute_unprepared(&format!("DROP ROLE {username}"))
        .await
        .expect("PostgreSQL test role should be removed");
    admin
        .close()
        .await
        .expect("PostgreSQL admin pool should close");
}

#[tokio::test]
async fn mysql_connects_with_raw_reserved_password() {
    let container = MysqlTestContainer::start(test_suite()).await;
    let admin = Database::connect(container.root_url())
        .await
        .expect("MySQL admin connection should open");
    let username = format!("forge_{}", &uuid::Uuid::new_v4().simple().to_string()[..20]);
    let database_name = format!("forge_{}", uuid::Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("DROP USER IF EXISTS '{username}'@'%'"))
        .await
        .expect("stale MySQL test user should be removable");
    admin
        .execute_unprepared(&format!(
            "CREATE USER '{username}'@'%' IDENTIFIED BY '{RAW_PASSWORD}'"
        ))
        .await
        .expect("MySQL test user should be created");
    admin
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("MySQL test database should be created");
    admin
        .execute_unprepared(&format!(
            "GRANT ALL PRIVILEGES ON {database_name}.* TO '{username}'@'%'"
        ))
        .await
        .expect("MySQL test user should receive product database access");

    let mut config = DatabaseConfig::with_credentials(
        base_url_for_database_without_userinfo(container.root_url(), &database_name),
        Some(username.clone()),
        Some(RAW_PASSWORD.to_string()),
    );
    config.retry_count = 0;
    let database = connect(&config)
        .await
        .expect("MySQL should accept the raw reserved password");
    database
        .execute_unprepared("SELECT 1")
        .await
        .expect("credentialed MySQL connection should execute a query");
    database.close().await.expect("MySQL pool should close");

    let wrong_password = "wrong#mysql-secret";
    let mut wrong_config = DatabaseConfig::with_credentials(
        base_url_for_database_without_userinfo(container.root_url(), &database_name),
        Some(username.clone()),
        Some(wrong_password.to_string()),
    );
    wrong_config.retry_count = 0;
    let error = connect(&wrong_config)
        .await
        .expect_err("wrong MySQL password should reject the connection");
    let message = error.to_string();
    assert!(!message.contains(wrong_password));
    assert!(!message.contains("wrong%23mysql-secret"));

    admin
        .execute_unprepared(&format!("DROP DATABASE {database_name}"))
        .await
        .expect("MySQL test database should be removed");
    admin
        .execute_unprepared(&format!("DROP USER '{username}'@'%'"))
        .await
        .expect("MySQL test user should be removed");
    admin.close().await.expect("MySQL admin pool should close");
}

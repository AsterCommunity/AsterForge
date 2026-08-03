#![cfg(feature = "mysql")]

use aster_forge_test::{
    mysql::{MYSQL_TEST_TABLE_DEFINITION_CACHE, MysqlTestContainer},
    suite::TestContainerSuite,
};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

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

    database
        .close()
        .await
        .expect("MySQL test connection should close");
}

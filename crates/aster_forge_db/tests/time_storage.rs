//! Native timestamp round trips and exact-expiry fences on all three SQL backends.
#![cfg(feature = "runtime-lease")]
use aster_forge_db::runtime_lease::{Entity, RuntimeLeaseDbStore, create_runtime_leases_table};
use aster_forge_runtime::{RuntimeLeaseAcquire, RuntimeLeaseClaim, RuntimeLeaseStore};
use aster_forge_test::{
    mysql::MysqlTestContainer, postgres::PostgresTestContainer, suite::TestContainerSuite,
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, EntityTrait};
use time::{Duration, macros::datetime};

async fn verify(db: DatabaseConnection) {
    db.execute(&create_runtime_leases_table(db.get_database_backend()))
        .await
        .unwrap();
    let store = RuntimeLeaseDbStore::new(db.clone());
    // All backends preserve at least microseconds; no nanosecond rounding assumptions.
    let now = datetime!(2000-02-29 12:34:56.123456 UTC);
    let expires = now + Duration::seconds(30);
    let claim = |owner, at| RuntimeLeaseClaim {
        lease_id: "timestamp-fence",
        owner_id: owner,
        now: at,
        expires_at: at + Duration::seconds(30),
    };
    assert_eq!(
        store.try_acquire(claim("a", now)).await.unwrap(),
        RuntimeLeaseAcquire::Acquired
    );
    let saved = Entity::find_by_id("timestamp-fence")
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.created_at, now);
    assert_eq!(saved.expires_at, expires);
    assert!(matches!(
        store
            .try_acquire(claim("b", expires - Duration::microseconds(1)))
            .await
            .unwrap(),
        RuntimeLeaseAcquire::Standby { .. }
    ));
    assert_eq!(
        store.try_acquire(claim("b", expires)).await.unwrap(),
        RuntimeLeaseAcquire::Acquired
    );
    assert!(
        !store
            .renew(
                "timestamp-fence",
                "a",
                expires,
                expires + Duration::seconds(60)
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .renew(
                "timestamp-fence",
                "b",
                expires,
                expires + Duration::seconds(60)
            )
            .await
            .unwrap()
    );
    db.close().await.unwrap();
}

#[tokio::test]
async fn sqlite_time_round_trip_and_expiry_fence() {
    verify(Database::connect("sqlite::memory:").await.unwrap()).await;
}

#[tokio::test]
async fn postgres_time_round_trip_and_expiry_fence() {
    let suite = TestContainerSuite::new("forge-time-contract");
    let container = PostgresTestContainer::start(&suite).await;
    let database = container
        .create_database(&format!("time_{}", uuid::Uuid::new_v4().simple()))
        .await;
    verify(Database::connect(database.url()).await.unwrap()).await;
}

#[tokio::test]
async fn mysql_time_round_trip_and_expiry_fence() {
    let suite = TestContainerSuite::new("forge-time-contract");
    let container = MysqlTestContainer::start(&suite).await;
    let name = format!("time_{}", uuid::Uuid::new_v4().simple());
    container.create_shared_database(&name).await;
    verify(
        Database::connect(container.database_url(&name))
            .await
            .unwrap(),
    )
    .await;
    container.drop_shared_database(&name).await;
}

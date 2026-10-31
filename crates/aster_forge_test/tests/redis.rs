#![cfg(feature = "redis")]

use aster_forge_test::{redis::RedisTestContainer, suite::TestContainerSuite};

#[tokio::test]
async fn stopped_reusable_redis_becomes_ready_on_the_same_port() {
    let suite = TestContainerSuite::new("forge-test-redis-stopped-reuse");
    let first = RedisTestContainer::start(&suite).await;
    let endpoint = first.url().to_owned();
    first.stop().await;
    drop(first);

    // Exercise the reused-container start path, which skips testcontainers' readiness waits.
    let reused = RedisTestContainer::start(&suite).await;
    assert_eq!(reused.url(), endpoint);
    let client = redis::Client::open(reused.url()).unwrap();
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    let response: String = redis::cmd("PING")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(response, "PONG");
}
